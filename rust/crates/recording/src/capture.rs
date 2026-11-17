//! The capture half — one dedicated thread, one PipeWire main loop.
//!
//! `MainLoopRc`/`ContextRc` are `!Send`, so everything here runs on the thread
//! that [`run`] is called on. The thread ends when the take ends; the caller
//! joins it and the objects are deliberately leaked (see the teardown note at the
//! bottom of [`run`]).
//!
//! The traps this module encodes, each of which cost real debugging time:
//! * **no `target.object` property** and **no `SPA_PARAM_Buffers` on the
//!   consumer**: both make the portal node resolve/allocate at stream creation,
//!   before the registry has synced, and the stream dies with
//!   `no target node available` / `error alloc buffers: Invalid argument`;
//! * **wait for the node id in the registry** before connecting;
//! * **`VideoSize` as a range**, never a fixated size, or a window that does not
//!   match the hint negotiates nothing;
//! * **BGRx → rgb24 is stride-aware** and must not swap R and B.

use std::cell::{Cell, RefCell};
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::{ChoiceValue, Pod, Property, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id};

use crate::crop::{MAX_DIM, crop_dims, crop_frame_rgb24};
use crate::ffmpeg::{Encoder, EncoderJob};
use crate::{
    FIRST_FRAME_WAIT, RecordingConfig, RecordingError, RecordingEvent, RecordingFormat, Region,
};

/// Where the capture stream should connect.
pub(crate) enum Endpoint {
    /// The fd returned by the portal's `OpenPipeWireRemote`.
    PortalFd(OwnedFd),
    /// The default PipeWire daemon. Used by the offline probe, where a local
    /// `Video/Source` is the producer instead of the portal.
    #[cfg_attr(not(test), allow(dead_code))]
    Daemon,
}

/// Everything one capture thread needs (a struct, so the signature stays sane).
pub(crate) struct CaptureParams {
    pub format: RecordingFormat,
    pub region: Option<Region>,
    pub fps: u32,
    /// A `pactl` source id; `None` records a silent take.
    pub audio: Option<String>,
    pub out: PathBuf,
    pub events: mpsc::Sender<RecordingEvent>,
    pub stop: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
}

impl CaptureParams {
    /// The parts of a caller config that matter to the capture half.
    pub(crate) fn from_config(
        config: &RecordingConfig,
        events: mpsc::Sender<RecordingEvent>,
        stop: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
    ) -> Self {
        Self {
            format: config.format,
            region: config.region,
            fps: config.effective_fps(),
            audio: config.audio.source_id().map(str::to_string),
            out: config.output.clone(),
            events,
            stop,
            paused,
        }
    }
}

/// bytes per pixel + byte offsets of R, G and B inside one pixel.
///
/// The `x`/`A` padding variants are what KWin's screencast node advertises, and
/// getting the R/B order wrong is a silent colour swap, so every packed layout a
/// compositor offers is listed.
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

/// Convert one packed frame to tightly-packed `rgb24`, honouring the producer's
/// row stride (usually `width * bpp`, not always).
///
/// Returns `false` when the buffer cannot cover even one full row — the check
/// that turns a garbage buffer into a logged "no frames" instead of a panic or a
/// sheared take.
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
    out.resize(width * height * 3, 0);

    for y in 0..height {
        let start = y * stride;
        if start + row_bytes > src.len() {
            return false;
        }
        let row = &src[start..start + row_bytes];
        let dst = &mut out[y * width * 3..(y + 1) * width * 3];
        for x in 0..width {
            let pixel = &row[x * bpp..x * bpp + bpp];
            dst[x * 3] = pixel[idx[0]];
            dst[x * 3 + 1] = pixel[idx[1]];
            dst[x * 3 + 2] = pixel[idx[2]];
        }
    }
    true
}

