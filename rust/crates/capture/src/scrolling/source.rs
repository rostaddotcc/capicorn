//! Newest-frame source for scrolling capture: ScreenCast portal → PipeWire →
//! `Surface` slot. Linux only.
//!
//! This is the recording pipeline's proven capture half with two deliberate
//! differences:
//!
//! * **no ffmpeg child** — nobody needs every frame, only the newest one, so each
//!   converted frame replaces the previous entry in [`Latest`]. The stitch thread
//!   polls that slot at its own pace and never backlogs an encoder.
//! * **`CursorMode::HIDDEN`** — the pointer sits over the content the whole time
//!   the user scrolls; baking it into every frame would smear a cursor trail down
//!   the stitched image.
//!
//! The traps this module encodes, each of which cost real debugging time in the
//! recording work:
//! * **no `target.object` property** and **no `SPA_PARAM_Buffers` on the
//!   consumer**: both make the portal node resolve/allocate before the registry
//!   has synced, and the stream dies with `no target node available` /
//!   `error alloc buffers: Invalid argument`;
//! * **wait for the node id in the registry** before connecting;
//! * **`VideoSize` as a range**, never a fixated size, or a window that does not
//!   match the hint negotiates nothing;
//! * **BGRx → RGBA is stride-aware** and must not swap R and B;
//! * **teardown order** — the PipeWire thread stops before the portal session is
//!   dropped, and the session is dropped *inside* the runtime that owns its
//!   zbus connection.
//!
//! Two threads, both owned here: a **session thread** (a current-thread tokio
//! runtime that keeps zbus polled for the whole capture) and a **PipeWire thread**
//! (`MainLoopRc` is `!Send`). [`Source::shutdown`] joins both, in order.

use std::cell::{Cell, RefCell};
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use snagcraft_raster::Surface;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::{ChoiceValue, Pod, Property, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id};

use ashpd::desktop::screencast::SourceType;

use crate::CaptureError;

/// Pure D-Bus round-trips: a wedged portal must fail readably, not hang.
const PORTAL_CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// The share dialog: the user has to see it, decide and click "Share".
const PICKER_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a capture may wait for the first frame before it counts as refused.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(10);
/// Session-thread tick: keeps zbus polled and notices a stop quickly.
const SESSION_TICK: Duration = Duration::from_millis(100);

/// The stream's newest frame (or `None` before the first one arrives).
pub(crate) type Latest = Arc<Mutex<Option<Arc<Surface>>>>;

// -- pixel formats ---------------------------------------------------------

/// bytes-per-pixel + byte offsets of R, G and B inside one pixel.
///
/// The `x`/`A` variants are what compositors actually advertise for screencast
/// nodes; getting R/B backwards is a silent colour swap, so every packed layout is
/// listed.
pub(crate) fn pixfmt(format: VideoFormat) -> Option<(usize, [usize; 3])> {
    if format == VideoFormat::RGB {
        Some((3, [0, 1, 2]))
    } else if format == VideoFormat::BGR {
        Some((3, [2, 1, 0]))
    } else if format == VideoFormat::RGBx || format == VideoFormat::RGBA {
        Some((4, [0, 1, 2]))
    } else if format == VideoFormat::BGRx || format == VideoFormat::BGRA {
        Some((4, [2, 1, 0]))
    } else if format == VideoFormat::xRGB || format == VideoFormat::ARGB {
        Some((4, [1, 2, 3]))
    } else if format == VideoFormat::xBGR || format == VideoFormat::ABGR {
        Some((4, [3, 2, 1]))
    } else {
        None
    }
}

