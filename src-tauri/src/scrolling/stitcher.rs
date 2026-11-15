//! The scrolling stitcher: one crop-sized frame in, accumulated tall image out.
//!
//! Per frame (`tmp/research/scrolling.md` §5.3):
//!
//! 1. build the row signature ([`super::align::Bands`]),
//! 2. find the vertical shift against the **last accepted** frame, scanning
//!    outward from the previous shift,
//! 3. append only the rows that are actually new (`cur[H−shift..]`), or prepend
//!    them if the user scrolled back up,
//! 4. remember the shift as the next frame's prediction.
//!
//! Nothing is appended when the match is doubtful: a duplicated band in the
//! output is worse than a missing one, and the UI already tells the user to slow
//! down. A frame that fails to match does **not** replace the reference frame, so
//! one bad frame does not poison the ones after it.

use super::align::{find_offset, Bands};
use super::cap;

/// Starting height cap for tests: the real one is [`cap::MAX_HEIGHT`].
#[allow(unused_imports)]
pub use super::cap::MAX_HEIGHT;

/// Outcome of offering one frame to the stitcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Rows were appended (or prepended, for a reverse scroll).
    Accepted { new_rows: u32 },
    /// Nothing moved (or too little to be a scroll): nothing appended.
    NoChange,
    /// The frames could not be aligned confidently: nothing appended, warn.
    LowConfidence,
    /// The height/memory cap was reached: stop the capture.
    Capped,
}

/// Accumulates cropped frames into one tall `rgb24` image.
pub struct ScrollingStitcher {
    width: u32,
    height: u32,
    /// `width × rows`, tightly packed rgb24.
    acc: Vec<u8>,
    rows: u32,
    /// Signature of the last frame that was accepted (the alignment reference).
    last: Option<Bands>,
    /// Raw bytes of the last accepted frame, for the cheap "nothing changed" test.
    last_frame: Vec<u8>,
    /// Previous shift, i.e. the centre of the next scan.
    pred: isize,
    frames: u64,
    last_mad: f32,
}

