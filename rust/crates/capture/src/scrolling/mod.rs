//! `capture::scrolling` — manual-scroll long capture.
//!
//! The user selects a region, SnagCraft casts that monitor through the
//! **ScreenCast** portal, and while the user scrolls the newest frame is aligned
//! against the last accepted one; only the rows a scroll revealed are appended.
//! One tall PNG comes out. No synthetic input, no new permission, no ffmpeg.
//!
//! Layout of this module:
//!
//! * [`align`] — pure row-signature + MAD aligner ([`row_signature`],
//!   [`align_offset`]).
//! * [`stitch`] — pure [`stitch_vertical`] plus the streaming [`Stitcher`].
//! * `source` (Linux) — ScreenCast portal → PipeWire → newest-frame slot, with a
//!   dedicated thread because `MainLoopRc` is `!Send`.
//!
//! Threading and teardown order (the part that is easy to get wrong): this module
//! owns a **stitch thread** that polls the newest frame, and the source owns a
//! **PipeWire thread**. Stop the stitch thread first, then the PipeWire thread,
//! then drop the portal `Session` — dropping the session first ends the cast under
//! the capture thread's feet.
//!
//! Stop rules: the user calling [`ScrollHandle::stop`], the max height cap, or the
//! content going quiet (two unchanged frames, or ~2.5 s without progress). A stop
//! is *graceful*: what was captured is stitched, saved and reported as
//! [`ScrollEvent::Done`].

pub mod align;
pub mod stitch;

#[cfg(target_os = "linux")]
mod source;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use snagcraft_geom::Rect;
use snagcraft_raster::{Surface, dims_ok};

use crate::CaptureError;

pub use align::{OffsetMatch, align_offset, row_signature};
pub use stitch::{MAX_HEIGHT, stitch_vertical};

/// How often the stitch thread looks at the newest frame (~14 fps). The portal
/// runs at monitor rate, so this is a cheap poll, not a capture rate.
const POLL_INTERVAL: Duration = Duration::from_millis(70);

/// After this long without new content, the capture stops on its own (Shottr's
/// heuristic). Only armed once the user has actually scrolled.
const IDLE_STOP: Duration = Duration::from_millis(2_500);

/// Consecutive unchanged frames that also end a capture.
const STALL_LIMIT: u32 = 2;

/// Smallest usable region: below this neither the aligner nor the user wins.
pub const MIN_REGION_PX: u32 = 32;

/// What to capture and where to put it.
#[derive(Clone, Debug)]
pub struct ScrollConfig {
    /// The region on screen, in **stream (physical) pixels** — not CSS px:
    /// fractional scaling makes the two differ, and the portal reports physical
    /// pixels.
    pub region: Rect,
    /// Height ceiling, clamped to `1..=40000`.
    pub max_height: u32,
    /// Where the stitched PNG is written.
    pub output: PathBuf,
}

impl ScrollConfig {
    /// The region clamped to a stream of `width × height` pixels, as
    /// `(x, y, w, h)`. `None` when nothing usable is left.
    fn region_px(&self, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
        let (x0, y0, x1, y1) = self.region.clamp_to(width, height)?;
        let (w, h) = (x1 - x0, y1 - y0);
        if w < MIN_REGION_PX || h < MIN_REGION_PX || !dims_ok(w, h) {
            return None;
        }
        Some((x0, y0, w, h))
    }

    /// The effective height cap (hostile configs included).
    fn height_cap(&self) -> u32 {
        self.max_height.clamp(1, MAX_HEIGHT)
    }
}

/// Progress and terminal states. Every `spawn_scrolling` call ends with exactly
/// one of [`ScrollEvent::Done`], [`ScrollEvent::Failed`], [`ScrollEvent::Stopped`]
/// or [`ScrollEvent::Cancelled`], after which the channel is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScrollEvent {
    /// A frame contributed rows. `index` counts accepted frames, `total_height`
    /// is the stitched height so far in pixels.
    Captured { index: u32, total_height: u32 },
    /// The capture finished and the image was written.
    Done {
        path: PathBuf,
        width: u32,
        height: u32,
    },
    /// Something went wrong; nothing was written.
    Failed(String),
    /// The capture was stopped before it captured anything.
    Stopped,
    /// W20: the user cancelled — the partial scroll is discarded, nothing is
    /// written (the live card's Cancel button, `scrolling_cancel`).
    Cancelled,
}

