//! "Latest frame" source for scrolling capture: ScreenCast portal → PipeWire →
//! `rgb24` slot.
//!
//! This is `recording.rs`'s proven capture half with two deliberate differences
//! (`tmp/research/scrolling.md` §3, §5.5):
//!
//! * **no ffmpeg child** — nobody needs every frame, only the newest one, so each
//!   converted frame replaces the previous entry in [`Latest`]. The stitcher
//!   polls that slot at its own pace and never backlogs an encoder.
//! * **`CursorMode::HIDDEN`** — the pointer sits over the content the whole time
//!   the user scrolls; baking it into every frame would smear a cursor trail down
//!   the stitched image.
//!
//! Everything else (portal timeouts, the `SPA_PARAM_Buffers` fix, stride-aware
//! BGRx/RGBx conversion, and leaking PipeWire objects on teardown to dodge the
//! drop deadlock) is copied from `recording.rs` on purpose: that path is verified.
//!
//! The portal `Session` must outlive the stream. [`Source`] owns it and hands it
//! back through [`Source::take_session`] so the caller can drop it on the async
//! runtime *after* the PipeWire thread has stopped.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use pipewire as pw;
use pw::spa;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::{Pod, Property, Value};
use spa::utils::Id;

/// Pure D-Bus round-trips: a wedged portal must fail readably, not hang.
const PORTAL_CALL_TIMEOUT: u64 = 15;
/// The share dialog: the user has to see it, decide and click "Share".
const PICKER_TIMEOUT: u64 = 120;
/// How long a capture may wait for the first frame before it counts as refused.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(10);

/// One converted frame. Shared as `Arc` so the consumer clones a pointer, not
/// three megabytes of pixels.
pub struct Frame {
    pub bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// The stream's newest frame (or `None` before the first one arrives).
pub type Latest = Arc<Mutex<Option<Arc<Frame>>>>;

// -- pixel formats ---------------------------------------------------------

/// bytes-per-pixel + byte offsets of R, G and B inside one pixel.
///
/// The `x`/`A` variants are what compositors actually advertise for screencast
/// nodes; getting R/B backwards is a silent colour swap, so every packed layout
/// is listed.
fn pixfmt(format: VideoFormat) -> Option<(usize, [usize; 3])> {
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

/// Convert one packed frame into tightly packed `rgb24`, honouring the producer's
/// row stride (usually `width * bpp`, but not always).
fn convert_into(
    src: &[u8],
    width: usize,
    height: usize,
    bpp: usize,
    idx: [usize; 3],
    stride: usize,
    out: &mut Vec<u8>,
) -> bool {
    let row_bytes = width * bpp;
    if width == 0 || height == 0 || row_bytes == 0 || src.len() < row_bytes {
        return false;
    }

    out.clear();
    out.resize(width * height * 3, 0);

    for y in 0..height {
        let start = y * stride;
        if start + row_bytes > src.len() {
            return false;
        }
        let row = &src[start..start + row_bytes];
        let dst = &mut out[y * width * 3..(y + 1) * width * 3];
        for x in 0..width {
            let p = &row[x * bpp..x * bpp + bpp];
            dst[x * 3] = p[idx[0]];
            dst[x * 3 + 1] = p[idx[1]];
            dst[x * 3 + 2] = p[idx[2]];
        }
    }
    true
}

// -- SPA pods (verbatim from recording.rs: this is the fiddly part) --------

/// One `EnumFormat` pod per candidate format; the caller keeps the byte vectors
/// alive because `Pod::from_bytes` only borrows.
fn make_params(width: u32, height: u32, formats: &[VideoFormat]) -> Result<Vec<Vec<u8>>, String> {
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
                Value::Rectangle(spa::utils::Rectangle { width, height }),
            ),
        ];
        let object = spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties,
        };
        let bytes: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
            Cursor::new(Vec::new()),
            &spa::pod::Value::Object(object),
        )
        .map_err(|_| "cannot serialise the video format pod".to_string())?
        .0
        .into_inner();
        out.push(bytes);
    }
    Ok(out)
}

