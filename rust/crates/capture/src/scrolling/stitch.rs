//! Vertical stitching: pure placement of aligned frames on one tall canvas.
//!
//! Two entry points, one rule set:
//!
//! * [`stitch_vertical`] is pure — given frames and their offsets it lays them
//!   out on a single canvas. The offline tests use it to prove that recovered
//!   offsets reproduce the original tall image.
//! * [`Stitcher`] is the streaming form the capture loop drives: it keeps one
//!   reference frame, appends only the rows a scroll actually revealed, and
//!   refuses frames it cannot align.
//!
//! Rules encoded here (they are what makes the seams invisible):
//! * only **new** rows are added — a matched offset of `d` contributes `d` rows,
//!   never the whole overlap;
//! * a frame whose new rows are already at that edge of the accumulation (the
//!   user scrolled down and back up) adds nothing, so a wiggle cannot duplicate a
//!   band;
//! * a failed match never replaces the reference frame, so one bad frame does not
//!   poison the frames after it;
//! * the canvas is capped at [`MAX_HEIGHT`] rows — beyond that the capture is
//!   saved and the user is told to do a second pass.

use snagcraft_raster::Surface;

/// Hard cap on the stitched height, in pixels.
///
/// Same public limit Shottr uses. At 4 B/px (RGBA) a 4K-wide 40 000 px capture is
/// ~614 MB, which is why the runtime refuses rather than silently truncating.
pub const MAX_HEIGHT: u32 = 40_000;

/// Rows a `Surface` contributes, or 0 for an empty one.
fn rows_of(surface: &Surface) -> u32 {
    surface.height()
}

/// Lay `frames` out vertically using `offsets`.
///
/// `offsets[i]` is the alignment of frame `i` against frame `i − 1` (positive =
/// content moved up, i.e. the user scrolled down); `offsets[0]` is ignored.
/// Missing or non-finite offsets are treated as `0` (no movement), so a short
/// slice cannot shift the layout into nonsense.
///
/// The canvas is the bounding box of all frames, top-aligned at the first
/// frame's top after normalising reverse scrolls, and clipped to [`MAX_HEIGHT`]
/// rows. Overlaps are overwritten by the newer frame — a single consistent rule,
/// and lossless when the offsets are exact.
///
/// An empty input returns an empty (0×0) surface; frames whose width differs from
/// the first one are skipped rather than sheared.
pub fn stitch_vertical(frames: &[Surface], offsets: &[i32]) -> Surface {
    let (Some(first), Some(width)) = (frames.first(), frames.first().map(Surface::width)) else {
        return Surface::new(0, 0);
    };
    if width == 0 || rows_of(first) == 0 {
        return Surface::new(0, 0);
    }

    // Top of every frame on the canvas, in i64 so a hostile offset list cannot
    // wrap. A positive offset means this frame sits `offset` rows *lower* than the
    // previous one (its content moved up: the user scrolled down).
    let mut tops: Vec<i64> = Vec::with_capacity(frames.len());
    let mut current = 0i64;
    for (index, _) in frames.iter().enumerate() {
        if index > 0 {
            let shift = offsets.get(index).copied().unwrap_or(0) as i64;
            current = current.saturating_add(shift.clamp(-(MAX_HEIGHT as i64), MAX_HEIGHT as i64));
        }
        tops.push(current);
    }

    let top = tops.iter().copied().min().unwrap_or(0).min(0);
    let bottom = frames
        .iter()
        .zip(&tops)
        .map(|(frame, y)| y.saturating_add(rows_of(frame) as i64))
        .max()
        .unwrap_or(0);
    let span = bottom.saturating_sub(top).clamp(0, MAX_HEIGHT as i64) as u32;
    if span == 0 {
        return Surface::new(0, 0);
    }

    let mut canvas = Surface::new(width, span);
    for (frame, y) in frames.iter().zip(&tops) {
        if frame.width() != width || rows_of(frame) == 0 {
            continue;
        }
        let dest_y = y.saturating_sub(top);
        if dest_y >= span as i64 {
            continue;
        }
        // `imageops::replace` clips at the canvas edge, so a frame straddling the
        // height cap contributes its visible rows and no more.
        image::imageops::replace(&mut canvas, frame, 0, dest_y);
    }
    canvas
}

