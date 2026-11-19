//! Interval capture → one PNG per tick ("timelapse" source frames).
//!
//! The live app wrote `SnagCraft-interval-*.png` from its own loop and had a
//! separate `make_timelapse` command that ran ffmpeg over whatever the loop left
//! on disk (`src-tauri/src/timelapse.rs`). The Rust engine owns the loop: a
//! dedicated thread takes a shot every `period_secs`, saves it as
//! `snagcraft-timelapse-NNNNN.png` in the configured folder and reports each
//! frame as a [`TimelapseEvent`]. Encoding the MP4 stays out of here — frames on
//! disk mean a failed render never costs the shots.
//!
//! Everything is bounded: the period is clamped to 1..=3600 s, the frame count
//! to 1..=10000, and the stop flag is polled every [`STOP_POLL`] so a stop never
//! waits out a long interval.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use snagcraft_raster::Surface;

use crate::{CaptureError, capture_full_blocking};

/// Shortest accepted interval (a hot loop of screenshots is never wanted).
pub const MIN_PERIOD_SECS: u32 = 1;
/// Longest accepted interval (one hour).
pub const MAX_PERIOD_SECS: u32 = 3_600;
/// Hard cap on frames in one run (`10000` PNGs is already gigabytes).
pub const MAX_FRAMES: u32 = 10_000;
/// Longest single sleep slice, so [`TimelapseHandle::stop`] lands promptly.
pub const STOP_POLL: Duration = Duration::from_millis(100);

/// W31: compositor settle after the UI hides the windows, before the grab
/// (the live `HIDE_SETTLE_MS`, commands.rs:41).
const HIDE_SETTLE: Duration = Duration::from_millis(220);
/// W31: how long to wait for the UI's hide ack before grabbing anyway — a
/// stuck UI must not stall the run (the frame may then contain the app).
const HIDE_ACK_TIMEOUT: Duration = Duration::from_secs(2);
/// W31: ack polling slice.
const HIDE_ACK_POLL: Duration = Duration::from_millis(20);

/// File stem of a frame; the index is zero-padded to five digits so
/// lexicographic order (what ffmpeg's `glob` pattern uses) is frame order.
const FRAME_PREFIX: &str = "snagcraft-timelapse-";

/// Glob matching exactly the frames [`frame_path`] writes in one folder — the
/// default `timelapse.assemble` pattern. Keep in step with [`FRAME_PREFIX`].
pub const FRAME_GLOB: &str = "snagcraft-timelapse-*.png";

/// What to capture and where to put it.
#[derive(Clone, Debug)]
pub struct TimelapseConfig {
    /// Seconds between shots; clamped to [`MIN_PERIOD_SECS`]..=[`MAX_PERIOD_SECS`].
    pub period_secs: u32,
    /// Stop after this many frames (`None` = until [`TimelapseHandle::stop`]).
    pub count: Option<u32>,
    /// Folder for the PNGs; created when missing.
    pub dir: PathBuf,
    /// W31: hide-ack handshake with the UI. When present, the loop emits
    /// [`TimelapseEvent::HideForFrame`] before each grab and waits for the UI
    /// to set this flag (the app's windows are then hidden), so timelapse
    /// frames never contain SnagCraft itself — live grabs every interval
    /// frame through `capture_screenshot`'s hide/settle/restore dance.
    pub hide_gate: Option<Arc<AtomicBool>>,
}

/// Progress reported to the caller's channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelapseEvent {
    /// One frame was written to disk.
    Frame { path: PathBuf, index: u32 },
    /// The run finished normally (stopped or count reached).
    Done { frames: u32 },
    /// The run stopped early; the string is user-readable.
    Failed(String),
    /// W31: hide the app's windows, then ack via the config's `hide_gate`.
    HideForFrame,
    /// W31: the grab is over — bring the app's windows back.
    RestoreAfterFrame,
}

/// Owning side of a running timelapse. Dropping it does **not** stop the run —
/// call [`TimelapseHandle::stop`] for that, so a caller can keep the handle in a
/// short-lived struct without killing the capture.
#[derive(Clone, Debug)]
pub struct TimelapseHandle {
    stop: Arc<AtomicBool>,
}