/// Convert one packed frame into tightly packed **RGBA**, honouring the producer's
/// row stride (usually `width * bpp`, not always).
///
/// `false` when the buffer cannot cover even one full row or the row stride is
/// nonsense: a garbage buffer must become "no frame", never a panic or a sheared
/// image.
pub(crate) fn convert_into(
    src: &[u8],
    width: usize,
    height: usize,
    bpp: usize,
    idx: [usize; 3],
    stride: usize,
    out: &mut Vec<u8>,
) -> bool {
    let row_bytes = width * bpp;
    if width == 0 || height == 0 || row_bytes == 0 || stride < row_bytes || src.len() < row_bytes {
        return false;
    }

    out.clear();
    out.resize(width * height * 4, 0);

    for y in 0..height {
        let start = y * stride;
        if start + row_bytes > src.len() {
            return false;
        }
        let row = &src[start..start + row_bytes];
        let dst = &mut out[y * width * 4..(y + 1) * width * 4];
        for x in 0..width {
            let pixel = &row[x * bpp..x * bpp + bpp];
            dst[x * 4] = pixel[idx[0]];
            dst[x * 4 + 1] = pixel[idx[1]];
            dst[x * 4 + 2] = pixel[idx[2]];
            dst[x * 4 + 3] = 255;
        }
    }
    true
}

// -- diagnostics -----------------------------------------------------------

/// Append a timestamped line to the shared portal log
/// (`$XDG_CACHE_HOME/snagcraft-portal.log`, i.e. `~/.cache/...`).
///
/// Deliberately best-effort: a log write must never be why a capture fails.
pub(crate) fn log(message: &str) {
    use std::io::Write;

    let path = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("snagcraft-portal.log");

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "[{stamp}] scrolling: {message}");
    }
}

// -- SPA pods --------------------------------------------------------------

/// One `EnumFormat` pod per candidate packed RGB format.
///
/// The caller keeps the `Vec<Vec<u8>>` alive because `Pod::from_bytes` only
/// borrows.
fn make_params(
    width: u32,
    height: u32,
    formats: &[VideoFormat],
) -> Result<Vec<Vec<u8>>, CaptureError> {
    let mut out = Vec::new();
    for format in formats {
        let properties = vec![
            Property::new(
                FormatProperties::MediaType.as_raw(),
                Value::Id(Id(MediaType::Video.as_raw())),
            ),
            Property::new(
                FormatProperties::MediaSubtype.as_raw(),
                Value::Id(Id(MediaSubtype::Raw.as_raw())),
            ),
            Property::new(
                FormatProperties::VideoFormat.as_raw(),
                Value::Id(Id(format.as_raw())),
            ),
            Property::new(
                FormatProperties::VideoSize.as_raw(),
                // A RANGE with the hint as default, never a fixated size: the
                // portal's hint can be wrong for an individual window, and a
                // fixated size then negotiates nothing (no frames at all, which
                // looks exactly like "the user cancelled").
                Value::Choice(ChoiceValue::Rectangle(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: spa::utils::Rectangle { width, height },
                        min: spa::utils::Rectangle {
                            width: 1,
                            height: 1,
                        },
                        max: spa::utils::Rectangle {
                            width: snagcraft_raster::MAX_DIM,
                            height: snagcraft_raster::MAX_DIM,
                        },
                    },
                ))),
            ),
        ];
        let object = spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties,
        };
        let bytes = spa::pod::serialize::PodSerializer::serialize(
            Cursor::new(Vec::new()),
            &spa::pod::Value::Object(object),
        )
        .map_err(|err| {
            CaptureError::Portal(format!("cannot serialise the video format pod: {err:?}"))
        })?
        .0
        .into_inner();
        out.push(bytes);
    }
    Ok(out)
}

/// Await one portal call, turning a hang into a readable, logged error.
async fn portal_call<T>(
    timeout: Duration,
    what: &str,
    fut: impl std::future::Future<Output = Result<T, ashpd::Error>>,
) -> Result<T, CaptureError> {
    match tokio::time::timeout(timeout, fut).await {
        Err(_) => {
            let message = format!(
                "{what} timed out after {}s — is xdg-desktop-portal healthy and the screen \
                 unlocked?",
                timeout.as_secs()
            );
            log(&message);
            Err(CaptureError::Portal(message))
        }
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => {
            let message = format!("{what} failed: {err}");
            log(&message);
            Err(CaptureError::Portal(message))
        }
    }
}

/// A started screencast: the PipeWire fd, the node to connect to, and the
/// session that must outlive it.
struct PortalCapture {
    fd: OwnedFd,
    node: u32,
    /// The portal's size hint — only an enumeration default; `param_changed` has
    /// the truth.
    size: (i32, i32),
    session: ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>,
}