/// `SPA_PARAM_Buffers`. Without it the pool is 8 KiB and every frame is silently
/// truncated (`recording.md` §6.1 — the trap that cost the most probe time).
fn buffers_param(size_per_frame: u32, stride: u32, count: i32) -> Result<Vec<u8>, String> {
    let properties = vec![
        Property::new(spa::sys::SPA_PARAM_BUFFERS_buffers, Value::Int(count)),
        Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
        Property::new(
            spa::sys::SPA_PARAM_BUFFERS_size,
            Value::Int(size_per_frame as i32),
        ),
        Property::new(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride as i32)),
    ];
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties,
    };
    let bytes: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map_err(|_| "cannot serialise the buffer pod".to_string())?
    .0
    .into_inner();
    Ok(bytes)
}

/// Await one portal call, turning a hang into a readable error.
async fn portal_call<T>(
    secs: u64,
    what: &str,
    fut: impl Future<Output = Result<T, ashpd::Error>>,
) -> Result<T, String> {
    match tokio::time::timeout(Duration::from_secs(secs), fut).await {
        Err(_) => Err(format!(
            "{what} timed out after {secs}s — is xdg-desktop-portal healthy and the screen unlocked?"
        )),
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(format!("{what} failed: {err}")),
    }
}

/// A live capture: the newest frame slot plus everything that must stay alive
/// (PipeWire thread, portal session) for frames to keep arriving.
pub struct Source {
    latest: Latest,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    size: (i32, i32),
    session: Option<Session<Screencast>>,
}

impl Source {
    /// The stream's reported size — physical pixels, which is what a crop must be
    /// expressed in (not CSS px: fractional scaling makes them differ).
    pub fn size(&self) -> (i32, i32) {
        self.size
    }

    /// A handle the capture loop polls for the newest frame.
    pub fn latest(&self) -> Latest {
        self.latest.clone()
    }

    /// The flag that stops the PipeWire thread.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Stop and join the PipeWire thread. Safe to call twice.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }

    /// Hand the portal session back so the caller can drop it on the async
    /// runtime (the `Close` round-trip needs a tokio context).
    pub fn take_session(&mut self) -> Option<Session<Screencast>> {
        self.session.take()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// -- portal half ----------------------------------------------------------

struct PortalCapture {
    fd: OwnedFd,
    node: u32,
    size: (i32, i32),
    session: Session<Screencast>,
}

/// Create the session, let the user pick the monitor, start the cast, open the
/// PipeWire remote. The session must outlive the capture.
async fn open_portal() -> Result<PortalCapture, String> {
    let proxy = portal_call(PORTAL_CALL_TIMEOUT, "ScreenCast portal", Screencast::new()).await?;

    let session = portal_call(
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
                .set_sources(SourceType::MONITOR)
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

    let streams = match tokio::time::timeout(Duration::from_secs(PICKER_TIMEOUT), async {
        request.response()
    })
    .await
    {
        Err(_) => {
            return Err(format!(
                "the screen-share dialog was not answered within {PICKER_TIMEOUT}s \
                 (a locked screen blocks it) — unlock and try again"
            ))
        }
        Ok(Err(err)) => return Err(format!("screen sharing was refused: {err}")),
        Ok(Ok(streams)) => streams,
    };

    let stream = streams
        .streams()
        .first()
        .ok_or_else(|| {
            "the portal returned no streams (this compositor may not support monitor capture)"
                .to_string()
        })?;
    let node = stream.pipe_wire_node_id();
    // Only a hint for format enumeration; `param_changed` has the truth.
    let size = stream.size().unwrap_or((1920, 1080));

    let fd = portal_call(
        PORTAL_CALL_TIMEOUT,
        "OpenPipeWireRemote",
        proxy.open_pipe_wire_remote(&session, Default::default()),
    )
    .await?;

    Ok(PortalCapture {
        fd,
        node,
        size,
        session,
    })
}

// -- capture half ---------------------------------------------------------

struct Ud {
    fmt: VideoInfoRaw,
    latest: Latest,
    /// Report the first frame (or the first fatal error) to [`open`].
    notify: Rc<RefCell<Option<mpsc::SyncSender<Result<(), String>>>>>,
    stop: Arc<AtomicBool>,
    loopweak: pw::main_loop::MainLoopWeak,
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

/// The whole capture half for one scrolling capture. Returns once the stream
/// ended (stop requested, error, or the compositor closed the cast).
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
        .map_err(|e| format!("cannot create the PipeWire main loop: {e}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| format!("cannot create the PipeWire context: {e}"))?;
    let core = context
        .connect_fd_rc(fd, None)
        .map_err(|e| format!("cannot connect to the portal PipeWire fd: {e}"))?;

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    // `pw::keys::TARGET_OBJECT` is feature-gated in pipewire 0.10: literal key.
    props.insert("target.object", node.to_string());

    let stream = pw::stream::StreamBox::new(&core, "capicorn-scrolling", props)
        .map_err(|e| format!("cannot create the PipeWire capture stream: {e}"))?;

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
                eprintln!(
                    "capicorn: scrolling capture {:?} {}x{}",
                    ud.fmt.format(),
                    size.width,
                    size.height
                );
            }
        })
        .state_changed(|_, _, old, new| {
            if old != new {
                eprintln!("capicorn: scrolling stream {old:?} -> {new:?}");
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

            // Publish: take the buffer (no copy) and swap it into the slot. The
            // consumer clones the `Arc`, so it never blocks on three megabytes.
            let frame = Arc::new(Frame {
                bytes: std::mem::take(&mut ud.buf),
                width,
                height,
            });
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
        .map_err(|e| format!("cannot register the capture callbacks: {e}"))?;

    let (width, height) = (size.0.max(1) as u32, size.1.max(1) as u32);
    let mut pods = make_params(
        width,
        height,
        &[
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::BGR,
            VideoFormat::RGB,
        ],
    )?;
    // Without this the pool is 8 KiB and every frame is truncated.
    pods.push(buffers_param(width * height * 4, width * 4, 8)?);
    let mut params: Vec<&Pod> = pods.iter().filter_map(|v| Pod::from_bytes(v)).collect();

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|e| format!("cannot connect the capture stream: {e}"))?;

    // Poll the stop flag from the loop: a static screen produces no buffers, so a
    // stop that only lived in `process()` could never land.
    let tick_weak = mainloop.downgrade();
    let tick_stop = stop.clone();
    let tick = mainloop.loop_().add_timer(move |_| {
        if tick_stop.load(Ordering::Relaxed) {
            if let Some(mainloop) = tick_weak.upgrade() {
                mainloop.quit();
            }
        }
    });
    let _ = tick.update_timer(Some(Duration::from_millis(100)), Some(Duration::from_millis(100)));

    mainloop.run();

    eprintln!(
        "capicorn: scrolling capture ended after {} frames",
        seen.get()
    );

    // If the stream ended before the first frame (cancel, compositor quit, no
    // packed format) the command is still waiting on `notify`.
    if notify.borrow().is_some() {
        if let Some(tx) = notify.borrow_mut().take() {
            let _ = tx.try_send(Err(
                "the screen cast ended before the first frame — was the share cancelled?"
                    .to_string(),
            ));
        }
    }

    // Destroying PipeWire objects after the loop returned needs a sync roundtrip
    // nobody services -> deadlock (`recording.md` §6.5). This thread is about to
    // exit, so leaking them is the safe teardown.
    std::mem::forget(tick);
    std::mem::forget(listener);
    std::mem::forget(stream);
    std::mem::forget(mainloop);

    Ok(())
}

/// Open a portal cast, start the PipeWire thread and wait for the first frame.
///
/// Resolves only when real frames flow, so a refused/unanswered share dialog is a
/// rejected promise instead of an empty capture.
pub async fn open() -> Result<Source, String> {
    let portal = open_portal().await?;
    let size = portal.size;

    let latest: Latest = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let (started_tx, started_rx) = mpsc::sync_channel(1);

    let thread = {
        let latest = latest.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("capicorn-scrolling-source".to_string())
            .spawn(move || {
                let result = capture(portal.fd, portal.node, portal.size, latest, stop, started_tx);
                if let Err(err) = &result {
                    eprintln!("capicorn: scrolling source stopped: {err}");
                }
            })
            .map_err(|e| format!("cannot start the capture thread: {e}"))?
    };

    let signal = match tauri_async_wait(started_rx).await {
        Ok(signal) => signal,
        Err(err) => {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
            drop(portal.session);
            return Err(err);
        }
    };

    let failure = match signal {
        Ok(Ok(())) => None,
        Ok(Err(err)) => Some(err),
        Err(mpsc::RecvTimeoutError::Disconnected) => Some(
            "the capture pipeline could not be started — see the capicorn log for details"
                .to_string(),
        ),
        Err(mpsc::RecvTimeoutError::Timeout) => Some(format!(
            "no frames arrived within {}s — the screen cast was probably cancelled",
            FIRST_FRAME_WAIT.as_secs()
        )),
    };

    if let Some(err) = failure {
        stop.store(true, Ordering::Relaxed);
        let _ = thread.join();
        drop(portal.session);
        return Err(err);
    }

    Ok(Source {
        latest,
        stop,
        join: Some(thread),
        size,
        session: Some(portal.session),
    })
}

/// Wait for the first-frame signal without blocking the async runtime.
async fn tauri_async_wait(
    rx: mpsc::Receiver<Result<(), String>>,
) -> Result<Result<Result<(), String>, mpsc::RecvTimeoutError>, String> {
    tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(FIRST_FRAME_WAIT))
        .await
        .map_err(|e| format!("waiting for the first frame failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgrx_converts_without_a_red_blue_swap() {
        let src = [0u8, 0, 255, 255, 255, 0, 0, 255];
        let (bpp, idx) = pixfmt(VideoFormat::BGRx).expect("BGRx supported");
        let mut out = Vec::new();
        assert!(convert_into(&src, 2, 1, bpp, idx, 2 * bpp, &mut out));
        assert_eq!(out, vec![255, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn padded_stride_is_honoured() {
        let src = [10u8, 20, 30, 255, 99, 99, 99, 99];
        let (bpp, idx) = pixfmt(VideoFormat::BGRx).expect("BGRx supported");
        let mut out = Vec::new();
        assert!(convert_into(&src, 1, 1, bpp, idx, 8, &mut out));
        assert_eq!(out, vec![30, 20, 10], "B,G,R -> rgb24");
    }

    #[test]
    fn truncated_frames_are_rejected() {
        let (bpp, idx) = pixfmt(VideoFormat::RGBx).expect("RGBx supported");
        let mut out = Vec::new();
        assert!(!convert_into(&[0u8; 8], 4, 2, bpp, idx, 16, &mut out));
    }

    #[test]
    fn every_packed_layout_maps_to_rgb() {
        // One pixel, pure red in the layout's own byte order.
        let cases = [
            (VideoFormat::RGB, [255u8, 0, 0, 0]),
            (VideoFormat::RGBx, [255u8, 0, 0, 0]),
            (VideoFormat::BGR, [0u8, 0, 255, 0]),
            (VideoFormat::BGRx, [0u8, 0, 255, 0]),
        ];
        for (format, src) in cases {
            let (bpp, idx) = pixfmt(format).expect("supported");
            let mut out = Vec::new();
            assert!(convert_into(&src, 1, 1, bpp, idx, bpp, &mut out), "{format:?}");
            assert_eq!(out, vec![255, 0, 0], "{format:?} must convert to red");
        }
    }
}