impl TimelapseHandle {
    /// Ask the loop to stop after the frame it is currently taking.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// True once [`TimelapseHandle::stop`] has been called.
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Clamp a requested interval into the supported range.
pub fn clamp_period(period_secs: u32) -> u32 {
    period_secs.clamp(MIN_PERIOD_SECS, MAX_PERIOD_SECS)
}

/// Clamp a requested frame count: `None` stays unlimited, zero becomes one
/// (a "timelapse" of no frames is never what the user asked for).
pub fn clamp_count(count: Option<u32>) -> Option<u32> {
    count.map(|frames| frames.clamp(1, MAX_FRAMES))
}

/// Path of frame `index` (1-based) inside `dir`.
pub fn frame_path(dir: &Path, index: u32) -> PathBuf {
    dir.join(format!("{FRAME_PREFIX}{index:05}.png"))
}

/// Start capturing every `config.period_secs` into `config.dir`.
///
/// The folder is created before the thread starts, so a bad path fails here
/// (as [`CaptureError::Io`]) rather than asynchronously. Frames are reported on
/// `events`; a dropped receiver stops the run.
pub fn spawn_timelapse(
    config: TimelapseConfig,
    events: Sender<TimelapseEvent>,
) -> Result<TimelapseHandle, CaptureError> {
    let period = Duration::from_secs(u64::from(clamp_period(config.period_secs)));
    let limit = clamp_count(config.count);
    let dir = config.dir;

    std::fs::create_dir_all(&dir)
        .map_err(|err| CaptureError::Io(format!("cannot create {}: {err}", dir.display())))?;

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let gate = config.hide_gate.clone();
    std::thread::Builder::new()
        .name("snagcraft-timelapse".to_string())
        .spawn(move || run(&dir, period, limit, &flag, &events, gate.as_ref()))
        .map_err(|err| CaptureError::Io(format!("cannot start the timelapse thread: {err}")))?;
    Ok(TimelapseHandle { stop })
}

/// The capture loop. Stops when `stop` is set, when the frame limit is reached,
/// when a capture/save fails, or when the receiver is gone.
///
/// W31: with a `gate`, every grab is wrapped in the hide-ack handshake —
/// `HideForFrame` → wait for the UI's ack (windows hidden) → settle → grab →
/// `RestoreAfterFrame`. The restore is ALWAYS sent once a hide was requested
/// (even on a late/missing ack or a failed grab): the UI's restore is
/// flag-guarded, and the channel order guarantees a late hide is undone.
fn run(
    dir: &Path,
    period: Duration,
    limit: Option<u32>,
    stop: &AtomicBool,
    events: &Sender<TimelapseEvent>,
    gate: Option<&Arc<AtomicBool>>,
) {
    let mut frames = 0u32;
    loop {
        if stop.load(Ordering::Relaxed) || limit.is_some_and(|max| frames >= max) {
            break;
        }

        // Hide the app's own windows for the grab (live parity).
        let mut asked_hide = false;
        if let Some(gate) = gate {
            if events.send(TimelapseEvent::HideForFrame).is_err() {
                return;
            }
            asked_hide = true;
            let acked = wait_for_hide_ack(gate, stop, HIDE_ACK_TIMEOUT);
            if stop.load(Ordering::Relaxed) {
                let _ = events.send(TimelapseEvent::RestoreAfterFrame);
                break;
            }
            if acked && !sleep_interruptible(HIDE_SETTLE, stop) {
                let _ = events.send(TimelapseEvent::RestoreAfterFrame);
                break;
            }
        }

        let index = frames + 1;
        let surface = match capture_full_blocking() {
            Ok(surface) => surface,
            Err(err) => {
                if asked_hide {
                    let _ = events.send(TimelapseEvent::RestoreAfterFrame);
                }
                let _ = events.send(TimelapseEvent::Failed(err.to_string()));
                return;
            }
        };
        if asked_hide {
            let _ = events.send(TimelapseEvent::RestoreAfterFrame);
        }

        let path = frame_path(dir, index);
        if let Err(message) = save_png(&surface, &path) {
            let _ = events.send(TimelapseEvent::Failed(message));
            return;
        }

        frames = index;
        // A closed channel means nobody is listening: stop holding the screen.
        if events.send(TimelapseEvent::Frame { path, index }).is_err() {
            return;
        }

        if limit.is_some_and(|max| frames >= max) {
            break;
        }
        if !sleep_interruptible(period, stop) {
            break;
        }
    }

    let _ = events.send(TimelapseEvent::Done { frames });
}

/// Wait until the UI acks the hide (flag set), consuming the ack for the next
/// frame. Returns false on stop or timeout — the caller grabs anyway then, so
/// a stuck UI degrades one frame instead of stalling the whole run.
fn wait_for_hide_ack(gate: &AtomicBool, stop: &AtomicBool, timeout: Duration) -> bool {
    let mut left = timeout;
    while !left.is_zero() {
        if gate.swap(false, Ordering::Relaxed) {
            return true;
        }
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let slice = left.min(HIDE_ACK_POLL);
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
    gate.swap(false, Ordering::Relaxed)
}

/// Write `surface` to `path` as PNG.
fn save_png(surface: &Surface, path: &Path) -> Result<(), String> {
    image::DynamicImage::ImageRgba8(surface.clone())
        .save(path)
        .map_err(|err| format!("cannot save {}: {err}", path.display()))
}

/// Sleep `total`, waking every [`STOP_POLL`] to check `stop`.
///
/// Returns `false` when the stop flag was seen (the caller must not sleep the
/// rest of the interval).
fn sleep_interruptible(total: Duration, stop: &AtomicBool) -> bool {
    let mut left = total;
    while !left.is_zero() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let slice = left.min(STOP_POLL);
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
    !stop.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::channel;

    fn tmp_dir(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("snagcraft-timelapse-{tag}-{stamp}"))
    }

    #[test]
    fn period_is_clamped() {
        assert_eq!(clamp_period(0), MIN_PERIOD_SECS);
        assert_eq!(clamp_period(1), 1);
        assert_eq!(clamp_period(30), 30);
        assert_eq!(clamp_period(3_600), MAX_PERIOD_SECS);
        assert_eq!(clamp_period(86_400), MAX_PERIOD_SECS);
    }

    #[test]
    fn count_is_clamped() {
        assert_eq!(clamp_count(None), None);
        assert_eq!(clamp_count(Some(0)), Some(1));
        assert_eq!(clamp_count(Some(12)), Some(12));
        assert_eq!(clamp_count(Some(MAX_FRAMES)), Some(MAX_FRAMES));
        assert_eq!(clamp_count(Some(u32::MAX)), Some(MAX_FRAMES));
    }

    #[test]
    fn frames_are_zero_padded_inside_the_dir() {
        let dir = PathBuf::from("/tmp/shots");
        assert_eq!(
            frame_path(&dir, 1),
            PathBuf::from("/tmp/shots/snagcraft-timelapse-00001.png")
        );
        assert_eq!(
            frame_path(&dir, 42),
            PathBuf::from("/tmp/shots/snagcraft-timelapse-00042.png")
        );
        // Lexicographic order equals frame order (ffmpeg globs sort by name).
        let mut sorted = vec![10u32, 2, 1, 9]
            .into_iter()
            .map(|i| frame_path(&dir, i))
            .collect::<Vec<_>>();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                frame_path(&dir, 1),
                frame_path(&dir, 2),
                frame_path(&dir, 9),
                frame_path(&dir, 10)
            ]
        );
    }

    #[test]
    fn the_default_glob_matches_every_frame_name() {
        // `timelapse.assemble` defaults to FRAME_GLOB; it must select exactly
        // what the capture loop writes and nothing else in the folder.
        assert_eq!(FRAME_GLOB, format!("{FRAME_PREFIX}*.png"));
        let name = frame_path(Path::new("/tmp/shots"), 7)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (prefix, suffix) = name.split_once("00007").unwrap_or(("", ""));
        assert_eq!(format!("{prefix}*{suffix}"), FRAME_GLOB);
    }

    #[test]
    fn handle_stop_sets_the_flag() {
        let handle = TimelapseHandle {
            stop: Arc::new(AtomicBool::new(false)),
        };
        assert!(!handle.is_stopped());
        handle.stop();
        assert!(handle.is_stopped());
        // Idempotent: a second stop is harmless.
        handle.stop();
        assert!(handle.is_stopped());
    }

    #[test]
    fn sleep_wakes_on_stop_without_waiting_the_interval() {
        let stop = Arc::new(AtomicBool::new(true));
        let start = std::time::Instant::now();
        assert!(!sleep_interruptible(Duration::from_secs(3600), &stop));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn sleep_completes_and_stays_running() {
        let stop = AtomicBool::new(false);
        let start = std::time::Instant::now();
        assert!(sleep_interruptible(Duration::from_millis(150), &stop));
        assert!(start.elapsed() >= Duration::from_millis(140));
    }

    /// A pre-set stop flag must make the loop return immediately with
    /// `Done { frames: 0 }` — and never touch the capture portal.
    #[test]
    fn loop_honours_a_preset_stop() {
        let dir = tmp_dir("preset-stop");
        let (tx, rx) = channel();
        let stop = AtomicBool::new(true);
        run(&dir, Duration::from_secs(60), Some(5), &stop, &tx, None);
        drop(tx);

        let events: Vec<TimelapseEvent> = rx.iter().collect();
        assert_eq!(events, vec![TimelapseEvent::Done { frames: 0 }]);
        assert!(!dir.exists(), "no frame means no folder written");
    }

    /// A satisfied frame budget exits before the first capture. `clamp_count`
    /// never produces `0`, but the loop's own bound must still hold, so the
    /// capture portal is never reached with a zero budget.
    #[test]
    fn loop_stops_at_the_frame_limit() {
        let dir = tmp_dir("limit");
        let (tx, rx) = channel();
        let stop = AtomicBool::new(false);
        run(&dir, Duration::from_secs(60), Some(0), &stop, &tx, None);
        drop(tx);
        let events: Vec<TimelapseEvent> = rx.iter().collect();
        assert_eq!(events, vec![TimelapseEvent::Done { frames: 0 }]);
    }

    /// W31: a preset stop beats the handshake — no hide events, no portal.
    #[test]
    fn gated_loop_still_honours_a_preset_stop() {
        let dir = tmp_dir("gate-stop");
        let (tx, rx) = channel();
        let stop = AtomicBool::new(true);
        let gate = Arc::new(AtomicBool::new(false));
        run(
            &dir,
            Duration::from_secs(60),
            Some(5),
            &stop,
            &tx,
            Some(&gate),
        );
        drop(tx);
        let events: Vec<TimelapseEvent> = rx.iter().collect();
        assert_eq!(events, vec![TimelapseEvent::Done { frames: 0 }]);
    }

    /// W31: with nobody listening the gated loop exits at the first hide
    /// event — it never reaches the capture portal.
    #[test]
    fn gated_loop_exits_when_nobody_listens() {
        let dir = tmp_dir("gate-dropped");
        let (tx, rx) = channel();
        drop(rx);
        let stop = AtomicBool::new(false);
        let gate = Arc::new(AtomicBool::new(false));
        run(&dir, Duration::from_secs(60), None, &stop, &tx, Some(&gate));
        assert!(!dir.exists(), "no hide ack means no frame was taken");
    }

    #[test]
    fn events_compare_by_content() {
        let a = TimelapseEvent::Frame {
            path: PathBuf::from("/tmp/a.png"),
            index: 3,
        };
        let b = TimelapseEvent::Frame {
            path: PathBuf::from("/tmp/a.png"),
            index: 3,
        };
        assert_eq!(a, b);
        assert_ne!(a, TimelapseEvent::Done { frames: 3 });
        assert!(matches!(
            TimelapseEvent::Failed("no portal".to_string()),
            TimelapseEvent::Failed(_)
        ));
    }

    /// A folder that cannot be created fails at the call site, not on the
    /// worker thread. (A successful spawn is deliberately *not* tested: it would
    /// immediately ask the real screenshot portal for a frame, which needs an
    /// unlocked desktop session.)
    #[test]
    fn spawn_rejects_an_impossible_folder() {
        let (tx, _rx) = channel();
        // A directory nested under a regular file can never be created — on any
        // platform (the old `/proc/...` fixture only failed on Linux).
        let file = std::env::temp_dir().join(format!(
            "snagcraft-tl-impossible-{}.txt",
            std::process::id()
        ));
        std::fs::write(&file, b"x").expect("fixture file");
        let config = TimelapseConfig {
            period_secs: 5,
            count: None,
            dir: file.join("not-a-dir"),
            hide_gate: None,
        };
        let result = spawn_timelapse(config, tx);
        let _ = std::fs::remove_file(&file);
        assert!(result.is_err());
    }

    #[test]
    fn hide_ack_is_consumed_for_the_next_frame() {
        let gate = AtomicBool::new(true);
        let stop = AtomicBool::new(false);
        assert!(wait_for_hide_ack(&gate, &stop, Duration::from_millis(50)));
        assert!(
            !gate.load(Ordering::Relaxed),
            "the ack is consumed so the next frame waits for a fresh one"
        );
    }

    #[test]
    fn hide_ack_waits_for_a_late_ui() {
        let gate = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gate);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            g.store(true, Ordering::Relaxed);
        });
        assert!(wait_for_hide_ack(&gate, &stop, Duration::from_millis(500)));
    }

    #[test]
    fn hide_ack_times_out_without_stalling_the_run() {
        let gate = AtomicBool::new(false);
        let stop = AtomicBool::new(false);
        let t0 = std::time::Instant::now();
        assert!(!wait_for_hide_ack(&gate, &stop, Duration::from_millis(60)));
        assert!(t0.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn hide_ack_aborts_on_stop() {
        let gate = AtomicBool::new(false);
        let stop = AtomicBool::new(true);
        let t0 = std::time::Instant::now();
        assert!(!wait_for_hide_ack(&gate, &stop, Duration::from_secs(2)));
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "stop lands promptly"
        );
    }
}