/// Rows still available under the cap, so an accepted scroll can be trimmed
/// instead of the frame being dropped.
pub fn room(rows: u32, max_height: u32) -> u32 {
    max_height.min(MAX_HEIGHT).saturating_sub(rows)
}

/// Outcome of offering one frame to the streaming [`Stitcher`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Push {
    /// Rows were appended (or prepended, for a reverse scroll).
    Accepted { new_rows: u32 },
    /// Nothing moved, or the rows were already captured: nothing to add.
    NoChange,
    /// The frames could not be aligned confidently: nothing added, warn.
    LowConfidence,
    /// The height cap was reached: stop the capture.
    Capped,
}

/// Accumulates a scroll into one tall RGBA buffer, one frame at a time.
pub(crate) struct Stitcher {
    width: u32,
    height: u32,
    /// `width × rows`, tightly packed RGBA — a byte buffer rather than a
    /// `Surface` because both edges grow and `Vec::splice` is the cheap move.
    acc: Vec<u8>,
    rows: u32,
    /// Reference frame: the last one that was *accepted*.
    last: Option<Surface>,
    frames: u64,
    max_height: u32,
}

impl Stitcher {
    /// `width × height` is the size of every frame that will be pushed (the
    /// cropped region, in stream pixels); `max_height` is already clamped.
    pub(crate) fn new(width: u32, height: u32, max_height: u32) -> Self {
        Self {
            width,
            height,
            acc: Vec::new(),
            rows: 0,
            last: None,
            frames: 0,
            max_height: max_height.min(MAX_HEIGHT),
        }
    }

    /// Offer one cropped frame.
    pub(crate) fn push(&mut self, next: &Surface) -> Push {
        if self.width == 0 || self.height == 0 || next.dimensions() != (self.width, self.height) {
            return Push::LowConfidence;
        }

        // First frame: it is the anchor, and it is trusted as-is.
        if self.acc.is_empty() {
            if self.height > self.max_height {
                return Push::Capped;
            }
            self.acc.extend_from_slice(next.as_raw());
            self.rows = self.height;
            self.last = Some(next.clone());
            self.frames = 1;
            return Push::Accepted {
                new_rows: self.height,
            };
        }

        if self
            .last
            .as_ref()
            .is_some_and(|last| last.as_raw() == next.as_raw())
        {
            return Push::NoChange;
        }

        let Some(reference) = self.last.as_ref() else {
            // An anchor that vanished cannot happen through the public API, but
            // adopting the new frame keeps the capture progressing if it did.
            self.last = Some(next.clone());
            return Push::NoChange;
        };

        let Some(offset) = super::align::align_offset(reference, next, self.height) else {
            // Keep the old reference: one unmatched frame must not poison the
            // frames after it.
            return Push::LowConfidence;
        };

        if offset == 0 {
            self.last = Some(next.clone());
            return Push::NoChange;
        }

        let moved = offset.unsigned_abs();
        if moved > room(self.rows, self.max_height) {
            return Push::Capped;
        }

        let row_bytes = self.width as usize * 4;
        let moved_bytes = moved as usize * row_bytes;
        let raw = next.as_raw();

        if offset > 0 {
            // Content moved up: the bottom `moved` rows are new — unless the user
            // already scrolled through them and came back, in which case the tail
            // of the accumulation is exactly those rows.
            let fresh = &raw[(self.height - moved) as usize * row_bytes..];
            let already =
                self.acc.len() >= moved_bytes && self.acc[self.acc.len() - moved_bytes..] == *fresh;
            self.last = Some(next.clone());
            if already {
                return Push::NoChange;
            }
            self.acc.extend_from_slice(fresh);
        } else {
            // Reverse scroll: new rows arrive at the top.
            let head = &raw[..moved_bytes];
            let already = self.acc.len() >= moved_bytes && self.acc[..moved_bytes] == *head;
            self.last = Some(next.clone());
            if already {
                return Push::NoChange;
            }
            self.acc.splice(0..0, head.iter().copied());
        }

        self.rows = self.rows.saturating_add(moved).min(self.max_height);
        self.frames += 1;
        Push::Accepted { new_rows: moved }
    }