/// The `VideoSize` property of an `EnumFormat` pod: a RANGE with the hint as
/// default, never a fixated size.
///
/// A fixated size works for a monitor (the portal hint is exact) but fails for an
/// individual window, where the hint rarely matches: the producer cannot fulfil
/// it, no format negotiates, and the take dies as "cancelled".
fn video_size_property(width: u32, height: u32) -> Property {
    Property::new(
        FormatProperties::VideoSize.as_raw(),
        Value::Choice(ChoiceValue::Rectangle(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range {
                default: spa::utils::Rectangle { width, height },
                min: spa::utils::Rectangle {
                    width: 1,
                    height: 1,
                },
                max: spa::utils::Rectangle {
                    width: MAX_DIM,
                    height: MAX_DIM,
                },
            },
        ))),
    )
}

/// One `EnumFormat` pod per candidate format. The caller keeps the `Vec<Vec<u8>>`
/// alive: `Pod::from_bytes` only borrows.
fn make_params(
    width: u32,
    height: u32,
    formats: &[VideoFormat],
) -> Result<Vec<Vec<u8>>, RecordingError> {
    let mut out = Vec::new();
    for format in formats {
        // libspa has no `Into<Vec<Property>>` for `VideoInfoRaw`, so the pod is
        // assembled by hand.
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
            video_size_property(width, height),
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
        .map_err(|error| {
            RecordingError::PipeWire(format!("cannot serialise the video format pod: {error:?}"))
        })?
        .0
        .into_inner();
        out.push(bytes);
    }
    Ok(out)
}

/// Per-take diagnostics next to the recording, so a failed take can be read
/// afterwards (~/.cache is only reached when the log cannot be written).
fn rec_log(path: &Path, message: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "[{stamp}] {message}");
    }
}

/// User data shared with the PipeWire callbacks.
struct Ud {
    fmt: VideoInfoRaw,
    encoder: Rc<RefCell<Option<Encoder>>>,
    fatal: Rc<RefCell<Option<RecordingError>>>,
    frames: Rc<Cell<u64>>,
    spawned: Rc<Cell<bool>>,
    events: mpsc::Sender<RecordingEvent>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    loopweak: pw::main_loop::MainLoopWeak,
    out: PathBuf,
    log: PathBuf,
    format: RecordingFormat,
    fps: u32,
    audio: Option<String>,
    region: Option<Region>,
    /// Size ffmpeg was told via `-s` (already even-padded for a region).
    out_size: (u32, u32),
    /// Full-frame rgb24 from [`convert_into`].
    buf: Vec<u8>,
    /// Reused scratch for the cropped (and padded) frame.
    cropped: Vec<u8>,
    /// True once the producer's size stopped matching the encoder input (a window
    /// that got resized); logged once per episode.
    mismatched: bool,
}

fn quit(ud: &Ud) {
    if let Some(mainloop) = ud.loopweak.upgrade() {
        mainloop.quit();
    }
}

/// Record the first fatal error and stop the loop; later errors are ignored so
/// the first cause is the one reported.
fn fail(ud: &Ud, error: RecordingError) {
    {
        let mut slot = ud.fatal.borrow_mut();
        if slot.is_none() {
            tracing::error!(%error, "recording capture failed");
            *slot = Some(error);
        }
    }
    quit(ud);
}