/// A running capture. Dropping it stops nothing on its own — call [`stop`] so the
/// partial scroll is stitched and saved.
///
/// [`stop`]: ScrollHandle::stop
pub struct ScrollHandle {
    stop: Arc<AtomicBool>,
    /// W20: discard the partial scroll instead of stitching + saving it.
    cancel: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl ScrollHandle {
    /// Ask the capture to stop and wait for it to finish.
    ///
    /// Graceful by contract: the capture loop stitches what it has, writes the
    /// PNG and sends [`ScrollEvent::Done`] (or [`ScrollEvent::Stopped`] when
    /// nothing was captured) before this returns.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let handle = match self.join.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// W20: stop AND discard — the partial scroll is not stitched or saved.
    /// Ends with [`ScrollEvent::Cancelled`] (the live card's Cancel button).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.stop();
    }
}

impl Drop for ScrollHandle {
    fn drop(&mut self) {
        // A dropped handle must not leave a thread holding a portal session.
        self.stop();
    }
}

/// Start a scrolling capture.
///
/// Returns once the portal cast is live and frames are flowing, so a refused or
/// unanswered share dialog is an error here rather than a capture that silently
/// never fills. Progress arrives on `events`; the source is a monitor/window
/// cast (`CursorMode::HIDDEN`, never a rectangle), and the region is cropped
/// in-app.
///
/// Fails immediately for a bad config — including an output path with no parent
/// directory — so nothing is opened that would have to be torn down again.
pub fn spawn_scrolling(
    config: ScrollConfig,
    events: std::sync::mpsc::Sender<ScrollEvent>,
) -> Result<ScrollHandle, CaptureError> {
    if !config.region.is_finite() {
        return Err(CaptureError::Unsupported(
            "the capture region is not a finite rectangle".to_string(),
        ));
    }
    if config.region.w < MIN_REGION_PX as f32 || config.region.h < MIN_REGION_PX as f32 {
        return Err(CaptureError::Unsupported(format!(
            "the capture region needs at least {MIN_REGION_PX}x{MIN_REGION_PX} px"
        )));
    }
    ensure_output_dir(&config.output)?;

    #[cfg(target_os = "linux")]
    {
        linux::spawn(config, events)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, events);
        Err(CaptureError::Unsupported(
            "scrolling capture is only implemented on Linux/Wayland".to_string(),
        ))
    }
}

/// Make sure the output's directory exists — a save that fails at the very end
/// of a long capture is the worst possible time to discover a missing folder.
fn ensure_output_dir(path: &Path) -> Result<(), CaptureError> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent)
            .map_err(|err| CaptureError::Io(format!("cannot create {}: {err}", parent.display()))),
        _ => Ok(()),
    }
}

/// Linux: open the portal cast, then drive the stitcher on its own thread.
#[cfg(target_os = "linux")]
mod linux {
    use super::stitch::{Push, Stitcher};
    use super::{
        CaptureError, IDLE_STOP, MIN_REGION_PX, POLL_INTERVAL, STALL_LIMIT, ScrollConfig,
        ScrollEvent, ScrollHandle, Surface, source,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// Region in stream pixels: `(x, y, width, height)`.
    type RegionPx = (u32, u32, u32, u32);

    pub(super) fn spawn(
        config: ScrollConfig,
        events: mpsc::Sender<ScrollEvent>,
    ) -> Result<ScrollHandle, CaptureError> {
        // Opening the portal blocks on the share dialog (up to 120 s) and on the
        // first frame, so this is documented as a blocking call: run it off the
        // UI thread.
        let source = source::open_blocking()?;
        let (stream_w, stream_h) = source.size();
        let (stream_w, stream_h) = (stream_w.max(0) as u32, stream_h.max(0) as u32);

        let Some(region) = config.region_px(stream_w, stream_h) else {
            let mut source = source;
            source.shutdown();
            return Err(CaptureError::Portal(format!(
                "the selected region does not fit a usable area inside the \
                 {stream_w}x{stream_h} stream"
            )));
        };

        let cap = config.height_cap();
        let out = config.output.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);

        let thread = std::thread::Builder::new()
            .name("snagcraft-scrolling-stitch".to_string())
            .spawn(move || run(source, region, cap, out, thread_stop, thread_cancel, events))
            .map_err(|err| {
                CaptureError::Io(format!("cannot start the scrolling stitch thread: {err}"))
            })?;

        Ok(ScrollHandle {
            stop,
            cancel,
            join: Mutex::new(Some(thread)),
        })
    }