impl ScrollingStitcher {
    /// `width × height` is the size of every frame that will be pushed (the
    /// selected region, in stream pixels).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            acc: Vec::new(),
            rows: 0,
            last: None,
            last_frame: Vec::new(),
            pred: 0,
            frames: 0,
            last_mad: 0.0,
        }
    }

    /// Offer one cropped `rgb24` frame.
    pub fn push(&mut self, rgb: &[u8]) -> Status {
        if self.width == 0 || self.height == 0 {
            return Status::LowConfidence;
        }
        let expected = self.width as usize * self.height as usize * 3;
        if rgb.len() < expected {
            // A short buffer means a truncated PipeWire frame: never align it.
            return Status::LowConfidence;
        }
        let rgb = &rgb[..expected];

        // First frame: it is the anchor.
        if self.acc.is_empty() {
            self.acc.extend_from_slice(rgb);
            self.rows = self.height;
            self.last = Bands::new(rgb, self.width as usize, self.height as usize);
            self.last_frame.clear();
            self.last_frame.extend_from_slice(rgb);
            self.frames = 1;
            return Status::Accepted {
                new_rows: self.height,
            };
        }

        // Bit-identical to the last accepted frame: the screen is simply static.
        if self.last_frame.len() == rgb.len() && self.last_frame == rgb {
            return Status::NoChange;
        }

        let Some(cur) = Bands::new(rgb, self.width as usize, self.height as usize) else {
            return Status::LowConfidence;
        };
        let Some(prev) = self.last.as_ref() else {
            // The first frame was featureless; adopt this one as the reference so
            // a capture that starts on a blank area can still progress.
            self.last = Some(cur);
            self.last_frame.clear();
            self.last_frame.extend_from_slice(rgb);
            return Status::NoChange;
        };

        let Some(found) = find_offset(prev, &cur, self.pred) else {
            return Status::LowConfidence;
        };
        self.last_mad = found.mad;

        if found.shift == 0 {
            // Same reference content in place: remember the newer frame (it may
            // carry a caret or hover change) but append nothing.
            self.accept_reference(rgb, cur);
            return Status::NoChange;
        }

        let moved = found.shift.unsigned_abs() as u32;
        if cap::check(self.width, self.rows, moved).is_err() {
            return Status::Capped;
        }

        let row_bytes = self.width as usize * 3;
        let moved_bytes = moved as usize * row_bytes;

        if found.shift > 0 {
            // Content moved up: the bottom `shift` rows are new — unless the user
            // already scrolled through them and came back (a wiggle), in which
            // case appending would duplicate a band.
            let from = (self.height - moved) as usize * row_bytes;
            let fresh = &rgb[from..];
            let already =
                self.acc.len() >= moved_bytes && self.acc[self.acc.len() - moved_bytes..] == *fresh;
            if already {
                self.accept_reference(rgb, cur);
                self.pred = found.shift;
                return Status::NoChange;
            }
            self.acc.extend_from_slice(fresh);
        } else {
            // Reverse scroll: new rows arrive at the top. If they are the rows we
            // already start with, the user merely scrolled back up into captured
            // content — prepending them would duplicate that band.
            let head = &rgb[..moved_bytes];
            let already = self.acc.len() >= moved_bytes && self.acc[..moved_bytes] == *head;
            if already {
                self.accept_reference(rgb, cur);
                self.pred = found.shift;
                return Status::NoChange;
            }
            self.acc.splice(0..0, head.to_vec());
        }

        self.rows += moved;
        self.accept_reference(rgb, cur);
        self.pred = found.shift;
        Status::Accepted { new_rows: moved }
    }

    fn accept_reference(&mut self, rgb: &[u8], bands: Bands) {
        self.last = Some(bands);
        self.last_frame.clear();
        self.last_frame.extend_from_slice(rgb);
        self.frames += 1;
    }

    #[allow(dead_code)]
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Frames that produced content (or a confirmed no-change).
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// MAD of the last accepted match — the confidence the UI shows.
    pub fn confidence(&self) -> f32 {
        self.last_mad
    }

    /// The accumulated image: `(rgb24 bytes, width, height)`.
    pub fn into_image(self) -> (Vec<u8>, u32, u32) {
        (self.acc, self.width, self.rows)
    }

    #[allow(dead_code)]
    pub fn pixels(&self) -> &[u8] {
        &self.acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrolling::align::tests::{tall_page, window};

    const W: usize = 320;
    const H: usize = 500;

    /// Push every frame of a scripted scroll and return the stitcher.
    fn scroll(page: &[u8], tops: &[usize]) -> ScrollingStitcher {
        let mut stitcher = ScrollingStitcher::new(W as u32, H as u32);
        for &top in tops {
            let status = stitcher.push(&window(page, W, top, H));
            assert!(
                matches!(status, Status::Accepted { .. } | Status::NoChange),
                "frame at {top} was not accepted: {status:?}"
            );
        }
        stitcher
    }

    #[test]
    fn overlapping_frames_recover_the_scroll_exactly() {
        // 10 frames of a 500 px viewport, 250 px apart: the probe's scenario.
        let page = tall_page(W, 3000);
        let tops: Vec<usize> = (0..9).map(|i| i * 250).collect();
        let stitcher = scroll(&page, &tops);

        let expected_rows = (H + 8 * 250) as u32;
        assert_eq!(stitcher.rows(), expected_rows, "500 + 8*250 rows");
        assert!(
            stitcher.pixels() == &page[..expected_rows as usize * W * 3],
            "every row must land where it came from, with no duplicated seam"
        );
    }

    #[test]
    fn a_fast_scroll_is_rejected_without_appending() {
        let page = tall_page(W, 3000);
        let mut stitcher = ScrollingStitcher::new(W as u32, H as u32);
        assert_eq!(
            stitcher.push(&window(&page, W, 0, H)),
            Status::Accepted { new_rows: H as u32 }
        );
        // 0.85 * 500 = 425: a 470 px jump leaves almost no overlap to trust.
        let status = stitcher.push(&window(&page, W, 470, H));
        assert_eq!(status, Status::LowConfidence, "fast scroll must be refused");
        assert_eq!(stitcher.rows(), H as u32, "nothing was appended");

        // The reference frame is still frame 0, so the next sane scroll works.
        let status = stitcher.push(&window(&page, W, 250, H));
        assert_eq!(status, Status::Accepted { new_rows: 250 });
        assert_eq!(stitcher.rows(), (H + 250) as u32);
    }

    #[test]
    fn a_flat_region_never_stitches_itself_forever() {
        let mut stitcher = ScrollingStitcher::new(W as u32, H as u32);
        let blank = vec![250u8; W * H * 3];
        assert!(matches!(
            stitcher.push(&blank),
            Status::Accepted { new_rows } if new_rows == H as u32
        ));
        // A flat area that shifts by 8 px must not be "matched" at offset 8.
        let mut nudged = blank.clone();
        // Same content, one row of slightly darker grey added at the bottom, to
        // be sure the frame is not byte-identical (the cheap NoChange path).
        for px in nudged[(H - 1) * W * 3..].chunks_mut(3) {
            px[0] = 249;
            px[1] = 249;
            px[2] = 249;
        }
        assert_eq!(stitcher.push(&nudged), Status::LowConfidence);
        assert_eq!(stitcher.rows(), H as u32, "blank content added nothing");
    }

    #[test]
    fn a_sticky_header_is_kept_once_not_repeated() {
        const HEADER: usize = 60;
        let body = tall_page(W, 3000);
        // Every frame = the same 60 px header + a window of the body.
        let frame = |top: usize| -> Vec<u8> {
            let mut out = vec![0u8; W * H * 3];
            // Header: frozen toolbar, identical in every frame.
            for y in 0..HEADER {
                for x in 0..W {
                    let i = (y * W + x) * 3;
                    let v = if x % 4 == 0 { 60 } else { 210 };
                    out[i] = v;
                    out[i + 1] = v;
                    out[i + 2] = v;
                }
            }
            out[HEADER * W * 3..].copy_from_slice(&body[top * W * 3..(top + H - HEADER) * W * 3]);
            out
        };

        let mut stitcher = ScrollingStitcher::new(W as u32, H as u32);
        let mut tops = Vec::new();
        for i in 0..6 {
            let top = i * 220;
            tops.push(top);
            let status = stitcher.push(&frame(top));
            assert!(
                matches!(status, Status::Accepted { .. } | Status::NoChange),
                "sticky-header frame {top} rejected: {status:?}"
            );
        }

        let expected = (H + 5 * 220) as u32;
        assert_eq!(stitcher.rows(), expected, "only the body grew");

        // Header rows appear exactly once, at the top, and the body below them is
        // the page from row 0 on — i.e. the seams are clean.
        let header = &frame(0)[..HEADER * W * 3];
        let stitched = stitcher.pixels();
        assert!(stitched[..HEADER * W * 3] == *header, "header stays on top");
        assert!(
            stitched[HEADER * W * 3..(HEADER + 200) * W * 3] == body[..200 * W * 3],
            "body starts right under the single header"
        );
        // Count occurrences of the whole 60-row header block.
        let occurrences = (0..=expected as usize - HEADER)
            .filter(|&y| stitched[y * W * 3..(y + HEADER) * W * 3] == *header)
            .count();
        assert_eq!(occurrences, 1, "the sticky header must not be repeated");
    }

    #[test]
    fn reverse_scrolling_does_not_duplicate_captured_rows() {
        let page = tall_page(W, 3000);
        let mut stitcher = ScrollingStitcher::new(W as u32, H as u32);
        // Start mid-page, scroll down, then scroll back up into captured content.
        assert!(matches!(
            stitcher.push(&window(&page, W, 500, H)),
            Status::Accepted { .. }
        ));
        assert_eq!(
            stitcher.push(&window(&page, W, 750, H)),
            Status::Accepted { new_rows: 250 }
        );
        assert_eq!(
            stitcher.push(&window(&page, W, 500, H)),
            Status::NoChange,
            "content we already hold must not be added twice"
        );
        assert_eq!(stitcher.rows(), (H + 250) as u32);

        // Scrolling further up than the first frame adds genuinely new rows on
        // top — and they must land above the existing image.
        assert_eq!(
            stitcher.push(&window(&page, W, 250, H)),
            Status::Accepted { new_rows: 250 }
        );
        assert_eq!(stitcher.rows(), (H + 500) as u32);
        assert!(
            stitcher.pixels() == &page[250 * W * 3..(250 + H + 500) * W * 3],
            "prepended rows belong above the existing ones"
        );
    }

    #[test]
    fn identical_frames_are_a_no_op() {
        let page = tall_page(W, 3000);
        let mut stitcher = ScrollingStitcher::new(W as u32, H as u32);
        let first = window(&page, W, 0, H);
        stitcher.push(&first);
        assert_eq!(stitcher.push(&first), Status::NoChange);
        assert_eq!(stitcher.rows(), H as u32);
        assert_eq!(stitcher.frames(), 1, "no-change frames are not counted");
    }
}