/// Spawn ffmpeg on the first frame — size and rate are only trustworthy once
/// `VideoInfoRaw` has been parsed, not from `Stream::size()`.
fn ensure_encoder(ud: &mut Ud) -> bool {
    let size = ud.fmt.size();
    let format = ud.fmt.format();
    if pixfmt(format).is_none() {
        fail(
            ud,
            RecordingError::PipeWire(format!(
                "unsupported capture pixel format {format:?} — this compositor offers no packed RGB/BGR mode"
            )),
        );
        return false;
    }

    // ffmpeg must be told the size of what we actually write: the padded region
    // when one is selected, the full frame otherwise.
    let (out_w, out_h) = match ud.region {
        Some(region) => {
            let dims = crop_dims(region);
            if dims.0 == 0 || dims.1 == 0 {
                fail(
                    ud,
                    RecordingError::Unsupported(format!(
                        "the recording region {}x{}+{}+{} has no area",
                        region.width, region.height, region.left, region.top
                    )),
                );
                return false;
            }
            (dims.0 as u32, dims.1 as u32)
        }
        None => (size.width, size.height),
    };

    match Encoder::spawn(EncoderJob {
        out: &ud.out,
        format: ud.format,
        width: out_w,
        height: out_h,
        fps: ud.fps,
        audio: ud.audio.as_deref(),
    }) {
        Ok(encoder) => {
            *ud.encoder.borrow_mut() = Some(encoder);
            ud.out_size = (out_w, out_h);
            ud.spawned.set(true);
            rec_log(
                &ud.log,
                &format!("encoder spawned for {out_w}x{out_h} @ {} fps", ud.fps),
            );
            let _ = ud.events.send(RecordingEvent::Started {
                width: out_w,
                height: out_h,
            });
            true
        }
        Err(error) => {
            fail(ud, error);
            false
        }
    }
}

