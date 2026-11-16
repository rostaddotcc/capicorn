//! Single-frame ScreenCast portal capture (Wave 10, track R).
//!
//! Linux only (`#[cfg(target_os = "linux")]` in `lib.rs`): xdg-desktop-portal is
//! the Wayland-native picker, and `capture.rs` reports an honest error elsewhere.
//!
//! `capture_screenshot` uses the *Screenshot* portal, which can only grab whole
//! screens. Snagit's window/monitor modes need the *ScreenCast* portal: it is the
//! only Wayland-native way to let the user pick one specific window or one
//! specific monitor, and it is the same session plumbing `recording.rs` (mp4/gif)
//! and `scrolling/source.rs` (latest-frame) already use.
//!
//! This module is that path reduced to one frame:
//!
//! 1. portal half (async): create a ScreenCast session, `SelectSources` with
//!    [`SourceKind::Window`] (`SourceType::WINDOW`) or [`SourceKind::Monitor`]
//!    (`SourceType::MONITOR`) and `CursorMode::HIDDEN`, `Start` (this is the OS
//!    picker), then `OpenPipeWireRemote`.
//! 2. capture half (own thread — `MainLoopRc` is `!Send`, so it must own its
//!    loop): connect to the fd, negotiate a packed RGB/BGR format, convert the
//!    first frame to `rgb24` with the same stride-aware conversion as
//!    `scrolling/source.rs`, then quit.
//! 3. teardown: the [`Session`] is kept alive until the frame is in hand, then
//!    dropped on the async runtime (dropping it closes the portal session, which
//!    is what stops the cast). PipeWire objects are leaked on purpose — see
//!    [`GREP_DEADLOCK_NOTE`].
//!
//! Every portal D-Bus call is wrapped in a timeout (`portal_call`), so a wedged
//! `xdg-desktop-portal` or a locked screen turns into a readable `Err` instead of
//! a hung command. There is no `unwrap` on any portal result.
//!
//! `GREP_DEADLOCK_NOTE`: destroying PipeWire objects after `mainloop.run()`
//! returned needs a sync roundtrip that nobody services, which deadlocks
//! (`tmp/research/recording.md` §6.5); the thread is about to exit, so leaking
//! them is the safe teardown.

/// How long a pure D-Bus portal round-trip may take before it is called wedged.
const PORTAL_CALL_TIMEOUT: u64 = 15;

/// The OS picker: the user has to look at the window/monitor list, choose one and
/// click "Share".
const PICKER_TIMEOUT: u64 = 120;

/// How long the capture half may wait for the first frame after the picker was
/// answered. Frames normally arrive within a few hundred ms.
const FIRST_FRAME_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Which kind of source the OS picker should offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// One application window (`SourceType::WINDOW`).
    Window,
    /// One monitor — the picker lists every connected output, which is the
    /// multi-monitor path (`SourceType::MONITOR`).
    Monitor,
}

impl SourceKind {
    /// What to call this kind in an error message.
    fn noun(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Monitor => "monitor",
        }
    }
}

/// One captured frame, tightly packed `rgb24`.
pub struct Frame {
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Grab exactly one frame from a user-picked window or monitor.
///
/// Returns once the frame is in memory and the portal session has been closed;
/// on denial, cancel, locked screen or a wedged portal it returns a readable
/// `Err` and leaves nothing running.
#[cfg(target_os = "linux")]
pub async fn grab(kind: SourceKind) -> Result<Frame, String> {
    imp::grab(kind).await
}

// ---------------------------------------------------------------------------
// Linux implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::io::Cursor;
    use std::os::fd::OwnedFd;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::{PersistMode, Session};
    use pipewire as pw;
    use pw::spa;
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::format_utils;
    use spa::param::video::{VideoFormat, VideoInfoRaw};
    use spa::pod::{ChoiceValue, Pod, Property, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id};
    use std::path::PathBuf;

