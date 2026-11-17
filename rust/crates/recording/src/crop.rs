//! Pure rgb24 region cropping.
//!
//! Nothing here touches PipeWire or ffmpeg, so the whole region contract is
//! unit-testable on any machine — which matters because the portal leg cannot
//! run on a locked screen.
//!
//! Rules (see `crates/recording` contract):
//! * clamp the region to the frame (a selection dragged off-screen is normal),
//! * pad the result up to **even** width/height (yuv420p 4:2:0 requires it),
//! * black-pad whatever part of the region the frame does not cover,
//! * a full-frame region returns the frame unchanged.

use crate::Region;

/// Largest frame dimension we accept from a producer. Guards the allocations
/// below against a hostile/broken size hint.
pub(crate) const MAX_DIM: u32 = 16_384;

/// Round up to the next even number: `720 -> 720`, `721 -> 722`.
fn even_ceil(value: usize) -> usize {
    value + (value & 1)
}

/// Bytes of one rgb24 image of `width`x`height`.
pub(crate) fn rgb24_len(width: usize, height: usize) -> usize {
    width.saturating_mul(height).saturating_mul(3)
}

/// Dimensions of the rgb24 image [`crop_frame_rgb24`] produces for `region`.
///
/// This is what ffmpeg must be told via `-s`; frame and encoder size drifting
/// apart shears (or corrupts) the whole take.
pub(crate) fn crop_dims(region: Region) -> (usize, usize) {
    let width = region.width.clamp(1, MAX_DIM) as usize;
    let height = region.height.clamp(1, MAX_DIM) as usize;
    (even_ceil(width), even_ceil(height))
}

