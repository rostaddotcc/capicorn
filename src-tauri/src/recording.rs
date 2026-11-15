//! Screen recording (Wave 8) — Linux: ScreenCast portal → PipeWire → ffmpeg.
//!
//! The approach and every API/gotcha here come from `tmp/research/recording.md`
//! and the proven probe at `~/.hermes/cache/scratch/rec-probe`: the capture half
//! (PipeWire consumer → stride-aware BGRx/RGBx → rgb24 → ffmpeg stdin) produced a
//! real 90-frame h264 MP4, so it is ported rather than reinvented.
//!
//! Two halves that must not share a thread:
//!
//! 1. **Portal half** (async, on the Tauri tokio runtime): [`start_portal`] creates
//!    a ScreenCast session, selects the monitor, starts it (this is the dialog the
//!    user clicks), then hands back the PipeWire remote fd + node id + size. The
//!    `Session` is stored in [`RecordingState`] for the whole recording: dropping it
//!    closes the portal session and the stream dies with it.
//! 2. **Capture half** (dedicated thread, `MainLoopRc` — `!Send`, so it must own its
//!    loop): connects to the portal fd, negotiates a packed RGB/BGR format, converts
//!    each frame to `rgb24` and writes it into a lazily-spawned ffmpeg child.
//!
//! Commands (registered in `lib.rs`, state managed there too):
//!
//! | command           | args (camelCase)             | returns               |
//! |-------------------|------------------------------|-----------------------|
//! | `start_recording` | `format` (`mp4`/`gif`), `dir`, `filename`, `region` (optional) | `void` — resolves once frames flow |
//! | `stop_recording`  | —                            | output path (`string`) |
//! | `is_recording`    | —                            | `bool`                |
//!
//! `region` selects a rectangle of the monitor ({left,top,width,height} in
//! stream pixels). Every captured frame is cropped to it before it reaches
//! ffmpeg, and ffmpeg is sized to the (even-padded) region, so the take is the
//! selected rectangle and nothing else. Omitted/`null` keeps the whole monitor.
//!
//! `recording-state` events (`{ recording: bool, path: string|null }`) are emitted on
//! start and stop so the UI can resync if it missed an invoke result.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

/// Rectangle of the captured stream to keep, in stream pixels.
///
/// `left`/`top` are signed so a selection dragged partly off-screen still
/// arrives intact; the crop clamps whatever falls outside the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// An empty rectangle would make ffmpeg choke; that is a frontend bug.
    fn validate(self) -> Result<Self, String> {
        if self.width == 0 || self.height == 0 {
            return Err("the recording region has no area".to_string());
        }
        Ok(self)
    }
}

/// Message for platforms without a capture backend yet.
#[cfg(not(target_os = "linux"))]
const NOT_SUPPORTED: &str = "recording not supported on this platform yet";

/// How long `start_recording` waits for the first captured frame before it calls
/// the attempt a failure. The picker itself is awaited in the portal half; this
/// only covers "session open but no buffers", which means the cast was cancelled.
const FIRST_FRAME_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long `stop_recording` waits for the capture thread to finalise the file.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Container the recorder writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingFormat {
    Mp4,
    Gif,
}

impl RecordingFormat {
    /// `mp4` or `gif` (case-insensitive). Anything else is a frontend bug.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mp4" => Ok(Self::Mp4),
            "gif" => Ok(Self::Gif),
            other => Err(format!(
                "unsupported recording format '{other}' (expected mp4 or gif)"
            )),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Gif => "gif",
        }
    }
}

/// Reduce arbitrary user input to one plain file name inside the target dir and
/// make its extension agree with the format.
///
/// Same rules as `export::safe_filename` but for the two recording containers,
/// so a `..`/path-laden name can never escape the chosen directory.
fn safe_filename(filename: &str, format: RecordingFormat) -> String {
    const FALLBACK: &str = "capicorn-recording";

    let trimmed = filename.trim();
    let hostile = trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0');

    let name = if hostile { FALLBACK } else { trimmed };
    let wanted = format.extension();

    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case(wanted) => name.to_string(),
        Some(_) => format!(
            "{}.{wanted}",
            Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(FALLBACK)
        ),
        None => format!("{name}.{wanted}"),
    }
}

/// Payload of the `recording-state` event.
#[derive(Clone, Debug, Serialize)]
struct RecordingStateEvent {
    recording: bool,
    path: Option<String>,
}

/// One live recording: the stop signal, the capture thread, the finalisation
/// channel, the output path and — on Linux — the portal session that must stay
/// open until the very end.
struct Active {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    /// Signalled by the capture thread once ffmpeg is finalised (or it gave up).
    done: mpsc::Receiver<()>,
    out_path: PathBuf,
    #[cfg(target_os = "linux")]
    #[allow(dead_code)] // held open on purpose: dropping it ends the portal cast
    session: ashpd::desktop::Session<ashpd::desktop::screencast::Screencast>,
}

