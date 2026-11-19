//! L4 recording — ScreenCast portal → PipeWire → ffmpeg.
//!
//! Two halves, two threads, joined by an fd and a node id (the pattern proven in
//! the `tauri-desktop-app` skill, `references/recording.md`):
//!
//! * **portal half** ([`session`], async): `ashpd` `Screencast` creates a
//!   session, asks the user what to share, then hands back an `OwnedFd` plus the
//!   stream's PipeWire node id. The session and the tokio runtime stay alive for
//!   the whole take — dropping either ends the cast.
//! * **capture half** ([`capture`], dedicated thread): connects that fd with
//!   `pipewire`, waits for the node in the registry, converts BGRx → rgb24
//!   (stride-aware), crops to an optional [`Region`], and writes into a
//!   lazily-spawned `ffmpeg` child's stdin.
//!
//! Encoding is a subprocess on purpose: libx264 through one pipe is ~50 lines
//! and gives MP4 and GIF for free.
//!
//! Progress and terminal states reach the caller through [`RecordingEvent`];
//! [`spawn_recording`] itself only fails for problems known before any portal
//! call (bad config, missing ffmpeg, unspawnable thread). Everything after that
//! — picker cancelled, no frames, encoder died — arrives as
//! [`RecordingEvent::Failed`], so a caller never blocks on a 120 s picker.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

mod assemble;
mod audio;
#[cfg(target_os = "linux")]
mod capture;
mod crop;
mod ffmpeg;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(all(test, target_os = "linux"))]
mod probe;
#[cfg(target_os = "linux")]
mod session;

pub use assemble::{
    AssembleError, AssembleReport, DEFAULT_OUTPUT_NAME, assemble_timelapse, clamp_fps,
    count_frames, glob_match, validate_pattern,
};
pub use crop::crop_frame_rgb24;

/// Frame rate used when a config asks for 0 (the UI sent nothing).
pub const DEFAULT_FPS: u32 = 30;
/// Slowest capture we ask the portal for — below this it is a slideshow.
pub const MIN_FPS: u32 = 1;
/// Fastest capture we accept. Beyond it the conversion outruns what a screencast
/// node produces anyway, and a hostile value must not become ffmpeg's `-r`.
pub const MAX_FPS: u32 = 60;

/// How long the capture half waits for the first frame before it calls the take
/// a failure. The picker is awaited separately in the portal half; this only
/// covers "session open but no buffers", which means the cast was cancelled.
pub(crate) const FIRST_FRAME_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long stopping waits for ffmpeg to finalise the container.
pub(crate) const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Rectangle of the captured stream to keep, in stream pixels.
///
/// `left`/`top` are signed so a selection dragged partly off-screen still
/// arrives intact; the crop clamps whatever falls outside the frame and
/// black-pads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// An empty rectangle would give ffmpeg a zero `-s`; reject it early.
    pub fn is_usable(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// Container the recorder writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingFormat {
    Mp4,
    Gif,
}

impl RecordingFormat {
    /// `mp4` or `gif`, case-insensitive and trimmed. Anything else is a caller
    /// bug, so it is an error rather than a silent default.
    pub fn parse(value: &str) -> Result<Self, RecordingError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mp4" => Ok(Self::Mp4),
            "gif" => Ok(Self::Gif),
            other => Err(RecordingError::Unsupported(format!(
                "unsupported recording format '{other}' (expected mp4 or gif)"
            ))),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Gif => "gif",
        }
    }
}

/// One selectable recording input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSource {
    /// The id to pass back in [`AudioConfig::source`] — a PulseAudio/PipeWire
    /// source name (fed to `-f pulse -i <id>`).
    pub id: String,
    /// Human-readable label for a picker.
    pub name: String,
    /// `"input"` for a real capture device, `"monitor"` for a sink monitor
    /// (what the machine plays), `"other"` when the id is unrecognised.
    pub kind: String,
}