/// The whole capture half for one take. Returns the number of frames written.
pub(crate) fn run(
    endpoint: Endpoint,
    node: u32,
    hint: (i32, i32),
    params: CaptureParams,
) -> Result<u64, RecordingError> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|error| {
        RecordingError::PipeWire(format!("cannot create the main loop: {error}"))
    })?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|error| RecordingError::PipeWire(format!("cannot create the context: {error}")))?;
    let core = match endpoint {
        Endpoint::PortalFd(fd) => context.connect_fd_rc(fd, None).map_err(|error| {
            RecordingError::PipeWire(format!(
                "cannot connect to the portal's PipeWire fd: {error}"
            ))
        })?,
        Endpoint::Daemon => context.connect_rc(None).map_err(|error| {
            RecordingError::PipeWire(format!("cannot connect to the PipeWire daemon: {error}"))
        })?,
    };

    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    // NOTE: no `target.object` property here on purpose — the node id is passed
    // to `stream.connect` after the registry wait below.

    let stream =
        pw::stream::StreamBox::new(&core, "snagcraft-recording", props).map_err(|error| {
            RecordingError::PipeWire(format!("cannot create the capture stream: {error}"))
        })?;

    let log_path = {
        let mut path = params.out.clone().into_os_string();
        path.push(".rec.log");
        PathBuf::from(path)
    };
    rec_log(
        &log_path,
        &format!(
            "capture start: node={node} hint={}x{} region={:?} fps={} audio={}",
            hint.0,
            hint.1,
            params.region,
            params.fps,
            params.audio.as_deref().unwrap_or("none")
        ),
    );

    let encoder: Rc<RefCell<Option<Encoder>>> = Rc::new(RefCell::new(None));
    let fatal: Rc<RefCell<Option<RecordingError>>> = Rc::new(RefCell::new(None));
    let frames = Rc::new(Cell::new(0u64));
    let spawned = Rc::new(Cell::new(false));
    let started_at = Instant::now();

    let ud = Ud {
        fmt: VideoInfoRaw::new(),
        encoder: encoder.clone(),
        fatal: fatal.clone(),
        frames: frames.clone(),
        spawned: spawned.clone(),
        events: params.events.clone(),
        stop: params.stop.clone(),
        paused: params.paused.clone(),
        loopweak: mainloop.downgrade(),
        out: params.out.clone(),
        log: log_path.clone(),
        format: params.format,
        fps: params.fps,
        audio: params.audio.clone(),
        region: params.region,
        out_size: (0, 0),
        buf: Vec::new(),
        cropped: Vec::new(),
        mismatched: false,
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
                let rate = ud.fmt.framerate();
                rec_log(
                    &ud.log,
                    &format!(
                        "format negotiated {:?} {}x{} @ {}/{}",
                        ud.fmt.format(),
                        size.width,
                        size.height,
                        rate.num,
                        rate.denom
                    ),
                );
            }
        })
        .state_changed(|_, ud, old, new| {
            if old != new {
                rec_log(&ud.log, &format!("stream state {old:?} -> {new:?}"));
            }
        })
        .process(|stream, ud| {
            // Polled here as well as from the timer: a stop lands on the very next
            // frame instead of waiting for the next tick.
            if ud.stop.load(Ordering::Relaxed) {
                quit(ud);
                return;
            }

            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            if buffer.datas_mut().is_empty() {
                return;
            }

            // Paused: the buffer is dequeued and dropped (never written) so the
            // stream, the portal session and the encoder all stay alive and
            // resuming continues the same file. The paused interval simply has no
            // frames — no re-timing, no black filler.
            if ud.paused.load(Ordering::Relaxed) {
                return;
            }

            if !ud.spawned.get() && !ensure_encoder(ud) {
                return;
            }

            let size = ud.fmt.size();
            let (width, height) = (size.width as usize, size.height as usize);
            if width == 0 || height == 0 {
                return;
            }

            // A window can be resized mid-take and the rawvideo input has a fixed
            // `-s`; a differently sized frame would shear the rest of the file, so
            // those frames are dropped until the size comes back. Region takes are
            // immune — the crop is sized by the region, not the frame.
            if ud.region.is_none() && ud.out_size != (size.width, size.height) {
                if !ud.mismatched {
                    rec_log(
                        &ud.log,
                        &format!(
                            "frame size {}x{} no longer matches the {}x{} encoder input — dropping frames",
                            size.width, size.height, ud.out_size.0, ud.out_size.1
                        ),
                    );
                    ud.mismatched = true;
                }
                return;
            }
            ud.mismatched = false;

            let Some((bpp, idx)) = pixfmt(ud.fmt.format()) else {
                return;
            };

            let (offset, chunk_size, stride) = {
                let datas = buffer.datas_mut();
                let data = &mut datas[0];
                let chunk = data.chunk();
                (
                    chunk.offset() as usize,
                    chunk.size() as usize,
                    chunk.stride(),
                )
            };
            let datas = buffer.datas_mut();
            let Some(raw) = datas[0].data() else { return };

            // Buffers can be smaller than the frame (a default 8 KiB pool): never
            // index past what is actually mapped.
            if offset.saturating_add(chunk_size) > raw.len() || chunk_size < width * bpp {
                return;
            }
            let src = &raw[offset..offset + chunk_size];
            let stride = if stride > 0 {
                stride as usize
            } else {
                width * bpp
            };

            if !convert_into(src, width, height, bpp, idx, stride, &mut ud.buf) {
                rec_log(&ud.log, "dropped a frame that did not fit the mapped buffer");
                return;
            }

            // Crop to the selected rectangle *before* anything reaches the
            // encoder. The padded size is the `-s` ffmpeg was spawned with, so
            // every frame fits exactly.
            let frame: &[u8] = match ud.region {
                Some(region) => {
                    let cropped =
                        crop_frame_rgb24(&ud.buf, size.width, size.height, region);
                    ud.cropped = cropped;
                    &ud.cropped
                }
                None => &ud.buf,
            };

            let wrote = {
                let mut slot = ud.encoder.borrow_mut();
                match slot.as_mut() {
                    // EPIPE means ffmpeg died: stop instead of spinning.
                    Some(encoder) => encoder.write_frame(frame),
                    None => false,
                }
            };
            if !wrote {
                fail(
                    ud,
                    RecordingError::Ffmpeg(
                        "the encoder stopped accepting frames (ffmpeg exited mid-take)"
                            .to_string(),
                    ),
                );
                return;
            }

            let count = ud.frames.get() + 1;
            ud.frames.set(count);
            if count == 1 {
                rec_log(&ud.log, "first frame written to ffmpeg");
            }
        })
        .register()
        .map_err(|error| {
            RecordingError::PipeWire(format!("cannot register the capture callbacks: {error}"))
        })?;

    let hint_w = hint.0.clamp(1, MAX_DIM as i32) as u32;
    let hint_h = hint.1.clamp(1, MAX_DIM as i32) as u32;
    let pods = make_params(
        hint_w,
        hint_h,
        &[
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::BGR,
            VideoFormat::RGB,
        ],
    )?;
    // No `SPA_PARAM_Buffers` here: requesting a large explicit pool makes KWin's
    // portal node reject the port with "error alloc buffers: Invalid argument".
    // With MAP_BUFFERS the producer allocates its own correct full-size buffers.
    let mut format_pods: Vec<&Pod> = pods.iter().filter_map(|pod| Pod::from_bytes(pod)).collect();

    // Sync this connection's registry and wait until the portal's node is
    // visible before targeting it: `connect_fd` can return before the registry
    // globals arrive, and a stream that targets an unknown node fails with
    // "no target node available".
    if let Ok(registry) = core.get_registry() {
        let seen = Rc::new(Cell::new(false));
        let seen_flag = seen.clone();
        let _registry_listener = registry
            .add_listener_local()
            .global(move |global| {
                if global.id == node {
                    seen_flag.set(true);
                }
            })
            .register();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !seen.get() && Instant::now() < deadline {
            let _ = mainloop
                .loop_()
                .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(20)));
        }
        rec_log(
            &log_path,
            &format!(
                "target node {node} {}",
                if seen.get() {
                    "visible in registry"
                } else {
                    "NOT seen in registry after 2s"
                }
            ),
        );
    }

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut format_pods,
        )
        .map_err(|error| {
            RecordingError::PipeWire(format!("cannot connect the capture stream: {error}"))
        })?;
    rec_log(&log_path, "stream connected; entering the main loop");

    // Poll from the loop: on a static screen no buffers arrive, so a stop that
    // only lived in `process()` could never land. The same tick enforces the
    // first-frame deadline.
    let tick_weak = mainloop.downgrade();
    let tick_stop = params.stop.clone();
    let tick_fatal = fatal.clone();
    let tick_spawned = spawned.clone();
    let tick_log = log_path.clone();
    let tick = mainloop.loop_().add_timer(move |_| {
        if tick_stop.load(Ordering::Relaxed) {
            if let Some(mainloop) = tick_weak.upgrade() {
                mainloop.quit();
            }
            return;
        }
        if !tick_spawned.get() && started_at.elapsed() > FIRST_FRAME_WAIT {
            let message = format!(
                "no frames arrived within {}s — the screen cast was probably cancelled",
                FIRST_FRAME_WAIT.as_secs()
            );
            rec_log(&tick_log, &message);
            {
                let mut slot = tick_fatal.borrow_mut();
                if slot.is_none() {
                    *slot = Some(RecordingError::PipeWire(message));
                }
            }
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

    // Everything below runs after the loop stopped: no PipeWire sync here.
    let finished = encoder.borrow_mut().take().map(Encoder::finish).transpose();

    let captured = frames.get();
    rec_log(&log_path, &format!("capture ended: {captured} frames"));
    tracing::info!(frames = captured, "recording capture ended");

    // Dropping PipeWire objects after `mainloop.run()` returns needs a sync
    // roundtrip that nobody services -> deadlock. That includes the context and
    // the core (destroying a context stops its internal threads and waits for
    // them, which hangs behind a blocking D-Bus call). This thread is about to
    // exit, so leaking them is the safe teardown (the OS reclaims it all).
    std::mem::forget(tick);
    std::mem::forget(listener);
    std::mem::forget(stream);
    std::mem::forget(mainloop);
    std::mem::forget(core);
    std::mem::forget(context);

    if let Some(error) = fatal.borrow_mut().take() {
        return Err(error);
    }
    finished?;
    if captured == 0 {
        return Err(RecordingError::PipeWire(
            "the capture stream produced no frames".to_string(),
        ));
    }
    Ok(captured)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_frame(width: usize, height: usize) -> Vec<u8> {
        // BGRx: (b, g, r, x) per pixel.
        let mut frame = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) * 4;
                frame[i] = 10;
                frame[i + 1] = 20;
                frame[i + 2] = 30;
                frame[i + 3] = 255;
            }
        }
        frame
    }

    #[test]
    fn bgrx_converts_without_a_red_blue_swap() {
        // Two pixels: pure red (B=0,G=0,R=255), then pure blue (B=255,G=0,R=0).
        let src = [0u8, 0, 255, 255, 255, 0, 0, 255];
        let mut out = Vec::new();
        assert!(convert_into(&src, 2, 1, 4, [2, 1, 0], 8, &mut out));
        assert_eq!(out, vec![255, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn padded_stride_is_honoured() {
        // 2 pixels per row, 12-byte stride (4 bytes of padding per row).
        let mut src = vec![0xEEu8; 24];
        src[0..8].copy_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]);
        src[12..20].copy_from_slice(&[7, 8, 9, 0, 10, 11, 12, 0]);
        let mut out = Vec::new();
        assert!(convert_into(&src, 2, 2, 4, [2, 1, 0], 12, &mut out));
        assert_eq!(out, vec![3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]);
    }

    #[test]
    fn a_truncated_frame_is_rejected_instead_of_read_out_of_bounds() {
        let mut out = Vec::new();
        assert!(!convert_into(&[0u8; 4], 2, 2, 4, [2, 1, 0], 8, &mut out));
        assert!(!convert_into(&[], 0, 0, 4, [2, 1, 0], 0, &mut out));
        // Stride smaller than a row can never be right.
        assert!(!convert_into(&[0u8; 32], 2, 2, 4, [2, 1, 0], 4, &mut out));
    }

    #[test]
    fn every_packed_layout_maps_to_its_channel_offsets() {
        assert_eq!(pixfmt(VideoFormat::BGRx), Some((4, [2, 1, 0])));
        assert_eq!(pixfmt(VideoFormat::RGBx), Some((4, [0, 1, 2])));
        assert_eq!(pixfmt(VideoFormat::xBGR), Some((4, [3, 2, 1])));
        assert_eq!(pixfmt(VideoFormat::BGR), Some((3, [2, 1, 0])));
        assert_eq!(pixfmt(VideoFormat::RGB), Some((3, [0, 1, 2])));
        assert_eq!(pixfmt(VideoFormat::YUY2), None);
    }

    #[test]
    fn a_full_bgrx_frame_round_trips_through_the_converter() {
        let (w, h) = (3usize, 2usize);
        let src = sample_frame(w, h);
        let mut out = Vec::new();
        assert!(convert_into(&src, w, h, 4, [2, 1, 0], w * 4, &mut out));
        assert_eq!(out.len(), w * h * 3);
        assert_eq!(&out[0..3], &[30, 20, 10]);
    }

    #[test]
    fn the_negotiated_format_offers_a_size_range() {
        let property = video_size_property(1280, 720);
        assert_eq!(property.key, FormatProperties::VideoSize.as_raw());
        match &property.value {
            Value::Choice(ChoiceValue::Rectangle(Choice(
                _,
                ChoiceEnum::Range { default, min, max },
            ))) => {
                assert_eq!((default.width, default.height), (1280, 720));
                assert_eq!((min.width, min.height), (1, 1));
                assert_eq!((max.width, max.height), (MAX_DIM, MAX_DIM));
            }
            other => panic!("VideoSize must be a range choice, got {other:?}"),
        }
    }

    #[test]
    fn the_format_pods_serialise_for_every_offered_packing() {
        let pods = make_params(1280, 720, &[VideoFormat::BGRx, VideoFormat::RGB]).expect("pods");
        assert_eq!(pods.len(), 2);
        for pod in &pods {
            assert!(pod.len() > 16, "a format pod is not empty");
            assert!(Pod::from_bytes(pod).is_some(), "pods must round-trip");
        }
    }
}