    /// The stitch thread body: capture until a stop rule fires, then save.
    fn run(
        mut source: source::Source,
        region: RegionPx,
        cap: u32,
        out: PathBuf,
        stop: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        events: mpsc::Sender<ScrollEvent>,
    ) {
        let outcome = capture_loop(&source, region, cap, &stop, &events);

        // Teardown order: the PipeWire thread first (it owns the buffers), then
        // the portal session — dropping the session earlier would end the cast
        // under the capture thread.
        source.shutdown();

        // W20: a cancelled run discards the partial scroll — nothing is written.
        if cancel.load(Ordering::SeqCst) {
            let _ = events.send(ScrollEvent::Cancelled);
            return;
        }

        match outcome {
            Ok(surface) if surface.width() > 0 && surface.height() > 0 => {
                let (width, height) = surface.dimensions();
                let saved = save(surface, &out);
                let event = match saved {
                    Ok(()) => ScrollEvent::Done {
                        path: out,
                        width,
                        height,
                    },
                    Err(err) => {
                        tracing::warn!(error = %err, "scrolling capture could not be saved");
                        ScrollEvent::Failed(err.to_string())
                    }
                };
                let _ = events.send(event);
            }
            Ok(_) => {
                // Stopped before a single row was captured: nothing to save.
                let _ = events.send(ScrollEvent::Stopped);
            }
            Err(err) => {
                tracing::warn!(error = %err, "scrolling capture failed");
                let _ = events.send(ScrollEvent::Failed(err.to_string()));
            }
        }
        // `events` drops here: the channel closes exactly once, after the terminal
        // event.
    }

    /// Poll the newest frame, crop the region, offer it to the stitcher, and emit
    /// progress until one of the stop rules fires.
    fn capture_loop(
        source: &source::Source,
        region: RegionPx,
        cap: u32,
        stop: &AtomicBool,
        events: &mpsc::Sender<ScrollEvent>,
    ) -> Result<Surface, CaptureError> {
        let (x, y, w, h) = region;
        let mut stitcher = Stitcher::new(w, h, cap);
        // Identity of the last frame seen: same pointer means the screen did not
        // change, which is what the idle rule watches.
        let mut last_seen: Option<*const Surface> = None;
        let mut last_progress = Instant::now();
        let mut armed = false;
        let mut stalled = 0u32;
        let mut reason = "user stop";

        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(POLL_INTERVAL);

            let Some(frame) = source.newest() else {
                continue;
            };
            let identity = Arc::as_ptr(&frame);
            if last_seen == Some(identity) {
                if armed && last_progress.elapsed() >= IDLE_STOP {
                    reason = "idle";
                    break;
                }
                continue;
            }
            last_seen = Some(identity);

            let Some(cropped) = crop(&frame, x, y, w, h) else {
                continue;
            };

            match stitcher.push(&cropped) {
                Push::Accepted { new_rows } => {
                    if new_rows > 0 && stitcher.rows() > h {
                        // The first real scroll arms the auto-stop rules; before
                        // that the user is still framing the shot.
                        armed = true;
                        last_progress = Instant::now();
                        stalled = 0;
                    }
                    let _ = events.send(ScrollEvent::Captured {
                        index: stitcher.frames() as u32,
                        total_height: stitcher.rows(),
                    });
                }
                Push::NoChange => {
                    if armed {
                        stalled += 1;
                        if stalled >= STALL_LIMIT || last_progress.elapsed() >= IDLE_STOP {
                            reason = "no new content";
                            break;
                        }
                    }
                }
                Push::LowConfidence => {
                    // Never append garbage: keep what we have and let the next
                    // frame try again (the UI shows "slow down" from Missing data).
                    stalled = 0;
                    tracing::debug!("scrolling frame not matched — scroll slower");
                }
                Push::Capped => {
                    reason = "height cap";
                    break;
                }
            }
        }