    use super::{SourceKind, FIRST_FRAME_WAIT, PICKER_TIMEOUT, PORTAL_CALL_TIMEOUT};

    // -- pixel formats (same table + conversion as `scrolling/source.rs`) ----

    /// bytes-per-pixel + byte offsets of R, G and B inside one pixel.
    ///
    /// The `x`/`A` variants are what compositors actually advertise for screencast
    /// nodes; getting R/B backwards is a silent colour swap, so every packed
    /// layout is listed.
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

    /// Convert one packed frame into tightly packed `rgb24`, honouring the
    /// producer's row stride (usually `width * bpp`, but not always).
    ///
    /// Returns `false` when the buffer is too small for even one row, so a
    /// truncated buffer can never produce a half-scrambled image.
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

    // -- SPA pods (verbatim from recording.rs: this is the fiddly part) ------

    /// One `EnumFormat` pod per candidate format; the caller keeps the byte
    /// vectors alive because `Pod::from_bytes` only borrows.
    fn make_params(
        width: u32,
        height: u32,
        formats: &[VideoFormat],
    ) -> Result<Vec<Vec<u8>>, String> {
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
                    // A RANGE, not a fixated size: the portal's size hint can be
                    // wrong for an individual window, and a fixated size makes the
                    // producer fail the negotiation (no frames -> "cancelled").
                    Value::Choice(ChoiceValue::Rectangle(Choice(
                        ChoiceFlags::empty(),
                        ChoiceEnum::Range {
                            default: spa::utils::Rectangle { width, height },
                            min: spa::utils::Rectangle {
                                width: 1,
                                height: 1,
                            },
                            max: spa::utils::Rectangle {
                                width: 16384,
                                height: 16384,
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

    /// `SPA_PARAM_Buffers` pod, kept for producers that need an explicit pool.
    /// The CAPTURE stream must NOT send one: KWin rejects a large explicit pool
    /// with "error alloc buffers" — MAP_BUFFERS producer defaults are correct.
    #[allow(dead_code)]
    fn buffers_param(size_per_frame: u32, stride: u32, count: i32) -> Result<Vec<u8>, String> {
        let properties = vec![
            Property::new(spa::sys::SPA_PARAM_BUFFERS_buffers, Value::Int(count)),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_size,
                Value::Int(size_per_frame as i32),
            ),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_stride,
                Value::Int(stride as i32),
            ),
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

    // -- portal half --------------------------------------------------------

    /// Await one portal call, turning a hang into a readable error.
    ///
    /// Every call is wrapped: a wedged `xdg-desktop-portal` (or a locked screen)
    /// would otherwise hang the request forever instead of failing the command.
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

    /// Append a timestamped line to the shared portal log, so "Copy recording
    /// diagnostics" covers window/monitor capture as well as the recorder.
    fn clog(msg: &str) {
        use std::io::Write;
        let path = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".cache").join("snagcraft-portal.log"))
            .unwrap_or_else(|_| std::env::temp_dir().join("snagcraft-portal.log"));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let _ = writeln!(f, "[{ts}] {msg}");
        }
    }

    struct PortalCast {
        fd: OwnedFd,
        node: u32,
        size: (i32, i32),
        session: Session<Screencast>,
    }

    /// Create the session, let the user pick a window or a monitor, start the
    /// cast and open the PipeWire remote.
    ///
    /// The returned session must outlive the frame grab: dropping it closes the
    /// portal session, which is what ends the cast.
    async fn open_portal(kind: SourceKind) -> Result<PortalCast, String> {
        let sources = match kind {
            SourceKind::Window => SourceType::WINDOW,
            SourceKind::Monitor => SourceType::MONITOR,
        };
        clog(&format!(
            "{} capture: opening the ScreenCast portal",
            kind.noun()
        ));

        let proxy =
            portal_call(PORTAL_CALL_TIMEOUT, "ScreenCast portal", Screencast::new()).await?;

        let session = portal_call(
            PORTAL_CALL_TIMEOUT,
            "CreateSession",
            proxy.create_session(Default::default()),
        )
        .await?;

        // `select_sources` hands back an empty `Request`; ashpd's own example
        // drops it (the portal answers on its well-known name). Do NOT
        // `.response()` it.
        let selection = portal_call(
            PORTAL_CALL_TIMEOUT,
            "SelectSources",
            proxy.select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(sources)
                    // HIDDEN, not EMBEDDED: a cursor burned into a still frame is
                    // almost never wanted (Snagit's window/monitor modes omit it
                    // too).
                    .set_cursor_mode(CursorMode::HIDDEN)
                    .set_multiple(false)
                    // PersistMode lives in `ashpd::desktop`, not `screencast`.
                    .set_persist_mode(PersistMode::DoNot),
            ),
        )
        .await?;
        drop(selection);

        // `start` opens the OS picker; it only resolves once the user clicks
        // "Share" (or the dialog is dismissed with an error).
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
            Ok(Err(err)) => {
                return Err(format!(
                    "{} capture was cancelled or refused: {err}",
                    kind.noun()
                ))
            }
            Ok(Ok(streams)) => streams,
        };

        let stream = streams.streams().first().ok_or_else(|| {
            format!(
                "the portal returned no streams (this compositor may not support {} capture)",
                kind.noun()
            )
        })?;
        let node = stream.pipe_wire_node_id();
        // Only a hint for format enumeration; `param_changed` has the truth.
        let size = stream.size().unwrap_or((1920, 1080));
        clog(&format!(
            "{} capture: picker answered node={} size={}x{}",
            kind.noun(),
            node,
            size.0,
            size.1
        ));

        let fd = portal_call(
            PORTAL_CALL_TIMEOUT,
            "OpenPipeWireRemote",
            proxy.open_pipe_wire_remote(&session, Default::default()),
        )
        .await?;

        Ok(PortalCast {
            fd,
            node,
            size,
            session,
        })
    }

    // -- capture half -------------------------------------------------------

    struct Ud {
        fmt: VideoInfoRaw,
        slot: Rc<RefCell<Option<super::Frame>>>,
        stop: Arc<AtomicBool>,
        loopweak: pw::main_loop::MainLoopWeak,
        buf: Vec<u8>,
        got: Rc<Cell<bool>>,
    }

    fn quit(ud: &Ud) {
        if let Some(mainloop) = ud.loopweak.upgrade() {
            mainloop.quit();
        }
    }

    /// Connect to the portal fd and run the loop until the first frame has been
    /// converted into `slot`, the stop flag is set, or the compositor closed the
    /// cast. Always returns with the loop serviced and the frame (if any) stored.
    fn capture_first(
        fd: OwnedFd,
        node: u32,
        size: (i32, i32),
        slot: Rc<RefCell<Option<super::Frame>>>,
        stop: Arc<AtomicBool>,
        name: &'static str,
    ) -> Result<(), String> {
        clog(&format!(
            "{name}: capture start — node={node} size={}x{}",
            size.0, size.1
        ));
        pw::init();

        let mainloop = pw::main_loop::MainLoopRc::new(None)
            .map_err(|e| format!("cannot create the PipeWire main loop: {e}"))?;
        let context = pw::context::ContextRc::new(&mainloop, None)
            .map_err(|e| format!("cannot create the PipeWire context: {e}"))?;
        let core = context
            .connect_fd_rc(fd, None)
            .map_err(|e| format!("cannot connect to the portal PipeWire fd: {e}"))?;

        let props = pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        };
        // The target node is passed to `stream.connect` below, NOT as a
        // `target.object` prop (which resolves too early on a fresh connection —
        // the same "no target node available" trap the recorder hit).

        let stream = pw::stream::StreamBox::new(&core, name, props)
            .map_err(|e| format!("cannot create the PipeWire capture stream: {e}"))?;

        let got = Rc::new(Cell::new(false));
        let ud = Ud {
            fmt: VideoInfoRaw::new(),
            slot: slot.clone(),
            stop: stop.clone(),
            loopweak: mainloop.downgrade(),
            buf: Vec::new(),
            got: got.clone(),
        };

        let listener = stream
            .add_local_listener_with_user_data(ud)
            .param_changed(move |_, ud, id, param| {
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
                    clog(&format!(
                        "{name}: format negotiated {:?} {}x{}",
                        ud.fmt.format(),
                        size.width,
                        size.height
                    ));
                }
            })
            .state_changed(move |_, _, old, new| {
                if old != new {
                    clog(&format!("capture stream {old:?} -> {new:?}"));
                }
            })
            .process(move |stream, ud| {
                // Stop is polled here as well as from the timer, so an abandoned
                // grab lands on the very next frame instead of waiting.
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
                    // No packed RGB/BGR: nothing to convert — fail loudly instead
                    // of looping until the timeout.
                    eprintln!(
                        "snagcraft: unsupported capture pixel format {:?}",
                        ud.fmt.format()
                    );
                    ud.stop.store(true, Ordering::Relaxed);
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

                // Buffers can be smaller than the frame (8 KiB default pool):
                // never index past what is actually mapped.
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

                // One frame is the whole job: publish it and leave the loop.
                let frame = super::Frame {
                    rgb: std::mem::take(&mut ud.buf),
                    width: width as u32,
                    height: height as u32,
                };
                match ud.slot.try_borrow_mut() {
                    Ok(mut slot) => *slot = Some(frame),
                    Err(_) => return,
                }
                ud.got.set(true);
                clog(&format!("{name}: first frame captured {width}x{height}"));
                quit(ud);
            })
            .register()
            .map_err(|e| format!("cannot register the capture callbacks: {e}"))?;

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
        )?;
        // No explicit SPA_PARAM_Buffers: a large explicit pool made KWin reject
        // the port ("error alloc buffers"); MAP_BUFFERS producer defaults are correct.
        let mut params: Vec<&Pod> = pods.iter().filter_map(|v| Pod::from_bytes(v)).collect();

        // Wait for the portal node to appear in this connection's registry before
        // targeting it — connecting to an unknown node fails the same way.
        if let Ok(registry) = core.get_registry() {
            let seen = Rc::new(Cell::new(false));
            let seen_flag = seen.clone();
            let _listener = registry
                .add_listener_local()
                .global(move |g| {
                    if g.id == node {
                        seen_flag.set(true);
                    }
                })
                .register();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !seen.get() && std::time::Instant::now() < deadline {
                let _ = mainloop.loop_().iterate(pw::loop_::Timeout::Finite(
                    std::time::Duration::from_millis(20),
                ));
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
            .map_err(|e| format!("cannot connect the capture stream: {e}"))?;

        // A static window produces no buffers after the first one, and a grab
        // that never gets a frame must still unblock the caller: poll the stop
        // flag from the loop itself.
        let tick_weak = mainloop.downgrade();
        let tick_stop = stop.clone();
        let tick = mainloop.loop_().add_timer(move |_| {
            if tick_stop.load(Ordering::Relaxed) {
                if let Some(mainloop) = tick_weak.upgrade() {
                    mainloop.quit();
                }
            }
        });
        let _ = tick.update_timer(
            Some(Duration::from_millis(100)),
            Some(Duration::from_millis(100)),
        );

        mainloop.run();

        // Destroying PipeWire objects after the loop returned needs a sync
        // roundtrip that nobody services -> deadlock (`recording.md` §6.5). This
        // thread is about to exit, so leaking them is the safe teardown.
        std::mem::forget(tick);
        std::mem::forget(listener);
        std::mem::forget(stream);
        std::mem::forget(mainloop);

        if got.get() {
            Ok(())
        } else {
            Err("the capture stream produced no frame".to_string())
        }
    }

    /// Portal half + capture thread + session teardown for one still frame.
    pub(super) async fn grab(kind: SourceKind) -> Result<super::Frame, String> {
        let portal = open_portal(kind).await?;
        let name = match kind {
            SourceKind::Window => "snagcraft-window-capture",
            SourceKind::Monitor => "snagcraft-monitor-capture",
        };

        // `MainLoopRc`/`Rc` are `!Send`, so the capture slots must be created
        // inside the capture thread; only the finished `Frame` (a `Vec<u8>`) is
        // handed back, through a channel that is bounded so a late frame after a
        // timeout never blocks the thread.
        let (tx, rx) = mpsc::sync_channel::<Result<super::Frame, String>>(1);
        let stop = Arc::new(AtomicBool::new(false));

        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    let slot: Rc<RefCell<Option<super::Frame>>> = Rc::new(RefCell::new(None));
                    let result = capture_first(
                        portal.fd,
                        portal.node,
                        portal.size,
                        slot.clone(),
                        stop,
                        name,
                    )
                    .and_then(|()| {
                        slot.try_borrow_mut()
                            .ok()
                            .and_then(|mut guard| guard.take())
                            .ok_or_else(|| "the capture stream produced no frame".to_string())
                    });
                    if let Err(err) = &result {
                        eprintln!("snagcraft: {err}");
                    }
                    let _ = tx.try_send(result);
                })
                .map_err(|e| format!("cannot start the capture thread: {e}"))?
        };

        // Wait for the frame off the async runtime: the picker already happened,
        // so this only covers "first buffers never arrived".
        let received =
            tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(FIRST_FRAME_WAIT))
                .await
                .map_err(|e| format!("waiting for the captured frame failed: {e}"))?;

        // Stop + join before the session goes away, so no callback can fire
        // against a closing PipeWire connection.
        stop.store(true, Ordering::Relaxed);
        if tauri::async_runtime::spawn_blocking(move || {
            let _ = thread.join();
        })
        .await
        .is_err()
        {
            eprintln!("snagcraft: the capture thread did not join cleanly");
        }

        // Only now is it safe to end the portal session: dropping it closes the
        // cast on the portal side.
        drop(portal.session);

        match received {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(err)) => Err(err),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!(
                "the {} capture pipeline could not be started — see the snagcraft log",
                kind.noun()
            )),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "the {} capture produced no frame within {}s — was the share cancelled?",
                kind.noun(),
                FIRST_FRAME_WAIT.as_secs()
            )),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn bgrx_converts_without_a_red_blue_swap() {
            // Two pixels as B,G,R,x in memory: pure red, then pure blue.
            let src = [0u8, 0, 255, 255, 255, 0, 0, 255];
            let (bpp, idx) = pixfmt(VideoFormat::BGRx).expect("BGRx supported");
            let mut out = Vec::new();
            assert!(convert_into(&src, 2, 1, bpp, idx, 2 * bpp, &mut out));
            // rgb24 keeps R,G,B order: red first, blue second — a swap flips this.
            assert_eq!(out, vec![255, 0, 0, 0, 0, 255]);
        }

        #[test]
        fn padded_stride_is_honoured() {
            // width 1, stride 8 = one row plus 4 bytes of padding.
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
        fn a_full_row_maps_to_rgb24_row() {
            // 3x2 BGRx with a stride wider than the row.
            let mut src = vec![0u8; 2 * 16];
            for y in 0..2 {
                for x in 0..3 {
                    let off = y * 16 + x * 4;
                    src[off] = (x * 10) as u8; // B
                    src[off + 1] = (y * 20) as u8; // G
                    src[off + 2] = 200; // R
                }
            }
            let (bpp, idx) = pixfmt(VideoFormat::BGRx).expect("BGRx supported");
            let mut out = Vec::new();
            assert!(convert_into(&src, 3, 2, bpp, idx, 16, &mut out));
            assert_eq!(out.len(), 3 * 2 * 3);
            assert_eq!(&out[0..3], &[200, 0, 0]);
            assert_eq!(&out[9..12], &[200, 20, 0]);
        }
    }
}