/// Crop a tightly-packed `width`x`height` rgb24 frame to `region`.
///
/// The returned buffer is always exactly `crop_dims(region)` big, zero-filled
/// outside the copied rectangle. That is deliberate: the encoder is spawned
/// with one fixed `-s`, so every frame must be that exact length even when the
/// region hangs off the edge of the frame (`black-pad remainder`).
///
/// A frame shorter than `width * height * 3` (a truncated buffer) yields an
/// all-black image rather than a panic or a short write.
pub fn crop_frame_rgb24(frame: &[u8], width: u32, height: u32, region: Region) -> Vec<u8> {
    let frame_w = width as usize;
    let frame_h = height as usize;
    let (out_w, out_h) = crop_dims(region);
    let mut out = vec![0u8; rgb24_len(out_w, out_h)];

    if frame_w == 0 || frame_h == 0 || frame.len() < rgb24_len(frame_w, frame_h) {
        return out;
    }

    // Intersection of the region with the frame, in frame pixels. i64 keeps a
    // `left + width` near i32::MAX from wrapping.
    let left = region.left.max(0) as usize;
    let top = region.top.max(0) as usize;
    let right = (region.left as i64 + region.width as i64).clamp(0, frame_w as i64) as usize;
    let bottom = (region.top as i64 + region.height as i64).clamp(0, frame_h as i64) as usize;

    if left >= frame_w || top >= frame_h {
        return out;
    }
    // Where the region's own origin sits inside the output: a selection dragged
    // 2 px off-screen left keeps its geometry, so the frame starts 2 px in.
    let pad_left = (-(region.left as i64)).clamp(0, out_w as i64) as usize;
    let pad_top = (-(region.top as i64)).clamp(0, out_h as i64) as usize;
    let copy_w = right
        .saturating_sub(left)
        .min(out_w.saturating_sub(pad_left));
    let copy_h = bottom
        .saturating_sub(top)
        .min(out_h.saturating_sub(pad_top));
    if copy_w == 0 || copy_h == 0 {
        return out;
    }

    let row_bytes = copy_w * 3;
    for y in 0..copy_h {
        let src_start = ((top + y) * frame_w + left) * 3;
        let dst_start = ((pad_top + y) * out_w + pad_left) * 3;
        out[dst_start..dst_start + row_bytes]
            .copy_from_slice(&frame[src_start..src_start + row_bytes]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`x`h` frame whose pixel `(x, y)` is `(x, y, x ^ y)` — every channel
    /// distinct, so a wrong row/column offset cannot look right by accident.
    fn frame(w: usize, h: usize) -> Vec<u8> {
        let mut f = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                f[i] = x as u8;
                f[i + 1] = y as u8;
                f[i + 2] = (x ^ y) as u8;
            }
        }
        f
    }

    fn pixel(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let i = (y * w + x) * 3;
        [buf[i], buf[i + 1], buf[i + 2]]
    }

    fn region(left: i32, top: i32, width: u32, height: u32) -> Region {
        Region {
            left,
            top,
            width,
            height,
        }
    }

    #[test]
    fn a_full_frame_region_returns_the_frame_unchanged() {
        let src = frame(8, 4);
        let out = crop_frame_rgb24(&src, 8, 4, region(0, 0, 8, 4));
        assert_eq!(out, src);
    }

    #[test]
    fn the_crop_recovers_exactly_the_selected_pixels() {
        let src = frame(16, 12);
        let out = crop_frame_rgb24(&src, 16, 12, region(4, 6, 6, 4));
        assert_eq!(out.len(), 6 * 4 * 3);
        for y in 0..4 {
            for x in 0..6 {
                assert_eq!(
                    pixel(&out, 6, x, y),
                    pixel(&src, 16, 4 + x, 6 + y),
                    "pixel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn a_region_running_past_the_frame_edge_is_clamped_and_black_padded() {
        let src = frame(8, 8);
        // 4 columns and 4 rows actually exist; the rest is padding.
        let out = crop_frame_rgb24(&src, 8, 8, region(4, 4, 8, 8));
        assert_eq!(out.len(), 8 * 8 * 3, "region size, not the clamped size");
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(pixel(&out, 8, x, y), pixel(&src, 8, 4 + x, 4 + y));
            }
        }
        for y in 0..8 {
            for x in 4..8 {
                assert_eq!(pixel(&out, 8, x, y), [0, 0, 0], "pad at {x},{y}");
            }
        }
        for y in 4..8 {
            for x in 0..8 {
                assert_eq!(pixel(&out, 8, x, y), [0, 0, 0], "pad at {x},{y}");
            }
        }
    }

    #[test]
    fn a_region_starting_before_the_origin_keeps_its_offsets() {
        let src = frame(16, 16);
        // Starts 2 px left of and 2 px above the frame: the visible part must
        // land 2 px into the output, not at 0,0.
        let out = crop_frame_rgb24(&src, 16, 16, region(-2, -2, 6, 6));
        assert_eq!(out.len(), 6 * 6 * 3);
        for y in 0..2 {
            for x in 0..6 {
                assert_eq!(pixel(&out, 6, x, y), [0, 0, 0], "top pad {x},{y}");
            }
        }
        for y in 2..6 {
            for x in 0..2 {
                assert_eq!(pixel(&out, 6, x, y), [0, 0, 0], "left pad {x},{y}");
            }
        }
        for y in 2..6 {
            for x in 2..6 {
                assert_eq!(
                    pixel(&out, 6, x, y),
                    pixel(&src, 16, x - 2, y - 2),
                    "visible pixel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn odd_region_sizes_are_padded_to_even_dimensions() {
        let src = frame(8, 8);
        let out = crop_frame_rgb24(&src, 8, 8, region(1, 1, 5, 3));
        assert_eq!(out.len(), 6 * 4 * 3, "5x3 padded up to 6x4");
        assert_eq!(crop_dims(region(1, 1, 5, 3)), (6, 4));
        // Real pixels stay in the top-left; the extra column/row is black.
        assert_eq!(pixel(&out, 6, 4, 0), pixel(&src, 8, 5, 1));
        for x in 0..6 {
            assert_eq!(pixel(&out, 6, x, 3), [0, 0, 0], "bottom pad row");
        }
        for y in 0..3 {
            assert_eq!(pixel(&out, 6, 5, y), [0, 0, 0], "right pad column");
        }
    }

    #[test]
    fn a_region_that_misses_the_frame_is_a_black_image_of_region_size() {
        let src = frame(8, 8);
        let out = crop_frame_rgb24(&src, 8, 8, region(64, 64, 4, 4));
        assert_eq!(out, vec![0u8; 4 * 4 * 3]);
    }

    #[test]
    fn a_region_hanging_off_the_left_edge_entirely_is_black() {
        let src = frame(8, 8);
        let out = crop_frame_rgb24(&src, 8, 8, region(-32, 0, 16, 8));
        assert_eq!(out, vec![0u8; 16 * 8 * 3]);
    }

    #[test]
    fn a_truncated_frame_yields_a_black_image_instead_of_a_panic() {
        let src = vec![7u8; 10]; // shorter than 8*8*3
        let out = crop_frame_rgb24(&src, 8, 8, region(0, 0, 8, 8));
        assert_eq!(out, vec![0u8; 8 * 8 * 3]);
    }

    #[test]
    fn a_zero_sized_frame_yields_a_black_image_of_region_size() {
        let out = crop_frame_rgb24(&[], 0, 0, region(0, 0, 2, 2));
        assert_eq!(out, vec![0u8; 2 * 2 * 3]);
    }

    #[test]
    fn a_hostile_region_is_capped_before_it_can_allocate() {
        // 4 Gx4 G would be ~50 exabytes; the cap keeps this test instant.
        let dims = crop_dims(region(0, 0, u32::MAX, u32::MAX));
        assert_eq!(dims, (MAX_DIM as usize, MAX_DIM as usize));
        let out = crop_frame_rgb24(&frame(2, 2), 2, 2, region(0, 0, u32::MAX, u32::MAX));
        assert_eq!(out.len(), rgb24_len(MAX_DIM as usize, MAX_DIM as usize));
    }

    #[test]
    fn a_region_touching_the_far_edge_keeps_its_last_row_and_column() {
        let src = frame(8, 8);
        let out = crop_frame_rgb24(&src, 8, 8, region(6, 6, 2, 2));
        assert_eq!(pixel(&out, 2, 1, 1), pixel(&src, 8, 7, 7));
    }
}