/// Create the session, let the user pick a monitor or window, start the cast and
/// open the PipeWire remote.
///
/// The portal offers no rectangle source: the region the user selected is cropped
/// in-app from this monitor/window stream.
async fn open_portal(sources: SourceType) -> Result<PortalCapture, CaptureError> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions};
    use ashpd::desktop::{PersistMode, Session};

    let proxy = portal_call(PORTAL_CALL_TIMEOUT, "ScreenCast portal", Screencast::new()).await?;

    let session: Session<Screencast> = portal_call(
        PORTAL_CALL_TIMEOUT,
        "CreateSession",
        proxy.create_session(Default::default()),
    )
    .await?;

    // `select_sources` hands back an empty `Request`; ashpd's own example drops it.
    let selection = portal_call(
        PORTAL_CALL_TIMEOUT,
        "SelectSources",
        proxy.select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_sources(sources)
                // HIDDEN, not EMBEDDED: a cursor baked into every frame would be
                // stitched into the tall image once per scroll step.
                .set_cursor_mode(CursorMode::HIDDEN)
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        ),
    )
    .await?;
    drop(selection);

    // `start` opens the picker; it resolves only once the user clicks Share.
    let request = portal_call(
        PORTAL_CALL_TIMEOUT,
        "Start screen cast",
        proxy.start(&session, None, Default::default()),
    )
    .await?;

    let streams = match tokio::time::timeout(PICKER_TIMEOUT, async { request.response() }).await {
        Err(_) => {
            let message = format!(
                "the screen-share dialog was not answered within {}s (a locked screen blocks \
                 it) — unlock and try again",
                PICKER_TIMEOUT.as_secs()
            );
            log(&message);
            return Err(CaptureError::Portal(message));
        }
        Ok(Err(err)) => {
            let message = format!("screen sharing was refused: {err}");
            log(&message);
            return Err(CaptureError::Portal(message));
        }
        Ok(Ok(streams)) => streams,
    };

    let stream = streams.streams().first().ok_or_else(|| {
        CaptureError::Portal(
            "the portal returned no streams (this compositor may not support monitor capture)"
                .to_string(),
        )
    })?;
    let node = stream.pipe_wire_node_id();
    let size = stream.size().unwrap_or((1920, 1080));

    let fd = portal_call(
        PORTAL_CALL_TIMEOUT,
        "OpenPipeWireRemote",
        proxy.open_pipe_wire_remote(&session, Default::default()),
    )
    .await?;

    log(&format!(
        "cast started: node={node} hint={}x{}",
        size.0, size.1
    ));

    Ok(PortalCapture {
        fd,
        node,
        size,
        session,
    })
}

// -- PipeWire half ---------------------------------------------------------

/// User data shared with the PipeWire callbacks.
struct Ud {
    fmt: VideoInfoRaw,
    latest: Latest,
    /// Reports the first frame (or the first fatal error) to the session thread.
    #[allow(clippy::type_complexity)]
    notify: Rc<RefCell<Option<mpsc::SyncSender<Result<(), String>>>>>,
    stop: Arc<AtomicBool>,
    loopweak: pw::main_loop::MainLoopWeak,
    /// Scratch for the RGBA conversion, handed to the published frame.
    buf: Vec<u8>,
    seen: Rc<Cell<u64>>,
}

fn quit(ud: &Ud) {
    if let Some(mainloop) = ud.loopweak.upgrade() {
        mainloop.quit();
    }
}

fn report(ud: &Ud, result: Result<(), String>) {
    if let Some(tx) = ud.notify.borrow_mut().take() {
        // Never block a real-time PipeWire callback on the command side.
        let _ = tx.try_send(result);
    }
}