/// Which audio to capture alongside the screen. The default records a silent
/// take.
///
/// `source` names the microphone (a [`list_audio_sources`] id); `system_audio`
/// records what the machine plays via the default output's monitor. Both may be
/// on at once and are mixed into a single AAC track (live parity, W13). GIF
/// takes never carry audio.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioConfig {
    pub source: Option<String>,
    pub system_audio: bool,
}

impl AudioConfig {
    /// The configured microphone, ignoring the blank ids a settings dropdown
    /// emits.
    pub fn source_id(&self) -> Option<&str> {
        self.source
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }

    /// How many audio inputs this config opens (0, 1 or 2).
    pub fn input_count(&self) -> usize {
        usize::from(self.source_id().is_some()) + usize::from(self.system_audio)
    }
}

/// Everything one take needs.
#[derive(Clone, Debug)]
pub struct RecordingConfig {
    pub format: RecordingFormat,
    /// `None` records the whole captured surface unchanged.
    pub region: Option<Region>,
    pub audio: AudioConfig,
    /// Requested frames per second, clamped to [`MIN_FPS`]..=[`MAX_FPS`]
    /// ([`DEFAULT_FPS`] when 0). Clamping is not a validation error: a UI slider
    /// is loose, not hostile.
    pub fps: u32,
    pub output: PathBuf,
}

impl RecordingConfig {
    /// Defaults for a take written to `output`.
    pub fn new(format: RecordingFormat, output: impl Into<PathBuf>) -> Self {
        Self {
            format,
            region: None,
            audio: AudioConfig::default(),
            fps: DEFAULT_FPS,
            output: output.into(),
        }
    }

    /// The frame rate actually used: 0 means "unset", anything else is clamped.
    pub fn effective_fps(&self) -> u32 {
        if self.fps == 0 {
            DEFAULT_FPS
        } else {
            self.fps.clamp(MIN_FPS, MAX_FPS)
        }
    }

    /// Reject what cannot work before any portal window appears.
    pub fn validate(&self) -> Result<(), RecordingError> {
        if let Some(region) = self.region {
            if !region.is_usable() {
                return Err(RecordingError::Unsupported(
                    "the recording region has no area".to_string(),
                ));
            }
            if region.width > crop::MAX_DIM || region.height > crop::MAX_DIM {
                return Err(RecordingError::Unsupported(format!(
                    "the recording region is larger than {}x{} pixels",
                    crop::MAX_DIM,
                    crop::MAX_DIM
                )));
            }
        }
        if self.output.as_os_str().is_empty() {
            return Err(RecordingError::Io(
                "the recording output path is empty".to_string(),
            ));
        }
        let wanted = self.format.extension();
        let extension_matches = self
            .output
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(wanted));
        if !extension_matches {
            return Err(RecordingError::Unsupported(format!(
                "output {} must end in .{wanted} for a {wanted} take",
                self.output.display()
            )));
        }
        if let Some(parent) = self.output.parent()
            && !parent.as_os_str().is_empty()
            && !parent.is_dir()
        {
            return Err(RecordingError::Io(format!(
                "the recording directory {} does not exist",
                parent.display()
            )));
        }
        Ok(())
    }
}

/// Progress of one take. The terminal variants are [`RecordingEvent::Stopped`]
/// and [`RecordingEvent::Failed`]; nothing is sent after those.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordingEvent {
    /// First frame reached the encoder. The dimensions are what ffmpeg was given
    /// (the padded region when one is selected, the frame otherwise).
    Started {
        width: u32,
        height: u32,
    },
    /// The container is finalised and on disk.
    Stopped {
        path: PathBuf,
    },
    /// Human-readable cause; the take is over.
    Failed(String),
    Paused,
    Resumed,
}