/// Managed recorder state (`.manage(RecordingState::new())` in `lib.rs`).
pub struct RecordingState {
    inner: Mutex<Option<Active>>,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn is_active(&self) -> bool {
        self.inner.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    /// Claim the slot. On collision the caller gets its `Active` back so it can
    /// stop the recording it just started instead of leaving it orphaned.
    fn begin(&self, active: Active) -> Result<(), Active> {
        match self.inner.lock() {
            Ok(mut slot) if slot.is_none() => {
                *slot = Some(active);
                Ok(())
            }
            _ => Err(active),
        }
    }

    fn take(&self) -> Option<Active> {
        self.inner.lock().ok().and_then(|mut slot| slot.take())
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Start a recording of the monitor — or of `region` within it — into
/// `dir/filename`.
///
/// Resolves only once real frames are flowing into ffmpeg, so a denied/locked
/// portal surfaces as a rejected promise instead of a silent "recording" badge.
#[tauri::command]
pub async fn start_recording(
    app: tauri::AppHandle,
    state: State<'_, RecordingState>,
    format: String,
    dir: String,
    filename: String,
    region: Option<Region>,
) -> Result<(), String> {
    let format = RecordingFormat::parse(&format)?;
    let region = region.map(Region::validate).transpose()?;

    if dir.trim().is_empty() {
        return Err("no save directory given".to_string());
    }
    if state.is_active() {
        return Err("a recording is already in progress".to_string());
    }

    let dir = PathBuf::from(dir.trim());
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let out_path = dir.join(safe_filename(&filename, format));

    #[cfg(target_os = "linux")]
    {
        imp::start(app, state, format, out_path, region).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, state, format, out_path, region);
        Err(NOT_SUPPORTED.to_string())
    }
}

/// Stop the recording, close ffmpeg cleanly (so the container is finalised) and
/// return the path of the finished file.
#[tauri::command]
pub async fn stop_recording(
    app: tauri::AppHandle,
    state: State<'_, RecordingState>,
) -> Result<String, String> {
    let Some(active) = state.take() else {
        return Err("no recording in progress".to_string());
    };

    let finished = tauri::async_runtime::spawn_blocking(move || finish(active))
        .await
        .map_err(|e| format!("stopping the recorder failed: {e}"))?;

    let (path, outcome) = finished;
    let _ = app.emit(
        "recording-state",
        RecordingStateEvent {
            recording: false,
            path: Some(path.clone()),
        },
    );

    outcome.map(|()| path)
}

/// Is a recording running right now? (UI polls this on mount.)
#[tauri::command]
pub fn is_recording(state: State<'_, RecordingState>) -> bool {
    state.is_active()
}

/// Signal stop, wait for the capture thread to finalise ffmpeg, then drop the
/// portal session. Returns the output path even when stopping timed out, so the
/// caller can still tell the user where the (possibly truncated) file is.
fn finish(mut active: Active) -> (String, Result<(), String>) {
    let path = active.out_path.to_string_lossy().into_owned();

    active.stop.store(true, Ordering::Relaxed);

    let outcome = match active.done.recv_timeout(STOP_WAIT) {
        Ok(()) => Ok(()),
        Err(_) => Err(format!(
            "the recorder did not stop within {}s; {path} may be incomplete",
            STOP_WAIT.as_secs()
        )),
    };

    // Joining is safe: the capture thread stops touching PipeWire before it
    // signals `done`, so there is no sync roundtrip left to deadlock on.
    if let Some(handle) = active.join.take() {
        let _ = handle.join();
    }

    // Only now is it safe to end the portal session.
    #[cfg(target_os = "linux")]
    drop(active.session);

    (path, outcome)
}

// ---------------------------------------------------------------------------
// Linux implementation: portal half + PipeWire/ffmpeg capture half
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod imp {
    use super::{RecordingFormat, Region, FIRST_FRAME_WAIT};
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::io::{Cursor, Write};
    use std::os::fd::OwnedFd;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    use ashpd::desktop::screencast::{
        CursorMode, Screencast, SelectSourcesOptions, SourceType,
    };
    use ashpd::desktop::PersistMode;
    use pipewire as pw;
    use pw::spa;
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::format_utils;
    use spa::param::video::{VideoFormat, VideoInfoRaw};
    use spa::pod::{Pod, Property, Value};
    use spa::utils::Id;
    use tauri::{Emitter, State};

    /// GIFs are downscaled to this width and re-timed to [`GIF_FPS`]: a full-res
    /// desktop GIF is enormous and, at 30 fps, stutters in every viewer.
    const GIF_MAX_WIDTH: u32 = 1280;
    const GIF_FPS: u32 = 15;

    /// Portal calls that are pure D-Bus round-trips: a wedged portal hangs them.
    const PORTAL_CALL_TIMEOUT: u64 = 15;
    /// The picker: the user has to see it, decide and click "Share".
    const PICKER_TIMEOUT: u64 = 120;

    // -- pixel formats ------------------------------------------------------

    /// bytes-per-pixel + byte offsets of R, G and B inside one pixel.
    ///
    /// The `x` (padding) variants are what KWin's screencast node advertises, and
    /// getting the R/B order wrong is a silent colour swap, so every packed layout
    /// the compositors offer is listed here.
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

    /// Convert one packed frame into tightly-packed `rgb24`, honouring the
    /// producer's row stride (which is usually `width * bpp` but not always).
    ///
    /// Returns `false` when the buffer is too small for even one row — the check
    /// that turned a garbage frame into a visible "no frames" in the probe.
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

    // -- region crop --------------------------------------------------------

    /// yuv420p (and libx264's default) needs even dimensions; the frame we feed
    /// ffmpeg is padded rather than scaled, so the extra column/row is zeroed.
    fn even_floor(v: usize) -> usize {
        (v + 1) & !1
    }

    /// The exact rectangle of a `fw`x`fh` rgb24 frame that `region` covers, in
    /// frame pixels. `None` when the region misses the frame entirely.
    ///
    /// Clamps instead of rejecting: a selection dragged past the screen edge is
    /// normal, and the part that exists is what the user wants recorded.
    fn region_bounds(region: Region, fw: usize, fh: usize) -> Option<(usize, usize, usize, usize)> {
        if fw == 0 || fh == 0 || region.width == 0 || region.height == 0 {
            return None;
        }
        let left = region.left.max(0) as usize;
        let top = region.top.max(0) as usize;
        if left >= fw || top >= fh {
            return None;
        }
        let right = (region.left as i64 + region.width as i64).clamp(0, fw as i64) as usize;
        let bottom = (region.top as i64 + region.height as i64).clamp(0, fh as i64) as usize;
        let (width, height) = (right.saturating_sub(left), bottom.saturating_sub(top));
        if width == 0 || height == 0 {
            return None;
        }
        Some((left, top, width, height))
    }

    /// Padded output size for `region` inside a `fw`x`fh` frame — the value that
    /// goes into `ffmpeg -s`, and exactly what [`crop_rgb24`] writes per frame.
    pub(crate) fn region_size(region: Region, fw: usize, fh: usize) -> Option<(u32, u32)> {
        let (_, _, width, height) = region_bounds(region, fw, fh)?;
        Some((even_floor(width) as u32, even_floor(height) as u32))
    }

    /// Crop tightly-packed `rgb24` (`fw`x`fh`) to `region`, writing a
    /// tightly-packed image of the padded region size into `out`.
    ///
    /// Returns the `(width, height)` written, which always equals
    /// [`region_size`] — ffmpeg is spawned with that size, so a mismatch would
    /// shear the whole take. Padding (odd region width/height, and any part of
    /// the rectangle outside the frame) is filled with black.
    pub(crate) fn crop_rgb24(
        src: &[u8],
        fw: usize,
        fh: usize,
        region: Region,
        out: &mut Vec<u8>,
    ) -> Option<(u32, u32)> {
        let (left, top, width, height) = region_bounds(region, fw, fh)?;
        if fw == 0 || fh == 0 || src.len() < fw * fh * 3 {
            return None;
        }

        let (out_w, out_h) = region_size(region, fw, fh)?;
        let (out_w, out_h) = (out_w as usize, out_h as usize);

        out.clear();
        out.resize(out_w * out_h * 3, 0);

        let row_bytes = width * 3;
        let dst_stride = out_w * 3;
        for y in 0..height {
            let src_start = ((top + y) * fw + left) * 3;
            let dst_start = y * dst_stride;
            out[dst_start..dst_start + row_bytes]
                .copy_from_slice(&src[src_start..src_start + row_bytes]);
        }

        Some((out_w as u32, out_h as u32))
    }

    // -- ffmpeg -------------------------------------------------------------

    /// `ffmpeg` from `PATH`, unless the user pinned a binary via `CAPICORN_FFMPEG`.
    fn ffmpeg_program() -> String {
        std::env::var("CAPICORN_FFMPEG")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "ffmpeg".to_string())
    }

    /// Complete argument list for the encoder child.
    ///
    /// MP4 is h264/yuv420p with a 2-pixel pad so an odd screen width can never make
    /// libx264 fail; GIF uses the single-pass palettegen/paletteuse recipe verified
    /// in `recording.md` §5 (two-pass would mean re-reading a temp file).
    fn ffmpeg_args(
        out: &Path,
        format: RecordingFormat,
        width: u32,
        height: u32,
        fps: &str,
    ) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "rgb24".into(),
            "-s".into(),
            format!("{width}x{height}"),
            "-r".into(),
            fps.to_string(),
            "-i".into(),
            "-".into(),
            "-an".into(),
        ];