/// The whole PipeWire half for one capture. Returns once the stream ended (stop
/// requested, error, or the compositor closed the cast).
fn capture(
    fd: OwnedFd,
    node: u32,
    size: (i32, i32),
    latest: Latest,
    stop: Arc<AtomicBool>,
    started: mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|err| format!("cannot create the PipeWire main loop: {err}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|err| format!("cannot create the PipeWire context: {err}"))?;
    let core = context
        .connect_fd_rc(fd, None)
        .map_err(|err| format!("cannot connect to the portal's PipeWire fd: {err}"))?;

    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    // NOTE: no `target.object` property here on purpose — the node id goes to
    // `stream.connect` after the registry wait below.
    let stream = pw::stream::StreamBox::new(&core, "snagcraft-scrolling", props)
        .map_err(|err| format!("cannot create the capture stream: {err}"))?;

    let notify = Rc::new(RefCell::new(Some(started)));
    let seen = Rc::new(Cell::new(0u64));

    let ud = Ud {
        fmt: VideoInfoRaw::new(),
        latest: latest.clone(),
        notify: notify.clone(),
        stop: stop.clone(),
        loopweak: mainloop.downgrade(),
        buf: Vec::new(),
        seen: seen.clone(),
    };

    let listener = stream
        .add_local_listener_with_user_data(ud)
        .param_changed(|_, ud, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else { return };
            match format_utils::parse_format(param) {
                Ok((MediaType::Video, MediaSubtype::Raw)) => {}
                _ => return,
            }
            if ud.fmt.parse(param).is_ok() {
                let size = ud.fmt.size();
                log(&format!(
                    "format negotiated {:?} {}x{}",
                    ud.fmt.format(),
                    size.width,
                    size.height
                ));
            }
        })
        .state_changed(|_, _, old, new| {
            if old != new {
                tracing::debug!(?old, ?new, "scrolling stream state");
            }
        })
        .process(|stream, ud| {
            // Polled here as well as from the timer, so a stop lands on the very
            // next frame instead of waiting for the tick.
            if ud.stop.load(Ordering::Relaxed) {
                quit(ud);
                return;
            }

            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }

            let size = ud.fmt.size();
            let (width, height) = (size.width as usize, size.height as usize);
            let Some((bpp, idx)) = pixfmt(ud.fmt.format()) else {
                // No packed RGB/BGR: nothing to stitch, and the user must be told
                // rather than left with a capture that never fills.
                report(
                    ud,
                    Err("this compositor offers no packed RGB/BGR capture format".to_string()),
                );
                quit(ud);
                return;
            };

            let data = &mut datas[0];
            let (offset, chunk_size, stride) = {
                let chunk = data.chunk();
                (
                    chunk.offset() as usize,
                    chunk.size() as usize,
                    chunk.stride(),
                )
            };
            let Some(raw) = data.data() else { return };

            // Buffers can be smaller than the frame: never index past what is
            // actually mapped.
            if offset + chunk_size > raw.len() || chunk_size < width * bpp {
                return;
            }
            let src = &raw[offset..offset + chunk_size];
            let stride = if stride > 0 {
                stride as usize
            } else {
                width * bpp
            };

            if !convert_into(src, width, height, bpp, idx, stride, &mut ud.buf) {
                return;
            }

            let Some(surface) =
                Surface::from_raw(width as u32, height as u32, std::mem::take(&mut ud.buf))
            else {
                return;
            };

            // Publish: the consumer clones the `Arc`, so it never blocks on a
            // multi-megabyte frame.
            let frame = Arc::new(surface);
            let first = ud.seen.get() == 0;
            match ud.latest.lock() {
                Ok(mut slot) => *slot = Some(frame),
                Err(poisoned) => *poisoned.into_inner() = Some(frame),
            }
            ud.seen.set(ud.seen.get() + 1);
            if first {
                report(ud, Ok(()));
            }
        })
        .register()
        .map_err(|err| format!("cannot register the capture callbacks: {err}"))?;

    let (width, height) = (size.0.max(1) as u32, size.1.max(1) as u32);
    let pods = make_params(
        width,
        height,
        &[
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::BGR,
            VideoFormat::RGB,
        ],
    )
    .map_err(|err| err.to_string())?;
    // No explicit SPA_PARAM_Buffers: the producer's MAP_BUFFERS defaults are
    // correct, and an explicit pool is rejected by KWin.
    let mut params: Vec<&Pod> = pods
        .iter()
        .filter_map(|bytes| Pod::from_bytes(bytes))
        .collect();

    // Wait until the portal node is visible in this connection's registry: a
    // `target.object` property would resolve before that and fail.
    if let Ok(registry) = core.get_registry() {
        let node_seen = Rc::new(Cell::new(false));
        let seen_flag = node_seen.clone();
        let _registry_listener = registry
            .add_listener_local()
            .global(move |global| {
                if global.id == node {
                    seen_flag.set(true);
                }
            })
            .register();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !node_seen.get() && std::time::Instant::now() < deadline {
            let _ = mainloop
                .loop_()
                .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(20)));
        }
    }

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|err| format!("cannot connect the capture stream: {err}"))?;

    // Poll the stop flag from the loop: a static screen produces no buffers, so a
    // stop that only lived in `process()` could never land.
    let tick_weak = mainloop.downgrade();
    let tick_stop = stop.clone();
    let tick = mainloop.loop_().add_timer(move |_| {
        if tick_stop.load(Ordering::Relaxed)
            && let Some(mainloop) = tick_weak.upgrade()
        {
            mainloop.quit();
        }
    });
    let _ = tick.update_timer(
        Some(Duration::from_millis(100)),
        Some(Duration::from_millis(100)),
    );

    mainloop.run();

    log(&format!("stream ended after {} frames", seen.get()));

    // If the stream ended before the first frame (cancel, compositor quit, no
    // packed format) the session thread is still waiting.
    if let Some(tx) = notify.borrow_mut().take() {
        let _ = tx.try_send(Err(
            "the screen cast ended before the first frame — was the share cancelled?".to_string(),
        ));
    }

    // Destroying PipeWire objects after the loop returned needs a sync roundtrip
    // nobody services -> deadlock. This thread is about to exit, so leaking them
    // is the safe teardown.
    std::mem::forget(tick);
    std::mem::forget(listener);
    std::mem::forget(stream);
    std::mem::forget(mainloop);

    Ok(())
}