    /// Stitched height so far, in rows.
    pub(crate) fn rows(&self) -> u32 {
        self.rows
    }

    /// Frames that contributed rows.
    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }

    /// The accumulated image (empty 0×0 when nothing was captured).
    pub(crate) fn into_surface(self) -> Surface {
        if self.width == 0 || self.rows == 0 {
            return Surface::new(0, 0);
        }
        Surface::from_raw(self.width, self.rows, self.acc).unwrap_or_else(|| Surface::new(0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrolling::align::align_offset;
    use crate::scrolling::align::synth::{flat, tall_page, window};

    const W: u32 = 320;
    const H: u32 = 500;

    /// Cut a viewport-sized window out of `page` at each top, then recover the
    /// offsets the way the capture loop does: frame by frame, against the previous
    /// accepted frame only (no peeking at the truth).
    fn frames_and_offsets(page: &Surface, tops: &[u32]) -> (Vec<Surface>, Vec<i32>) {
        let frames: Vec<Surface> = tops.iter().map(|&top| window(page, top, H)).collect();
        let mut offsets = vec![0i32];
        for pair in frames.windows(2) {
            let found = align_offset(&pair[0], &pair[1], H)
                .unwrap_or_else(|| panic!("frames must align at {:?}", tops));
            offsets.push(found);
        }
        (frames, offsets)
    }

    /// Assert `surface` equals the first `surface.height()` rows of `page`.
    fn assert_matches_page(surface: &Surface, page: &Surface) {
        let rows = surface.height() as usize * W as usize * 4;
        assert_eq!(surface.width(), W, "width must survive stitching");
        assert_eq!(
            surface.as_raw().as_slice(),
            &page.as_raw()[..rows],
            "every stitched row must be the row it came from"
        );
    }

    #[test]
    fn overlapping_frames_recover_the_page_exactly() {
        // 9 frames of a 500 px viewport, 250 px apart: the live-capture scenario.
        let page = tall_page(W, 3_000, 11);
        let tops: Vec<u32> = (0..9).map(|i| i * 250).collect();
        let (frames, offsets) = frames_and_offsets(&page, &tops);

        let mut expected = vec![0i32];
        expected.extend(std::iter::repeat_n(250, 8));
        assert_eq!(offsets, expected, "every offset recovered exactly");

        let stitched = stitch_vertical(&frames, &offsets);
        assert_eq!(stitched.height(), H + 8 * 250, "500 + 8*250 rows");
        assert_matches_page(&stitched, &page);
    }

    #[test]
    fn no_frames_is_an_empty_surface() {
        let empty = stitch_vertical(&[], &[]);
        assert_eq!(empty.dimensions(), (0, 0));
    }

    #[test]
    fn a_single_frame_is_copied_verbatim() {
        let page = tall_page(W, 900, 12);
        let frame = window(&page, 400, H);
        let stitched = stitch_vertical(std::slice::from_ref(&frame), &[0]);
        assert_eq!(stitched.dimensions(), (W, H));
        assert_eq!(stitched.as_raw(), frame.as_raw());
    }

    #[test]
    fn reverse_scroll_prepends_rows_above_the_first_frame() {
        // Down to 750, back to 500, then up past the start to 250.
        let page = tall_page(W, 3_000, 13);
        let (frames, offsets) = frames_and_offsets(&page, &[500, 750, 500, 250]);
        assert_eq!(offsets, vec![0, 250, -250, -250]);

        let stitched = stitch_vertical(&frames, &offsets);
        assert_eq!(
            stitched.height(),
            H + 500,
            "two steps of genuinely new rows"
        );
        // The canvas starts at page row 250, four rows of 500 - 250 overlap.
        let rows = stitched.height() as usize * W as usize * 4;
        assert_eq!(
            stitched.as_raw().as_slice(),
            &page.as_raw()[250 * W as usize * 4..250 * W as usize * 4 + rows]
        );
    }

    #[test]
    fn the_height_cap_clips_rather_than_failing() {
        // Narrow page so 40 000 rows stays cheap: 40 000 * 4 px * 4 B = 640 kB.
        const NARROW: u32 = 4;
        let page = tall_page(NARROW, 1_000, 14);
        let frames: Vec<Surface> = (0..50).map(|_| window(&page, 0, 1_000)).collect();
        let mut offsets = vec![0i32];
        offsets.extend(std::iter::repeat_n(900, 49));

        let stitched = stitch_vertical(&frames, &offsets);
        assert_eq!(stitched.height(), MAX_HEIGHT, "clamped to the public cap");
        assert_eq!(stitched.width(), NARROW);
    }

    #[test]
    fn short_offset_lists_are_treated_as_no_movement() {
        let page = tall_page(W, 900, 15);
        let a = window(&page, 0, 200);
        let b = window(&page, 300, 200);
        let stitched = stitch_vertical(&[a, b.clone()], &[0]);
        assert_eq!(stitched.height(), 200, "missing offset means zero");
        assert!(
            stitched.as_raw() == b.as_raw(),
            "with no offset the later frame overlaps and wins"
        );
    }

    #[test]
    fn hostile_offsets_cannot_overflow_the_layout() {
        const NARROW: u32 = 8;
        let page = tall_page(NARROW, 900, 16);
        let a = window(&page, 0, 100);
        let b = window(&page, 100, 100);
        let stitched = stitch_vertical(&[a, b], &[0, i32::MIN]);
        assert_eq!(stitched.height(), MAX_HEIGHT, "clamped, not wrapped");
        assert_eq!(stitched.width(), NARROW);
    }

    #[test]
    fn frames_of_a_different_width_are_skipped() {
        let page = tall_page(W, 900, 17);
        let a = window(&page, 0, 100);
        let narrow = Surface::from_pixel(4, 100, image::Rgba([9, 9, 9, 255]));
        let stitched = stitch_vertical(&[a.clone(), narrow], &[0, 100]);
        assert_eq!(stitched.width(), W);
        assert_eq!(
            stitched.height(),
            200,
            "layout still accounted for the frame"
        );
    }

    #[test]
    fn streaming_pushes_recover_the_page_exactly() {
        let page = tall_page(W, 3_000, 21);
        let mut stitcher = Stitcher::new(W, H, MAX_HEIGHT);
        for step in 0..8 {
            let frame = window(&page, step * 250, H);
            let status = stitcher.push(&frame);
            assert!(
                matches!(status, Push::Accepted { .. } | Push::NoChange),
                "frame {step} was not accepted: {status:?}"
            );
        }
        assert_eq!(
            stitcher.rows(),
            H + 7 * 250,
            "rows grow by the new ones only"
        );
        assert_eq!(stitcher.frames(), 8);
        assert_matches_page(&stitcher.into_surface(), &page);
    }

    #[test]
    fn a_fast_scroll_is_rejected_without_appending() {
        let page = tall_page(W, 3_000, 22);
        let mut stitcher = Stitcher::new(W, H, MAX_HEIGHT);
        assert_eq!(
            stitcher.push(&window(&page, 0, H)),
            Push::Accepted { new_rows: H }
        );
        // 0.85 * 500 = 425: a 470 px jump leaves almost no overlap to trust.
        assert_eq!(
            stitcher.push(&window(&page, 470, H)),
            Push::LowConfidence,
            "fast scroll must be refused, not guessed"
        );
        assert_eq!(stitcher.rows(), H, "nothing was appended");

        // The reference frame is still the first one, so the next sane step works.
        assert_eq!(
            stitcher.push(&window(&page, 250, H)),
            Push::Accepted { new_rows: 250 }
        );
        assert_eq!(stitcher.rows(), H + 250);
    }

    #[test]
    fn a_flat_region_never_stitches_itself_forever() {
        let mut stitcher = Stitcher::new(W, H, MAX_HEIGHT);
        assert_eq!(
            stitcher.push(&flat(W, H, 250)),
            Push::Accepted { new_rows: H }
        );
        // Same blank content, one row nudged darker so it is not byte-identical.
        let mut nudged = flat(W, H, 250);
        for x in 0..W {
            nudged.put_pixel(x, H - 1, image::Rgba([249, 249, 249, 255]));
        }
        assert_eq!(stitcher.push(&nudged), Push::LowConfidence);
        assert_eq!(stitcher.rows(), H, "blank content added nothing");
    }

    #[test]
    fn reverse_scrolling_does_not_duplicate_captured_rows() {
        let page = tall_page(W, 3_000, 23);
        let mut stitcher = Stitcher::new(W, H, MAX_HEIGHT);
        assert!(matches!(
            stitcher.push(&window(&page, 500, H)),
            Push::Accepted { .. }
        ));
        assert_eq!(
            stitcher.push(&window(&page, 750, H)),
            Push::Accepted { new_rows: 250 }
        );
        assert_eq!(
            stitcher.push(&window(&page, 500, H)),
            Push::NoChange,
            "rows we already hold must not be added twice"
        );
        assert_eq!(stitcher.rows(), H + 250);

        // Further up than the anchor: genuinely new rows, prepended.
        assert_eq!(
            stitcher.push(&window(&page, 250, H)),
            Push::Accepted { new_rows: 250 }
        );
        assert_eq!(stitcher.rows(), H + 500);
        let rows = stitcher.rows() as usize * W as usize * 4;
        assert_eq!(
            stitcher.into_surface().as_raw().as_slice(),
            &page.as_raw()[250 * W as usize * 4..250 * W as usize * 4 + rows]
        );
    }

    #[test]
    fn the_height_cap_stops_the_capture() {
        let page = tall_page(W, 3_000, 24);
        let mut stitcher = Stitcher::new(W, H, 1_200);
        assert!(matches!(
            stitcher.push(&window(&page, 0, H)),
            Push::Accepted { .. }
        ));
        assert_eq!(
            stitcher.push(&window(&page, 250, H)),
            Push::Accepted { new_rows: 250 }
        );
        assert_eq!(
            stitcher.push(&window(&page, 500, H)),
            Push::Accepted { new_rows: 250 }
        );
        // 1000 rows captured, 200 left: a 250 px step no longer fits.
        assert_eq!(stitcher.push(&window(&page, 750, H)), Push::Capped);
        assert_eq!(stitcher.rows(), 1_000, "the cap is never exceeded");
    }

    #[test]
    fn frames_of_the_wrong_size_are_refused() {
        let page = tall_page(W, 1_000, 25);
        let mut stitcher = Stitcher::new(W, H, MAX_HEIGHT);
        assert_eq!(
            stitcher.push(&window(&page, 0, H - 20)),
            Push::LowConfidence
        );
        assert_eq!(stitcher.rows(), 0);
    }

    #[test]
    fn identical_frames_are_a_no_op() {
        let page = tall_page(W, 900, 26);
        let frame = window(&page, 0, H);
        let mut stitcher = Stitcher::new(W, H, MAX_HEIGHT);
        assert!(matches!(
            stitcher.push(&frame),
            Push::Accepted { new_rows } if new_rows == H
        ));
        assert_eq!(stitcher.push(&frame), Push::NoChange);
        assert_eq!(stitcher.rows(), H);
        assert_eq!(stitcher.frames(), 1, "no-change frames are not counted");
    }

    #[test]
    fn an_anchor_taller_than_the_cap_is_capped_immediately() {
        let page = tall_page(W, 900, 27);
        let mut stitcher = Stitcher::new(W, H, 200);
        assert_eq!(stitcher.push(&window(&page, 0, H)), Push::Capped);
        assert_eq!(stitcher.rows(), 0);
        assert_eq!(stitcher.into_surface().dimensions(), (0, 0));
    }
}