/// Errors a recording attempt can fail with.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RecordingError {
    /// xdg-desktop-portal: missing, wedged, refused, or the picker was cancelled.
    #[error("screen cast portal: {0}")]
    Portal(String),
    /// PipeWire: fd connect, stream setup, negotiation, frame conversion.
    #[error("PipeWire: {0}")]
    PipeWire(String),
    /// The encoder child: missing binary, refused frames, died mid-take.
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
    /// Filesystem / thread plumbing.
    #[error("I/O: {0}")]
    Io(String),
    /// A config the pipeline cannot honour (bad region, format, path).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A missing dependency the user can install.
    #[error("not available: {0}")]
    NotAvailable(String),
}

impl From<std::io::Error> for RecordingError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl RecordingError {
    /// True when the user simply cancelled the picker — the one case where a
    /// neutral message is right instead of the real error.
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Portal(message) => is_cancel_text(message),
            _ => false,
        }
    }

    /// Message safe to show in a UI: neutral for a cancel, real otherwise.
    pub fn user_message(&self) -> String {
        if self.is_cancelled() {
            "recording was cancelled".to_string()
        } else {
            self.to_string()
        }
    }
}

/// A portal refusal that means "the user said no", not "the pipeline is broken".
fn is_cancel_text(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    ["cancelled", "canceled", "aborted", "dismissed"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Commands the caller sends to the thread that owns the take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Stop,
    Pause,
    Resume,
}

/// A live take. Dropping it stops and finalises the recording.
#[derive(Debug)]
pub struct RecordingHandle {
    cmd: mpsc::Sender<Command>,
    join: Mutex<Option<JoinHandle<()>>>,
    active: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl RecordingHandle {
    /// Ask the take to stop; the container is finalised on the capture thread and
    /// [`RecordingEvent::Stopped`] follows. Idempotent.
    pub fn stop(&self) {
        let _ = self.cmd.send(Command::Stop);
    }

    /// Stop feeding frames without ending the take.
    ///
    /// The paused interval simply has no frames: the stream, the portal session
    /// and the encoder all stay alive, and resuming continues the same file.
    pub fn pause(&self) {
        let _ = self.cmd.send(Command::Pause);
    }

    pub fn resume(&self) {
        let _ = self.cmd.send(Command::Resume);
    }

    /// True until the take ends (or fails, or the handle is dropped).
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

impl Drop for RecordingHandle {
    fn drop(&mut self) {
        let _ = self.cmd.send(Command::Stop);
        let handle = self.join.lock().ok().and_then(|mut slot| slot.take());
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        self.active.store(false, Ordering::Relaxed);
    }
}

/// Start a take: portal picker first (it blocks on the user), then the capture
/// thread. Returns as soon as both are under way.
///
/// # Errors
/// [`RecordingError::Unsupported`] / [`RecordingError::Io`] for a config that
/// cannot work, [`RecordingError::NotAvailable`] when ffmpeg is missing — all
/// before any window is shown. Everything later arrives as events.
pub fn spawn_recording(
    config: RecordingConfig,
    events: mpsc::Sender<RecordingEvent>,
) -> Result<RecordingHandle, RecordingError> {
    config.validate()?;
    if !ffmpeg::available() {
        return Err(RecordingError::NotAvailable(
            "ffmpeg was not found on PATH — install ffmpeg and put it on PATH".to_string(),
        ));
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = events;
        Err(RecordingError::NotAvailable(
            "recording is implemented for Linux (ScreenCast portal + PipeWire) only".to_string(),
        ))
    }

    #[cfg(target_os = "linux")]
    {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let active = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let join = {
            let active = active.clone();
            let paused = paused.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("snagcraft-recording".to_string())
                .spawn(move || session::run(config, events, cmd_rx, stop, paused, active))
                .map_err(|error| {
                    RecordingError::Io(format!("cannot start the recording thread: {error}"))
                })?
        };

        Ok(RecordingHandle {
            cmd: cmd_tx,
            join: Mutex::new(Some(join)),
            active,
            paused,
        })
    }
}

/// Is a runnable `ffmpeg` on `PATH` (or at `$SNAGCRAFT_FFMPEG`)?
///
/// Worth checking before the picker opens: a missing encoder is the most common
/// "recording does nothing".
pub fn ffmpeg_available() -> bool {
    ffmpeg::available()
}

/// Every audio input this machine can record from.
///
/// Never fails for environment reasons: a headless box, a missing `pactl` or a
/// dead sound server all yield an empty list, because "no devices" is a normal
/// state for a settings screen.
pub fn list_audio_sources() -> Result<Vec<AudioSource>, RecordingError> {
    Ok(audio::list())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(format: RecordingFormat, output: &str) -> RecordingConfig {
        RecordingConfig::new(format, output)
    }

    #[test]
    fn formats_parse_case_insensitively() {
        assert_eq!(RecordingFormat::parse("mp4"), Ok(RecordingFormat::Mp4));
        assert_eq!(RecordingFormat::parse(" GIF "), Ok(RecordingFormat::Gif));
        assert_eq!(RecordingFormat::parse("MP4"), Ok(RecordingFormat::Mp4));
    }

    #[test]
    fn an_unknown_format_is_an_unsupported_error() {
        let error = RecordingFormat::parse("webm").expect_err("webm is not offered");
        assert!(matches!(error, RecordingError::Unsupported(_)));
        assert!(error.to_string().contains("webm"));
    }

    #[test]
    fn extensions_match_the_container() {
        assert_eq!(RecordingFormat::Mp4.extension(), "mp4");
        assert_eq!(RecordingFormat::Gif.extension(), "gif");
    }

    #[test]
    fn fps_zero_means_the_default_and_hostile_rates_are_clamped() {
        let mut cfg = config(RecordingFormat::Mp4, "/tmp/a.mp4");
        cfg.fps = 0;
        assert_eq!(cfg.effective_fps(), DEFAULT_FPS);
        cfg.fps = 1;
        assert_eq!(cfg.effective_fps(), 1);
        cfg.fps = 25;
        assert_eq!(cfg.effective_fps(), 25);
        cfg.fps = u32::MAX;
        assert_eq!(cfg.effective_fps(), MAX_FPS);
    }

    #[test]
    fn a_config_with_an_empty_region_is_rejected() {
        let mut cfg = config(RecordingFormat::Mp4, "/tmp/a.mp4");
        cfg.region = Some(Region {
            left: 0,
            top: 0,
            width: 0,
            height: 10,
        });
        assert!(matches!(
            cfg.validate(),
            Err(RecordingError::Unsupported(_))
        ));
    }

    #[test]
    fn a_config_with_an_absurd_region_is_rejected() {
        let mut cfg = config(RecordingFormat::Mp4, "/tmp/a.mp4");
        cfg.region = Some(Region {
            left: 0,
            top: 0,
            width: 100_000,
            height: 10,
        });
        let error = cfg.validate().expect_err("100k wide is not a screen");
        assert!(error.to_string().contains("larger than"));
    }

    #[test]
    fn a_config_whose_extension_contradicts_the_format_is_rejected() {
        let cfg = config(RecordingFormat::Gif, "/tmp/a.mp4");
        let error = cfg.validate().expect_err("a gif cannot be an mp4");
        assert!(error.to_string().contains("must end in .gif"));
    }

    #[test]
    fn a_config_for_a_missing_directory_is_rejected() {
        let cfg = config(
            RecordingFormat::Mp4,
            "/home/rostads/.hermes/cache/scratch/definitely-not-here/a.mp4",
        );
        let error = cfg.validate().expect_err("no such directory");
        assert!(matches!(error, RecordingError::Io(_)));
    }

    #[test]
    fn a_sane_config_validates() {
        // Use the real temp dir (exists on every platform; "/tmp" does not on
        // Windows, where validate's parent-is-dir check would otherwise fail).
        let output = std::env::temp_dir().join("snagcraft-rec-sane.mp4");
        let mut cfg = config(RecordingFormat::Mp4, &output.to_string_lossy());
        cfg.region = Some(Region {
            left: -4,
            top: 8,
            width: 640,
            height: 480,
        });
        cfg.fps = 30;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn a_blank_audio_id_is_no_source() {
        let mut audio = AudioConfig {
            source: Some("   ".to_string()),
            system_audio: false,
        };
        assert_eq!(audio.source_id(), None);
        audio.source = Some(" alsa_input.pci-0000_00_1f.3.analog-stereo ".to_string());
        assert_eq!(
            audio.source_id(),
            Some("alsa_input.pci-0000_00_1f.3.analog-stereo")
        );
    }

    #[test]
    fn input_count_covers_zero_one_and_two_sources() {
        let silent = AudioConfig::default();
        assert_eq!(silent.input_count(), 0);

        let mic_only = AudioConfig {
            source: Some("alsa_input.usb-mic".to_string()),
            system_audio: false,
        };
        assert_eq!(mic_only.input_count(), 1);

        let system_only = AudioConfig {
            source: None,
            system_audio: true,
        };
        assert_eq!(system_only.input_count(), 1);

        let both = AudioConfig {
            source: Some("alsa_input.usb-mic".to_string()),
            system_audio: true,
        };
        assert_eq!(both.input_count(), 2);

        // A blank mic id never counts as a source, even with system audio on.
        let blank_mic = AudioConfig {
            source: Some("  ".to_string()),
            system_audio: true,
        };
        assert_eq!(blank_mic.input_count(), 1);
    }

    #[test]
    fn only_genuine_cancels_get_the_neutral_message() {
        let cancelled = RecordingError::Portal("screen sharing was refused: cancelled".to_string());
        assert!(cancelled.is_cancelled());
        assert_eq!(cancelled.user_message(), "recording was cancelled");

        let broken = RecordingError::Portal("OpenPipeWireRemote failed: no such name".to_string());
        assert!(!broken.is_cancelled());
        assert!(broken.user_message().contains("no such name"));
    }

    #[test]
    fn errors_display_their_payload_and_kind() {
        assert_eq!(
            RecordingError::Ffmpeg("boom".to_string()).to_string(),
            "ffmpeg: boom"
        );
        assert!(
            RecordingError::PipeWire("x".to_string())
                .to_string()
                .starts_with("PipeWire: ")
        );
        assert!(
            !RecordingError::NotAvailable("ffmpeg".to_string())
                .to_string()
                .is_empty()
        );
    }

    #[test]
    fn io_errors_convert_into_recording_errors() {
        let error: RecordingError = std::io::Error::other("disk gone").into();
        assert!(matches!(error, RecordingError::Io(_)));
        assert!(error.to_string().contains("disk gone"));
    }

    #[test]
    fn a_spawn_with_a_bad_config_never_starts_a_thread() {
        let (tx, _rx) = mpsc::channel();
        let error = spawn_recording(config(RecordingFormat::Mp4, "/tmp/a.gif"), tx)
            .expect_err("extension mismatch");
        assert!(matches!(error, RecordingError::Unsupported(_)));
    }

    #[test]
    fn ffmpeg_is_available_on_this_host() {
        // Diagnostic for hosts that ship ffmpeg (the Linux CI gates job installs
        // it). On a runner without ffmpeg (e.g. Windows CI) this skips rather
        // than failing — the spawn tests below self-skip the same way.
        if !ffmpeg_available() {
            return;
        }
        assert!(ffmpeg_available(), "ffmpeg must be on PATH for this crate");
    }

    #[test]
    fn listing_audio_sources_never_fails() {
        let sources = list_audio_sources().expect("listing must degrade to empty");
        for source in &sources {
            assert!(!source.id.is_empty());
            assert!(!source.name.is_empty());
            assert!(!source.kind.is_empty());
        }
    }
}