// -- session thread + public source ---------------------------------------

/// A live capture: the newest-frame slot plus the two threads that keep frames
/// arriving. Dropping or shutting it down is the only correct teardown.
pub(crate) struct Source {
    latest: Latest,
    /// Asked the session thread to wind the cast down.
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    size: (i32, i32),
}

impl Source {
    /// The newest frame, if any has arrived yet.
    pub(crate) fn newest(&self) -> Option<Arc<Surface>> {
        match self.latest.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The stream's reported size — physical pixels, which is what a crop must be
    /// expressed in (not CSS px: fractional scaling makes them differ).
    pub(crate) fn size(&self) -> (i32, i32) {
        self.size
    }

    /// Stop the PipeWire thread and the portal session, in that order. Safe to
    /// call twice.
    pub(crate) fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The session thread: portal → PipeWire thread → keep the cast alive until asked
/// to stop.
///
/// Everything D-Bus lives inside `block_on`, so the zbus connection stays polled
/// (an un-polled connection is what silently kills a cast) and the `Session` is
/// dropped *inside* the runtime that owns it.
fn session(
    latest: Latest,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<(), String>>,
    size_tx: mpsc::SyncSender<(i32, i32)>,
    sources: SourceType,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let message = format!("cannot start the portal runtime: {err}");
            log(&message);
            let _ = ready.try_send(Err(message));
            return;
        }
    };