        tracing::debug!(
            reason,
            frames = stitcher.frames(),
            height = stitcher.rows(),
            "scrolling capture ending"
        );
        Ok(stitcher.into_surface())
    }

    /// Crop the region out of a full-monitor frame.
    ///
    /// Exact size only: a region that does not fit is refused up front, so a
    /// differently sized crop here means a stream resize, which the stitcher
    /// cannot align.
    fn crop(frame: &Surface, x: u32, y: u32, w: u32, h: u32) -> Option<Surface> {
        let (frame_w, frame_h) = frame.dimensions();
        if w < MIN_REGION_PX || h < MIN_REGION_PX || x >= frame_w || y >= frame_h {
            return None;
        }
        if frame_w - x < w || frame_h - y < h {
            return None;
        }
        Some(image::imageops::crop_imm(frame, x, y, w, h).to_image())
    }

    /// Encode the stitched surface as one image file.
    ///
    /// Format follows the extension (`png` by default): the app's normal
    /// auto-save/history path takes it from there.
    fn save(surface: Surface, path: &Path) -> Result<(), CaptureError> {
        let format = image::ImageFormat::from_path(path).unwrap_or(image::ImageFormat::Png);
        let file = std::fs::File::create(path)
            .map_err(|err| CaptureError::Io(format!("cannot create {}: {err}", path.display())))?;
        let mut writer = std::io::BufWriter::new(file);
        image::DynamicImage::ImageRgba8(surface)
            .write_to(&mut writer, format)
            .map_err(|err| CaptureError::Io(format!("cannot encode {}: {err}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagcraft_geom::Rect;
    use std::sync::mpsc;

    fn config(region: Rect, max_height: u32, output: &str) -> ScrollConfig {
        ScrollConfig {
            region,
            max_height,
            output: PathBuf::from(output),
        }
    }

    #[test]
    fn the_height_cap_is_clamped_to_the_public_limit() {
        assert_eq!(
            config(Rect::new(0.0, 0.0, 400.0, 400.0), 0, "a.png").height_cap(),
            1
        );
        assert_eq!(
            config(Rect::new(0.0, 0.0, 400.0, 400.0), u32::MAX, "a.png").height_cap(),
            MAX_HEIGHT
        );
        assert_eq!(
            config(Rect::new(0.0, 0.0, 400.0, 400.0), 8_000, "a.png").height_cap(),
            8_000
        );
    }

    #[test]
    fn a_region_is_clipped_to_the_stream_and_has_to_stay_usable() {
        let wide = Rect::new(100.0, 100.0, 500.0, 500.0);
        assert_eq!(
            config(wide, MAX_HEIGHT, "a.png").region_px(400, 300),
            Some((100, 100, 300, 200))
        );
        // Entirely outside the stream, or too small to align: no usable region.
        let outside = Rect::new(900.0, 900.0, 200.0, 200.0);
        assert_eq!(
            config(outside, MAX_HEIGHT, "a.png").region_px(400, 300),
            None
        );
        let tiny = Rect::new(0.0, 0.0, 8.0, 8.0);
        assert_eq!(
            config(tiny, MAX_HEIGHT, "a.png").region_px(1_920, 1_080),
            None
        );
    }

    #[test]
    fn a_bad_config_fails_before_any_portal_call() {
        let (tx, _rx) = mpsc::channel();
        let tiny = config(Rect::new(0.0, 0.0, 8.0, 400.0), MAX_HEIGHT, "a.png");
        assert!(matches!(
            spawn_scrolling(tiny, tx.clone()),
            Err(CaptureError::Unsupported(_))
        ));

        let hostile = config(Rect::new(f32::NAN, 0.0, 400.0, 400.0), MAX_HEIGHT, "a.png");
        assert!(matches!(
            spawn_scrolling(hostile, tx),
            Err(CaptureError::Unsupported(_))
        ));
    }

    #[test]
    fn the_output_directory_is_created_up_front() {
        let dir = std::env::temp_dir().join(format!("snagcraft-scrolling-{}", std::process::id()));
        let file = dir.join("nested").join("capture.png");
        let _ = std::fs::remove_dir_all(&dir);
        ensure_output_dir(&file).expect("the parent must be created");
        assert!(file.parent().is_some_and(|parent| parent.is_dir()));
        // A path with no parent at all is left to the save step.
        assert!(ensure_output_dir(Path::new("capture.png")).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stopping_a_handle_is_graceful_and_idempotent() {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            // A capture loop reports its terminal event before it exits.
            let _ = tx.send(ScrollEvent::Done {
                path: PathBuf::from("/tmp/x.png"),
                width: 10,
                height: 20,
            });
        });
        let handle = ScrollHandle {
            stop,
            cancel: Arc::new(AtomicBool::new(false)),
            join: Mutex::new(Some(thread)),
        };
        handle.stop();
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(ScrollEvent::Done { width, height, .. }) => assert_eq!((width, height), (10, 20)),
            other => panic!("expected one Done event, got {other:?}"),
        }
        // A second stop finds no thread and must not hang or panic.
        handle.stop();
    }

    /// W20: cancel sets both flags so the loop exits promptly and discards.
    #[test]
    fn cancel_marks_the_handle_for_discard() {
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = ScrollHandle {
            stop: Arc::clone(&stop),
            cancel: Arc::clone(&cancel),
            join: Mutex::new(None),
        };
        assert!(!stop.load(Ordering::SeqCst));
        assert!(!cancel.load(Ordering::SeqCst));
        handle.cancel();
        assert!(
            stop.load(Ordering::SeqCst),
            "cancel must also stop the loop"
        );
        assert!(cancel.load(Ordering::SeqCst), "cancel must mark discard");
    }

    #[test]
    fn events_carry_their_payload() {
        let done = ScrollEvent::Done {
            path: PathBuf::from("/tmp/a.png"),
            width: 1_200,
            height: 4_000,
        };
        assert_eq!(done.clone(), done);
        assert_eq!(
            ScrollEvent::Captured {
                index: 3,
                total_height: 900
            },
            ScrollEvent::Captured {
                index: 3,
                total_height: 900
            }
        );
        assert_ne!(ScrollEvent::Stopped, ScrollEvent::Failed("boom".into()));
    }

    /// Live portal round trip. Ignored by default: it needs an unlocked desktop
    /// session, a real xdg-desktop-portal, and a **human who scrolls** — the share
    /// dialog has to be confirmed and then something on screen scrolled while the
    /// test waits.
    ///
    /// Run it from inside the session:
    /// `cargo test -p snagcraft-capture -- --ignored scrolling_live`
    ///
    /// Flow: confirm the share dialog, scroll a long page for ~6 s, then leave it
    /// still; the explicit stop ends the capture and the stitched PNG must exist.
    #[cfg(target_os = "linux")]
    #[ignore = "needs an unlocked desktop session, a live portal and manual scrolling"]
    #[test]
    fn scrolling_live_capture() {
        let out = std::env::temp_dir().join("snagcraft-scrolling-live.png");
        let _ = std::fs::remove_file(&out);
        let config = ScrollConfig {
            // Clipped to the stream, so any monitor size works.
            region: Rect::new(200.0, 200.0, 900.0, 700.0),
            max_height: MAX_HEIGHT,
            output: out.clone(),
        };
        let (tx, rx) = mpsc::channel();
        let handle = spawn_scrolling(config, tx).expect("the portal cast must start");

        // Give the human time to scroll, then stop: a stop must still stitch and
        // save what was captured.
        std::thread::sleep(Duration::from_secs(6));
        handle.stop();

        let mut last = None;
        for event in rx.iter() {
            last = Some(event);
        }
        match last {
            Some(ScrollEvent::Done {
                path,
                width,
                height,
            }) => {
                assert_eq!(path, out);
                assert!(width > 0 && height > 0);
                assert!(out.exists(), "the stitched PNG must be on disk");
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }
}