        match format {
            RecordingFormat::Mp4 => args.extend(
                [
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-crf",
                    "20",
                    // yuv420p needs even dimensions; screens are usually even but
                    // a 1366x768 laptop with a scaled output can be odd.
                    "-vf",
                    "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                    "-pix_fmt",
                    "yuv420p",
                    "-movflags",
                    "+faststart",
                ]
                .map(String::from),
            ),
            RecordingFormat::Gif => {
                let gif_width = (width.min(GIF_MAX_WIDTH) / 2 * 2).max(2);
                let graph = format!(
                    "fps={GIF_FPS},scale={gif_width}:-1:flags=lanczos,\
                     split[a][b];[a]palettegen=stats_mode=diff[p];\
                     [b][p]paletteuse=dither=bayer:bayer_scale=3"
                );
                args.extend(["-vf".to_string(), graph, "-loop".to_string(), "0".to_string()]);
            }
        }

        args.push(out.to_string_lossy().into_owned());
        args
    }

    fn spawn_ffmpeg(
        out: &Path,
        format: RecordingFormat,
        width: u32,
        height: u32,
        fps: &str,
    ) -> Result<(Child, ChildStdin), String> {
        if width == 0 || height == 0 {
            return Err("the capture stream reported an empty frame size".to_string());
        }

        // ffmpeg's own diagnostics go next to the recording, never to the app's
        // stdout (which would interleave with Tauri's logging).
        let mut log_path = out.as_os_str().to_os_string();
        log_path.push(".ffmpeg.log");
        let stderr = std::fs::File::create(&log_path)
            .or_else(|_| std::fs::File::create("/dev/null"))
            .map_err(|e| format!("cannot open {}: {e}", log_path.to_string_lossy()))?;

        let mut child = Command::new(ffmpeg_program())
            .args(ffmpeg_args(out, format, width, height, fps))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|e| format!("cannot start ffmpeg ({e}); is ffmpeg installed?"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg did not provide a stdin pipe".to_string())?;
        Ok((child, stdin))
    }

    /// Drop stdin (EOF) and wait for ffmpeg to flush the container. A hung ffmpeg
    /// is killed rather than blocking the stop button forever.
    fn close_child(mut child: Child) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
    }

    // -- SPA pods -----------------------------------------------------------

    /// Build one `EnumFormat` pod per candidate format. The caller keeps the
    /// `Vec<Vec<u8>>` alive: `Pod::from_bytes` only borrows.
    fn make_params(width: u32, height: u32, formats: &[VideoFormat]) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        for format in formats {
            // libspa 0.10 has no `Into<Vec<Property>>` for `VideoInfoRaw`
            // (only the audio one), so the pod is assembled by hand.
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

    /// `SPA_PARAM_Buffers`. Without it the pool defaults to 8 KiB buffers and
    /// frames are silently truncated (`chunk.size > mapped size`) — the trap that
    /// cost the most time in the probe (`recording.md` §6.1).
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

    struct PortalCapture {
        fd: OwnedFd,
        node: u32,
        size: (i32, i32),
        session: ashpd::desktop::Session<Screencast>,
    }

    /// Create the session, ask the user which monitor, wait for approval, and open
    /// the PipeWire remote. The returned session must outlive the capture.
    async fn open_portal() -> Result<PortalCapture, String> {
        let proxy = portal_call(PORTAL_CALL_TIMEOUT, "ScreenCast portal", Screencast::new()).await?;

        let session = portal_call(
            PORTAL_CALL_TIMEOUT,
            "CreateSession",
            proxy.create_session(Default::default()),
        )
        .await?;

        // `select_sources` hands back an empty `Request`; ashpd's own example drops
        // it (the portal answers on its well-known name). Do NOT `.response()` it.
        let _selection = portal_call(
            PORTAL_CALL_TIMEOUT,
            "SelectSources",
            proxy.select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(SourceType::MONITOR)
                    // The cursor is baked into the frame: right for mp4 and gif.
                    .set_cursor_mode(CursorMode::EMBEDDED)
                    .set_multiple(false)
                    // PersistMode lives in `ashpd::desktop`, not `screencast`.
                    .set_persist_mode(PersistMode::DoNot),
            ),
        )
        .await?;
        drop(_selection);

        // `start` opens the picker; it only resolves once the user clicks Share.
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
        // The negotiated format can differ from this; it is only a hint for the
        // initial enumeration, and `param_changed` has the truth.
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

    // -- capture half -------------------------------------------------------

    struct Ud {
        fmt: VideoInfoRaw,
        child: Rc<RefCell<Option<(Child, ChildStdin)>>>,
        /// Report the first frame (or the first fatal error) to `start_recording`.
        notify: Rc<RefCell<Option<mpsc::SyncSender<Result<(), String>>>>>,
        frames: Rc<Cell<u64>>,
        stop: Arc<AtomicBool>,
        loopweak: pw::main_loop::MainLoopWeak,
        out: PathBuf,
        format: RecordingFormat,
        /// Selected rectangle, constant for the whole take. `None` records the
        /// whole monitor exactly as before.
        region: Option<Region>,
        /// Size ffmpeg was told via `-s` (already even-padded for a region).
        out_size: (u32, u32),
        /// Full-frame rgb24 from `convert_into`.
        buf: Vec<u8>,
        /// Reused scratch for the cropped (and padded) rgb24 frame.
        cropped: Vec<u8>,
        spawned: bool,
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

    /// Spawn ffmpeg on the first frame — size and frame rate are only trustworthy
    /// once `VideoInfoRaw` has been parsed, not from `Stream::size()`.
    fn ensure_ffmpeg(ud: &mut Ud) -> bool {
        let size = ud.fmt.size();
        let format = ud.fmt.format();
        let Some((_, _)) = pixfmt(format) else {
            let msg = format!(
                "unsupported capture pixel format {format:?} — this compositor offers no packed RGB/BGR mode"
            );
            eprintln!("capicorn: {msg}");
            report(ud, Err(msg));
            quit(ud);
            return false;
        };

        let fps = ud.fmt.framerate();
        let fps = if fps.num > 0 && fps.denom > 0 {
            format!("{}/{}", fps.num, fps.denom)
        } else {
            // Compositors rarely report a frame rate; ffmpeg's `-r` is nominal.
            "30".to_string()
        };

        // ffmpeg must be told the size of what we actually write: the padded
        // region when one is selected, the full frame otherwise.
        let (out_w, out_h) = match ud.region {
            Some(region) => match region_size(region, size.width as usize, size.height as usize) {
                Some(dims) => dims,
                None => {
                    let msg = format!(
                        "the recording region {}x{}+{}+{} lies outside the {}x{} capture",
                        region.width, region.height, region.left, region.top,
                        size.width, size.height
                    );
                    eprintln!("capicorn: {msg}");
                    report(ud, Err(msg));
                    quit(ud);
                    return false;
                }
            },
            None => (size.width, size.height),
        };

        match spawn_ffmpeg(&ud.out, ud.format, out_w, out_h, &fps) {
            Ok(pair) => {
                *ud.child.borrow_mut() = Some(pair);
                ud.out_size = (out_w, out_h);
                ud.spawned = true;
                report(ud, Ok(()));
                true
            }
            Err(err) => {
                eprintln!("capicorn: {err}");
                report(ud, Err(err));
                quit(ud);
                false
            }
        }
    }

    /// The whole capture half for one recording. Returns once the stream ended
    /// (stop requested, error, or the compositor closed the cast).
    fn capture(
        fd: OwnedFd,
        node: u32,
        size: (i32, i32),
        out: PathBuf,
        format: RecordingFormat,
        region: Option<Region>,
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

        let stream = pw::stream::StreamBox::new(&core, "capicorn-recording", props)
            .map_err(|e| format!("cannot create the PipeWire capture stream: {e}"))?;

        let child_slot: Rc<RefCell<Option<(Child, ChildStdin)>>> = Rc::new(RefCell::new(None));
        let notify = Rc::new(RefCell::new(Some(started)));
        let frames = Rc::new(Cell::new(0u64));

        let ud = Ud {
            fmt: VideoInfoRaw::new(),
            child: child_slot.clone(),
            notify: notify.clone(),
            frames: frames.clone(),
            stop: stop.clone(),
            loopweak: mainloop.downgrade(),
            out: out.clone(),
            format,
            region,
            // Filled in by `ensure_ffmpeg` once the negotiated size is known.
            out_size: (0, 0),
            buf: Vec::new(),
            cropped: Vec::new(),
            spawned: false,
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
                    let fps = ud.fmt.framerate();
                    eprintln!(
                        "capicorn: capture format {:?} {}x{} @ {}/{}",
                        ud.fmt.format(),
                        size.width,
                        size.height,
                        fps.num,
                        fps.denom
                    );
                }
            })
            .state_changed(|_, _, old, new| {
                if old != new {
                    eprintln!("capicorn: capture stream {old:?} -> {new:?}");
                }
            })
            .process(|stream, ud| {
                // Stop is polled here as well as from the timer below, so a stop
                // lands on the very next frame instead of waiting for the tick.
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

                if !ud.spawned && !ensure_ffmpeg(ud) {
                    return;
                }

                let size = ud.fmt.size();
                let (width, height) = (size.width as usize, size.height as usize);
                let Some((bpp, idx)) = pixfmt(ud.fmt.format()) else {
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

                // Buffers can be smaller than the frame (8 KiB default pool): never
                // index past what is actually mapped.
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

                // Crop to the selected rectangle *before* anything reaches the
                // encoder, and pad to even dimensions: that padded size is the
                // `-s` ffmpeg was spawned with, so every frame fits exactly.
                let frame: &[u8] = match ud.region {
                    Some(region) => {
                        match crop_rgb24(&ud.buf, width, height, region, &mut ud.cropped) {
                            Some(size) => {
                                debug_assert_eq!(size, ud.out_size, "crop size drifted from -s");
                                &ud.cropped
                            }
                            // The region no longer overlaps the frame: skip this
                            // one rather than write a short buffer ffmpeg would
                            // misread as pixels.
                            None => return,
                        }
                    }
                    None => &ud.buf,
                };

                let wrote = {
                    let mut slot = ud.child.borrow_mut();
                    match slot.as_mut() {
                        // EPIPE means ffmpeg died: stop instead of spinning.
                        Some((_, stdin)) => stdin.write_all(frame).is_ok(),
                        None => false,
                    }
                };

                if !wrote {
                    eprintln!("capicorn: the encoder stopped accepting frames");
                    quit(ud);
                    return;
                }

                let count = ud.frames.get() + 1;
                ud.frames.set(count);
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

        // Poll the stop flag from the loop: on a static screen no buffers arrive,
        // so a stop that only lived in `process()` could never land.
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

        // Everything below runs after the loop stopped: no PipeWire sync here.
        if let Some((child, stdin)) = child_slot.borrow_mut().take() {
            drop(stdin);
            close_child(child);
        }

        let captured = frames.get();
        eprintln!(
            "capicorn: captured {captured} frames -> {}",
            out.to_string_lossy()
        );

        // If the stream ended before the first frame (cancel, compositor quit) the
        // command is still waiting on `notify`.
        let unreported = notify.borrow().is_some();
        if unreported {
            report_early(&notify, captured);
        }

        // Destroying PipeWire objects after the loop returned needs a sync roundtrip
        // that nobody services -> deadlock (`recording.md` §6.5). This thread is
        // about to exit, so leaking them is the safe teardown.
        std::mem::forget(tick);
        std::mem::forget(listener);
        std::mem::forget(stream);
        std::mem::forget(mainloop);

        if captured == 0 {
            return Err("the capture stream produced no frames".to_string());
        }
        Ok(())
    }

    fn report_early(
        notify: &Rc<RefCell<Option<mpsc::SyncSender<Result<(), String>>>>>,
        captured: u64,
    ) {
        if let Some(tx) = notify.borrow_mut().take() {
            let _ = tx.try_send(Err(format!(
                "the screen cast ended before the first frame ({captured} captured)"
            )));
        }
    }

    // -- command wiring -----------------------------------------------------

    /// Linux `start_recording`: portal half here, capture half on its own thread,
    /// then hand the handle to the managed state.
    pub(super) async fn start(
        app: tauri::AppHandle,
        state: State<'_, super::RecordingState>,
        format: RecordingFormat,
        out_path: PathBuf,
        region: Option<Region>,
    ) -> Result<(), String> {
        let portal = open_portal().await?;

        if let Some(region) = region {
            eprintln!(
                "capicorn: recording region {}x{}+{}+{}",
                region.width, region.height, region.left, region.top
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::channel();

        let thread = {
            let stop = stop.clone();
            let out = out_path.clone();
            std::thread::Builder::new()
                .name("capicorn-recording".to_string())
                .spawn(move || {
                    let result = capture(
                        portal.fd, portal.node, portal.size, out, format, region, stop,
                        started_tx,
                    );
                    if let Err(err) = &result {
                        eprintln!("capicorn: recording stopped: {err}");
                    }
                    let _ = done_tx.send(());
                })
                .map_err(|e| format!("cannot start the recording thread: {e}"))?
        };

        // "Started" means real frames reached ffmpeg, not that a session exists.
        let signal = match tauri::async_runtime::spawn_blocking(move || {
            started_rx.recv_timeout(FIRST_FRAME_WAIT)
        })
        .await
        {
            Ok(signal) => signal,
            Err(err) => {
                stop.store(true, Ordering::Relaxed);
                return Err(format!("waiting for the first frame failed: {err}"));
            }
        };

        let failure = match signal {
            Ok(Ok(())) => None,
            Ok(Err(err)) => Some(err),
            // The sender is gone: the capture thread gave up before the first
            // frame (fd connect, stream setup, ffmpeg), and already logged why.
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

        let out_path_string = out_path.to_string_lossy().into_owned();
        let active = super::Active {
            stop,
            join: Some(thread),
            done: done_rx,
            out_path,
            session: portal.session,
        };

        if let Err(rejected) = state.begin(active) {
            // Lost a race with another start: stop what we just started.
            let mut rejected = rejected;
            rejected.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = rejected.join.take() {
                let _ = handle.join();
            }
            drop(rejected.session);
            return Err("a recording is already in progress".to_string());
        }

        let _ = app.emit(
            "recording-state",
            super::RecordingStateEvent {
                recording: true,
                path: Some(out_path_string),
            },
        );
        Ok(())
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
            // rgb24 keeps R,G,B order: red first, blue second — a swap would flip this.
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

        // -- region crop ----------------------------------------------------

        /// A frame where every pixel is unique, so a crop that is off by one row
        /// or column cannot accidentally compare equal.
        fn sample_frame(fw: usize, fh: usize) -> Vec<u8> {
            let mut frame = vec![0u8; fw * fh * 3];
            for y in 0..fh {
                for x in 0..fw {
                    let i = (y * fw + x) * 3;
                    frame[i] = x as u8;
                    frame[i + 1] = y as u8;
                    frame[i + 2] = 200;
                }
            }
            frame
        }

        fn px(frame: &[u8], stride: usize, x: usize, y: usize) -> [u8; 3] {
            let i = (y * stride + x) * 3;
            [frame[i], frame[i + 1], frame[i + 2]]
        }

        #[test]
        fn crop_recovers_exactly_the_selected_pixels() {
            let (fw, fh) = (5usize, 4usize);
            let frame = sample_frame(fw, fh);
            let region = Region { left: 1, top: 2, width: 2, height: 2 };
            let mut out = Vec::new();

            let size = crop_rgb24(&frame, fw, fh, region, &mut out).expect("region is inside");
            assert_eq!(size, (2, 2));
            assert_eq!(out.len(), 2 * 2 * 3);

            for y in 0..2 {
                for x in 0..2 {
                    assert_eq!(
                        px(&out, 2, x, y),
                        px(&frame, fw, 1 + x, 2 + y),
                        "pixel ({x},{y}) of the crop"
                    );
                }
            }
        }

        #[test]
        fn crop_clamps_a_region_that_runs_past_the_frame_edge() {
            let (fw, fh) = (4usize, 3usize);
            let frame = sample_frame(fw, fh);
            // Only the single pixel at (3,2) is on screen.
            let region = Region { left: 3, top: 2, width: 10, height: 10 };
            let mut out = Vec::new();

            let size = crop_rgb24(&frame, fw, fh, region, &mut out).expect("it overlaps");
            // 1x1 visible -> padded to 2x2 so yuv420p stays happy.
            assert_eq!(size, (2, 2));
            assert_eq!(out.len(), 2 * 2 * 3);
            assert_eq!(px(&out, 2, 0, 0), px(&frame, fw, 3, 2));
            assert_eq!(px(&out, 2, 1, 0), [0, 0, 0], "pad column is black");
            assert_eq!(px(&out, 2, 0, 1), [0, 0, 0], "pad row is black");
        }

        #[test]
        fn crop_ignores_the_part_of_a_region_before_the_origin() {
            let (fw, fh) = (4usize, 3usize);
            let frame = sample_frame(fw, fh);
            let region = Region { left: -2, top: -1, width: 4, height: 3 };
            let mut out = Vec::new();

            let size = crop_rgb24(&frame, fw, fh, region, &mut out).expect("it overlaps");
            assert_eq!(size, (2, 2), "the on-screen 2x2 part");
            assert_eq!(px(&out, 2, 0, 0), px(&frame, fw, 0, 0));
            assert_eq!(px(&out, 2, 1, 1), px(&frame, fw, 1, 1));
        }

        #[test]
        fn odd_region_sizes_are_padded_and_ffmpeg_is_told_that_size() {
            let frame = sample_frame(7, 5);
            let region = Region { left: 2, top: 1, width: 3, height: 3 };
            let mut out = Vec::new();

            let size = crop_rgb24(&frame, 7, 5, region, &mut out).expect("inside the frame");
            assert_eq!(size, (4, 4), "3x3 region padded to even");
            assert_eq!(size, region_size(region, 7, 5).expect("region size"));
            assert_eq!(out.len(), 4 * 4 * 3);
            assert_eq!(px(&out, 4, 3, 3), [0, 0, 0], "pad corner is black");

            // The frame layout the encoder is spawned with must match the crop
            // exactly, or every take would shear.
            let args = ffmpeg_args(Path::new("/tmp/out.mp4"), RecordingFormat::Mp4, size.0, size.1, "30");
            assert!(args.contains(&"4x4".to_string()), "{args:?}");
        }

        #[test]
        fn crop_rejects_regions_that_miss_the_frame() {
            let frame = sample_frame(4, 3);
            let mut out = Vec::new();

            let outside = Region { left: 9, top: 0, width: 2, height: 2 };
            assert!(crop_rgb24(&frame, 4, 3, outside, &mut out).is_none());
            assert!(region_size(outside, 4, 3).is_none());

            let empty = Region { left: 0, top: 0, width: 0, height: 2 };
            assert!(crop_rgb24(&frame, 4, 3, empty, &mut out).is_none());

            let short = Region { left: 0, top: 0, width: 2, height: 2 };
            assert!(crop_rgb24(&frame[..8], 4, 3, short, &mut out).is_none());
        }

        #[test]
        fn mp4_args_encode_h264_at_the_negotiated_size() {
            let args = ffmpeg_args(Path::new("/tmp/out.mp4"), RecordingFormat::Mp4, 1920, 1080, "30");
            assert!(args.contains(&"rawvideo".to_string()));
            assert!(args.contains(&"1920x1080".to_string()));
            assert!(args.contains(&"libx264".to_string()));
            assert!(args.contains(&"yuv420p".to_string()));
            assert_eq!(args.last().map(String::as_str), Some("/tmp/out.mp4"));
        }

        #[test]
        fn gif_args_use_the_palette_recipe_and_a_downscale() {
            let args = ffmpeg_args(Path::new("/tmp/out.gif"), RecordingFormat::Gif, 3840, 2160, "30");
            let graph = args
                .iter()
                .position(|a| a == "-vf")
                .map(|i| args[i + 1].clone())
                .expect("gif has a filter graph");
            assert!(graph.contains("palettegen"), "{graph}");
            assert!(graph.contains("paletteuse"), "{graph}");
            assert!(graph.starts_with("fps=15"), "{graph}");
            assert!(graph.contains("scale=1280:"), "3840 wide is capped: {graph}");
        }

        // -- real ffmpeg children: the exact runtime path -------------------

        /// ffmpeg is an existing Capicorn requirement, but a dev box may lack it:
        /// skip rather than fail when it really is missing.
        fn ffmpeg_present() -> bool {
            Command::new(ffmpeg_program())
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }

        fn temp_out(ext: &str) -> PathBuf {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            std::env::temp_dir().join(format!(
                "capicorn-recording-test-{}-{stamp}.{ext}",
                std::process::id()
            ))
        }

        /// Pipe 30 synthetic rgb24 frames through the real encoder child.
        fn encode_frames(out: &Path, format: RecordingFormat) -> Vec<u8> {
            let (child, mut stdin) =
                spawn_ffmpeg(out, format, 64, 48, "30").expect("spawn ffmpeg");
            let mut frame = vec![0u8; 64 * 48 * 3];
            for i in 0..30u32 {
                for (x, px) in frame.chunks_mut(3).enumerate() {
                    px[0] = (x as u8).wrapping_add(i as u8);
                    px[1] = (i as u8).wrapping_mul(8);
                    px[2] = 40;
                }
                stdin.write_all(&frame).expect("write frame to ffmpeg");
            }
            // EOF is what finalises the container.
            drop(stdin);
            close_child(child);
            std::fs::read(out).unwrap_or_default()
        }

        fn ffmpeg_log(out: &Path) -> String {
            let mut log = out.as_os_str().to_os_string();
            log.push(".ffmpeg.log");
            std::fs::read_to_string(log).unwrap_or_default()
        }

        #[test]
        fn mp4_child_encodes_real_frames() {
            if !ffmpeg_present() {
                eprintln!("skipping: ffmpeg is not installed");
                return;
            }
            let out = temp_out("mp4");
            let bytes = encode_frames(&out, RecordingFormat::Mp4);
            let log = ffmpeg_log(&out);
            let _ = std::fs::remove_file(&out);
            assert!(
                bytes.len() > 1000,
                "mp4 is {} bytes; ffmpeg said: {log}",
                bytes.len()
            );
            assert_eq!(&bytes[4..8], b"ftyp", "not an MP4 container: {log}");
        }

        #[test]
        fn gif_child_accepts_the_palette_graph() {
            if !ffmpeg_present() {
                eprintln!("skipping: ffmpeg is not installed");
                return;
            }
            let out = temp_out("gif");
            let bytes = encode_frames(&out, RecordingFormat::Gif);
            let log = ffmpeg_log(&out);
            let _ = std::fs::remove_file(&out);
            assert!(bytes.len() > 200, "gif is {} bytes; ffmpeg said: {log}", bytes.len());
            assert_eq!(&bytes[..6], b"GIF89a", "not a GIF: {log}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse_case_insensitively() {
        assert_eq!(RecordingFormat::parse("mp4"), Ok(RecordingFormat::Mp4));
        assert_eq!(RecordingFormat::parse(" GIF "), Ok(RecordingFormat::Gif));
        assert!(RecordingFormat::parse("mkv").is_err());
        assert!(RecordingFormat::parse("").is_err());
    }

    #[test]
    fn filenames_cannot_escape_the_target_dir() {
        assert_eq!(safe_filename("demo", RecordingFormat::Mp4), "demo.mp4");
        assert_eq!(safe_filename("demo.MP4", RecordingFormat::Mp4), "demo.MP4");
        assert_eq!(safe_filename("demo.gif", RecordingFormat::Mp4), "demo.mp4");
        assert_eq!(safe_filename("", RecordingFormat::Gif), "capicorn-recording.gif");
        assert_eq!(
            safe_filename("../../etc/passwd", RecordingFormat::Mp4),
            "capicorn-recording.mp4"
        );
        assert_eq!(safe_filename(".", RecordingFormat::Gif), "capicorn-recording.gif");
    }

    #[test]
    fn state_starts_idle() {
        let state = RecordingState::new();
        assert!(!state.is_active());
        assert!(state.take().is_none());
    }
}