    runtime.block_on(async move {
        let portal = match open_portal(sources).await {
            Ok(portal) => portal,
            Err(err) => {
                let _ = ready.try_send(Err(err.to_string()));
                return;
            }
        };
        let PortalCapture {
            fd,
            node,
            size,
            session,
        } = portal;
        let _ = size_tx.try_send(size);

        let stream_stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let latest = latest.clone();
            let stream_stop = Arc::clone(&stream_stop);
            let started = ready.clone();
            std::thread::Builder::new()
                .name("snagcraft-scrolling-source".to_string())
                .spawn(move || {
                    if let Err(err) = capture(fd, node, size, latest, stream_stop, started.clone())
                    {
                        log(&format!("screen cast stopped: {err}"));
                        let _ = started.try_send(Err(err));
                    }
                })
        };

        let thread = match thread {
            Ok(thread) => thread,
            Err(err) => {
                let message = format!("cannot start the PipeWire thread: {err}");
                log(&message);
                let _ = ready.try_send(Err(message));
                return;
            }
        };

        // Keep zbus polled (and notice a stop) for the whole capture.
        while !stop.load(Ordering::SeqCst) {
            tokio::time::sleep(SESSION_TICK).await;
        }

        // PipeWire thread first, then the session: dropping the session earlier
        // ends the cast under the capture thread's feet.
        stream_stop.store(true, Ordering::SeqCst);
        let _ = thread.join();
        drop(session);
        log("session closed");
    });
}

/// Open the portal cast, start both threads, and wait for the first real frame.
///
/// Blocking by design: the share dialog alone can take up to two minutes, so call
/// this from a dedicated thread, never from an async runtime or the UI thread. A
/// refused or unanswered dialog, a compositor with no packed RGB format, or a cast
/// that never delivers a frame all come back as a real [`CaptureError`] instead of
/// an empty capture.
pub(crate) fn open_blocking() -> Result<Source, CaptureError> {
    open_blocking_kind(SourceType::MONITOR | SourceType::WINDOW)
}

/// W41: open the cast offering only the given source kind(s). The scrolling
/// capture offers MONITOR|WINDOW; a still window/monitor grab offers just one
/// so the OS picker lists exactly what the user asked for (Tauri parity).
pub(crate) fn open_blocking_kind(sources: SourceType) -> Result<Source, CaptureError> {
    let latest: Latest = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (size_tx, size_rx) = mpsc::sync_channel(1);

    let thread = {
        let latest = Arc::clone(&latest);
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("snagcraft-scrolling-session".to_string())
            .spawn(move || session(latest, stop, ready_tx, size_tx, sources))
            .map_err(|err| {
                CaptureError::Io(format!("cannot start the scrolling session thread: {err}"))
            })?
    };

    // The picker and the first frame share one budget: the dialog has to be
    // answered before a frame can exist.
    let budget = PICKER_TIMEOUT + FIRST_FRAME_WAIT;
    let signal = match ready_rx.recv_timeout(budget) {
        Ok(signal) => signal,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "no screen-cast frames arrived within {}s — was the share dialog cancelled?",
            budget.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("the screen-cast pipeline could not be started".to_string())
        }
    };

    if let Err(message) = signal {
        stop.store(true, Ordering::SeqCst);
        let _ = thread.join();
        log(&message);
        return Err(CaptureError::Portal(message));
    }

    let size = size_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or((1920, 1080));

    Ok(Source {
        latest,
        stop,
        join: Some(thread),
        size,
    })
}

/// W41: grab exactly ONE still frame from a user-picked window or monitor.
///
/// Reuses the scrolling cast machinery end to end: the OS picker (SelectSources
/// then Start), the PipeWire thread, the stride-aware RGBA conversion and the
/// graceful teardown. `open_blocking_kind` already waits for the first real
/// frame, so `newest()` is populated; the `Source` is shut down (PipeWire thread
/// joined, then the portal `Session` dropped) before the frame is handed back.
/// This restores the Tauri `portal_capture::grab` path on the Rust port — the
/// system window/screen picker the Screenshot portal cannot offer.
pub(crate) fn grab_one_blocking(sources: SourceType) -> Result<Surface, CaptureError> {
    let mut source = open_blocking_kind(sources)?;
    // The ready signal fires on the first published frame; poll briefly in case
    // the slot is read a hair before the publish lands.
    let mut frame = None;
    for _ in 0..50 {
        if let Some(found) = source.newest() {
            frame = Some(found);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = frame
        .ok_or_else(|| CaptureError::Portal("the screen cast produced no frame".to_string()))?;
    let surface = (*frame).clone();
    source.shutdown();
    log(&format!(
        "still grab: captured {}x{}",
        surface.width(),
        surface.height()
    ));
    Ok(surface)
}
