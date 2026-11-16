#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L2 — image algorithms: region blur (3-pass separable box ≈ gaussian),
//! pixelate, irreversible blackout, and the mode dispatch used by the
//! compositor and export.
//!
//! Every entry point is hostile-input safe: rects are clamped with
//! [`Rect::clamp_to`], radii/blocks are capped, and all buffer access is
//! bounds-checked with `.get()`. Work scales with the region (+halo), not the
//! whole image.

use snagcraft_doc::BlurMode;
use snagcraft_geom::Rect;
use snagcraft_raster::Surface;

/// Blur radius cap (hostile-input rule).
pub const MAX_BLUR_RADIUS: u32 = 64;
/// Pixelate block cap (hostile-input rule).
pub const MAX_PIXELATE_BLOCK: u32 = 64;

/// Read 4 bytes at `i` from a packed RGBA buffer. Missing bytes read as zero.
fn px4(buf: &[u8], i: usize) -> [u8; 4] {
    match buf.get(i..i + 4) {
        Some(&[r, g, b, a]) => [r, g, b, a],
        _ => [0, 0, 0, 0],
    }
}

/// Write 4 bytes at `i` into a packed RGBA buffer. Out-of-range writes are dropped.
fn put4(buf: &mut [u8], i: usize, p: [u8; 4]) {
    if let Some(chunk) = buf.get_mut(i..i + 4) {
        chunk.copy_from_slice(&p);
    }
}

/// Per-channel mean of `sum` over `n` samples, saturating at 255.
/// None when `n` is zero (empty block).
fn average(sum: &[u64; 4], n: u64) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    for c in 0..4 {
        out[c] = sum[c].checked_div(n)?.min(255) as u8;
    }
    Some(out)
}

/// 3-pass separable box blur ≈ gaussian. REPLACES the region pixels (no
/// ghosting of the original), reads clamp to the full image (radius-wide halo,
/// so no dark fringe at region edges), and writes only inside the region.
/// `radius` is clamped to `1..=MAX_BLUR_RADIUS`. Returns false when the region
/// was empty/invalid or every pixel already matched the blurred result.
pub fn gaussian_blur_region(img: &mut Surface, rect: Rect, radius: u32) -> bool {
    let (w, h) = img.dimensions();
    let Some((x0, y0, x1, y1)) = rect.clamp_to(w, h) else {
        return false;
    };
    let r = radius.clamp(1, MAX_BLUR_RADIUS) as usize;

    // Region + radius-wide halo, so window sums near the region border read
    // real neighbouring pixels instead of the region edge.
    let px0 = x0.saturating_sub(r as u32);
    let py0 = y0.saturating_sub(r as u32);
    let px1 = x1.saturating_add(r as u32).min(w);
    let py1 = y1.saturating_add(r as u32).min(h);
    let pw = (px1 - px0) as usize;
    let ph = (py1 - py0) as usize;
    if pw == 0 || ph == 0 {
        return false;
    }

    let mut src = vec![0u8; pw * ph * 4];
    for y in 0..ph {
        for x in 0..pw {
            let p = img.get_pixel(px0 + x as u32, py0 + y as u32).0;
            put4(&mut src, (y * pw + x) * 4, p);
        }
    }

    let rx0 = (x0 - px0) as usize;
    let ry0 = (y0 - py0) as usize;
    let rx1 = (x1 - px0) as usize;
    let ry1 = (y1 - py0) as usize;
    let last_col = pw - 1;
    let last_row = ph - 1;

    let mut a = src.clone();
    let mut b = src;
    for _pass in 0..3 {
        // Horizontal pass: a -> b, region rows only, region columns written.
        for y in ry0..ry1 {
            let row = y * pw;
            let mut sum = [0u32; 4];
            let mut left = rx0.saturating_sub(r);
            let mut right = (rx0 + r).min(last_col);
            for x in left..=right {
                let p = px4(&a, (row + x) * 4);
                for c in 0..4 {
                    sum[c] += p[c] as u32;
                }
            }
            for x in rx0..rx1 {
                let count = (right - left + 1) as u32;
                let mut out = [0u8; 4];
                for c in 0..4 {
                    out[c] = (sum[c] / count) as u8;
                }
                put4(&mut b, (row + x) * 4, out);

                let new_right = (x + 1 + r).min(last_col);
                if new_right != right {
                    let p = px4(&a, (row + new_right) * 4);
                    for c in 0..4 {
                        sum[c] += p[c] as u32;
                    }
                    right = new_right;
                }
                let new_left = x.saturating_sub(r);
                if new_left != left {
                    let p = px4(&a, (row + left) * 4);
                    for c in 0..4 {
                        sum[c] = sum[c].saturating_sub(p[c] as u32);
                    }
                    left = new_left;
                }
            }
        }
        // Vertical pass: b -> a, region columns only, region rows written.
        for x in rx0..rx1 {
            let mut sum = [0u32; 4];
            let mut top = ry0.saturating_sub(r);
            let mut bottom = (ry0 + r).min(last_row);
            for y in top..=bottom {
                let p = px4(&b, (y * pw + x) * 4);
                for c in 0..4 {
                    sum[c] += p[c] as u32;
                }
            }
            for y in ry0..ry1 {
                let count = (bottom - top + 1) as u32;
                let mut out = [0u8; 4];
                for c in 0..4 {
                    out[c] = (sum[c] / count) as u8;
                }
                put4(&mut a, (y * pw + x) * 4, out);

                let new_bottom = (y + 1 + r).min(last_row);
                if new_bottom != bottom {
                    let p = px4(&b, (new_bottom * pw + x) * 4);
                    for c in 0..4 {
                        sum[c] += p[c] as u32;
                    }
                    bottom = new_bottom;
                }
                let new_top = y.saturating_sub(r);
                if new_top != top {
                    let p = px4(&b, (top * pw + x) * 4);
                    for c in 0..4 {
                        sum[c] = sum[c].saturating_sub(p[c] as u32);
                    }
                    top = new_top;
                }
            }
        }
    }

    // Copy the blurred region back; pixels outside stay untouched.
    let mut changed = false;
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let out = px4(&a, (y * pw + x) * 4);
            let dst = img.get_pixel_mut(px0 + x as u32, py0 + y as u32);
            if dst.0 != out {
                dst.0 = out;
                changed = true;
            }
        }
    }
    changed
}

/// Average-colour pixelation of `rect`. `block` is clamped to `2..=MAX_PIXELATE_BLOCK`
/// and blocks are aligned to the (clamped) region origin; partial edge blocks are
/// averaged over the pixels that actually exist inside the region.
/// Returns false when nothing changed.
pub fn pixelate_region(img: &mut Surface, rect: Rect, block: u32) -> bool {
    let (w, h) = img.dimensions();
    let Some((x0, y0, x1, y1)) = rect.clamp_to(w, h) else {
        return false;
    };
    let b = block.clamp(2, MAX_PIXELATE_BLOCK);
    let mut changed = false;

    let mut by = y0;
    while by < y1 {
        let by1 = by.saturating_add(b).min(y1);
        let mut bx = x0;
        while bx < x1 {
            let bx1 = bx.saturating_add(b).min(x1);
            let mut sum = [0u64; 4];
            let mut n = 0u64;
            for y in by..by1 {
                for x in bx..bx1 {
                    let p = img.get_pixel(x, y).0;
                    for c in 0..4 {
                        sum[c] += p[c] as u64;
                    }
                    n += 1;
                }
            }
            if let Some(avg) = average(&sum, n) {
                for y in by..by1 {
                    for x in bx..bx1 {
                        let dst = img.get_pixel_mut(x, y);
                        if dst.0 != avg {
                            dst.0 = avg;
                            changed = true;
                        }
                    }
                }
            }
            bx = bx1;
        }
        by = by1;
    }
    changed
}

/// Solid opaque black fill — irreversible redaction (no recoverable pixels).
/// Returns false when nothing changed.
pub fn blackout_region(img: &mut Surface, rect: Rect) -> bool {
    let (w, h) = img.dimensions();
    let Some((x0, y0, x1, y1)) = rect.clamp_to(w, h) else {
        return false;
    };
    let black = snagcraft_color::Rgba::BLACK.to_array();
    let mut changed = false;
    for y in y0..y1 {
        for x in x0..x1 {
            let dst = img.get_pixel_mut(x, y);
            if dst.0 != black {
                dst.0 = black;
                changed = true;
            }
        }
    }
    changed
}

/// Clone `img` and apply `mode` over `rect`. Pixelate uses `radius` as the block
/// size. The input surface is never mutated (non-destructive document object).
pub fn apply_blur(img: &Surface, rect: Rect, mode: BlurMode, radius: u32) -> Surface {
    let mut out = img.clone();
    match mode {
        BlurMode::Gaussian => {
            gaussian_blur_region(&mut out, rect, radius);
        }
        BlurMode::Pixelate => {
            pixelate_region(&mut out, rect, radius);
        }
        BlurMode::Blackout => {
            blackout_region(&mut out, rect);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagcraft_color::Rgba;
    use snagcraft_geom::Size;
    use snagcraft_raster::filled;

    /// Gradient + fine checkerboard, so blur has real detail to destroy.
    fn synthetic(w: u32, h: u32) -> Surface {
        let mut img = filled(Size::new(w, h), Rgba::BLACK);
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 / w.max(1) as f32;
                let fy = y as f32 / h.max(1) as f32;
                let boost = if ((x / 4) + (y / 4)) % 2 == 0 { 60 } else { 0 };
                let p = img.get_pixel_mut(x, y);
                p.0 = [
                    (40.0 + 180.0 * fx) as u8,
                    (70.0 + 120.0 * fy) as u8 + boost,
                    (160.0 + 60.0 * (1.0 - fx)) as u8,
                    255,
                ];
            }
        }
        img
    }

    fn variance(img: &Surface, x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
        let mut sum = 0f64;
        let mut sum2 = 0f64;
        let mut n = 0f64;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = img.get_pixel(x, y).0;
                let l = (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
                sum += l;
                sum2 += l * l;
                n += 1.0;
            }
        }
        if n == 0.0 {
            return 0.0;
        }
        let mean = sum / n;
        sum2 / n - mean * mean
    }

    /// Constant background + fine checkerboard: pure high-frequency detail, so
    /// variance reduction after a blur is unambiguous.
    fn checkerboard(w: u32, h: u32) -> Surface {
        let mut img = filled(Size::new(w, h), Rgba::new(100, 100, 100, 255));
        for y in 0..h {
            for x in 0..w {
                if ((x / 3) + (y / 3)) % 2 == 0 {
                    img.get_pixel_mut(x, y).0 = [160, 160, 160, 255];
                }
            }
        }
        img
    }

    #[test]
    fn blur_reduces_variance() {
        let mut img = checkerboard(96, 96);
        let before = variance(&img, 10, 10, 80, 80);
        assert!(before > 100.0, "test image must carry detail: {before}");
        assert!(gaussian_blur_region(
            &mut img,
            Rect::new(10.0, 10.0, 70.0, 70.0),
            5
        ));
        let after = variance(&img, 10, 10, 80, 80);
        assert!(
            after < before * 0.2,
            "blur must smooth: {before} -> {after}"
        );
    }

    #[test]
    fn blur_uniform_region_is_invariant() {
        let mut img = filled(Size::new(48, 48), Rgba::new(120, 130, 140, 255));
        let copy = img.clone();
        assert!(!gaussian_blur_region(
            &mut img,
            Rect::new(8.0, 8.0, 32.0, 32.0),
            6
        ));
        assert_eq!(img, copy);
    }

    #[test]
    fn blur_leaves_outside_pixels_untouched() {
        let mut img = synthetic(64, 64);
        let copy = img.clone();
        assert!(gaussian_blur_region(
            &mut img,
            Rect::new(20.0, 20.0, 20.0, 20.0),
            3
        ));
        for y in 0..64 {
            for x in 0..64 {
                let inside = (20..40).contains(&x) && (20..40).contains(&y);
                if !inside {
                    assert_eq!(
                        img.get_pixel(x, y).0,
                        copy.get_pixel(x, y).0,
                        "outside pixel ({x},{y}) changed"
                    );
                }
            }
        }
        // Pixels inside the region did change.
        assert_ne!(&img, &copy);
    }

    #[test]
    fn blur_has_no_dark_fringe_at_region_edge() {
        // Uniform image, small region: a clamping bug at the region border would
        // produce a darker edge. Expect the exact source colour everywhere.
        let base = Rgba::new(200, 200, 200, 255);
        let mut img = filled(Size::new(64, 64), base);
        gaussian_blur_region(&mut img, Rect::new(16.0, 16.0, 16.0, 16.0), 8);
        assert_eq!(img.get_pixel(16, 16).0, base.to_array());
        assert_eq!(img.get_pixel(31, 31).0, base.to_array());
    }

    #[test]
    fn blur_hostile_rects_never_panic() {
        let mut img = synthetic(32, 32);
        for rect in [
            Rect::new(-1e9, -1e9, 1e9, 1e9),
            Rect::new(f32::NAN, 0.0, 10.0, 10.0),
            Rect::new(0.0, f32::INFINITY, 10.0, 10.0),
            Rect::new(100.0, 100.0, 10.0, 10.0),
            Rect::new(0.0, 0.0, 0.0, 0.0),
            Rect::new(10.0, 10.0, -5.0, -5.0),
        ] {
            let _ = gaussian_blur_region(&mut img, rect, 7);
        }
        let mut tiny = synthetic(1, 1);
        let _ = gaussian_blur_region(&mut tiny, Rect::new(0.0, 0.0, 1.0, 1.0), 64);
        let mut empty = Surface::new(0, 0);
        assert!(!gaussian_blur_region(
            &mut empty,
            Rect::new(0.0, 0.0, 1.0, 1.0),
            4
        ));
    }

    #[test]
    fn blur_radius_extremes_are_clamped() {
        let mut zero = synthetic(32, 32);
        let mut wild = zero.clone();
        assert!(gaussian_blur_region(
            &mut zero,
            Rect::new(0.0, 0.0, 32.0, 32.0),
            0
        ));
        assert!(gaussian_blur_region(
            &mut wild,
            Rect::new(0.0, 0.0, 32.0, 32.0),
            u32::MAX
        ));
        // radius 0 behaves like radius 1; u32::MAX behaves like 64.
        let mut r1 = synthetic(32, 32);
        let mut r64 = synthetic(32, 32);
        gaussian_blur_region(&mut r1, Rect::new(0.0, 0.0, 32.0, 32.0), 1);
        gaussian_blur_region(&mut r64, Rect::new(0.0, 0.0, 32.0, 32.0), 64);
        assert_eq!(zero, r1);
        assert_eq!(wild, r64);
    }

    #[test]
    fn blur_is_deterministic() {
        let run = || {
            let mut img = synthetic(80, 80);
            gaussian_blur_region(&mut img, Rect::new(5.0, 5.0, 60.0, 60.0), 4);
            img
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn pixelate_averages_each_block() {
        // 32x32 with a 4x4 black patch in the north-west: the 8px block that
        // contains it averages to 25% black + 75% white = 191 grey.
        let mut img = filled(Size::new(32, 32), Rgba::WHITE);
        for y in 0..4 {
            for x in 0..4 {
                img.get_pixel_mut(x, y).0 = Rgba::BLACK.to_array();
            }
        }
        assert!(pixelate_region(
            &mut img,
            Rect::new(0.0, 0.0, 32.0, 32.0),
            8
        ));
        // That one quadrant is now 25% black + 75% white = 191 grey.
        let p = img.get_pixel(0, 0).0;
        assert!(
            (185..=196).contains(&p[0]),
            "block should be the average grey, got {p:?}"
        );
        assert_eq!(img.get_pixel(7, 7).0, p);
        // Fully white blocks stay white.
        assert_eq!(img.get_pixel(9, 9).0, Rgba::WHITE.to_array());
    }

    #[test]
    fn pixelate_leaves_outside_pixels_untouched() {
        let mut img = synthetic(48, 48);
        let copy = img.clone();
        assert!(pixelate_region(
            &mut img,
            Rect::new(8.0, 8.0, 16.0, 16.0),
            4
        ));
        for y in 0..48 {
            for x in 0..48 {
                let inside = (8..24).contains(&x) && (8..24).contains(&y);
                if !inside {
                    assert_eq!(img.get_pixel(x, y).0, copy.get_pixel(x, y).0);
                }
            }
        }
    }

    #[test]
    fn pixelate_hostile_input_is_safe() {
        let mut img = synthetic(24, 24);
        let copy = img.clone();
        assert!(!pixelate_region(
            &mut img,
            Rect::new(f32::NAN, 0.0, 1.0, 1.0),
            4
        ));
        assert!(!pixelate_region(
            &mut img,
            Rect::new(100.0, 100.0, 4.0, 4.0),
            4
        ));
        assert_eq!(img, copy);
        // block below 2 clamps to 2, above the cap clamps to 64.
        let _ = pixelate_region(&mut img, Rect::new(0.0, 0.0, 24.0, 24.0), 0);
        let _ = pixelate_region(&mut img, Rect::new(0.0, 0.0, 24.0, 24.0), u32::MAX);
        let mut empty = Surface::new(0, 0);
        assert!(!pixelate_region(
            &mut empty,
            Rect::new(0.0, 0.0, 4.0, 4.0),
            4
        ));
    }

    #[test]
    fn blackout_fills_opaque_black_and_is_false_when_already_black() {
        let mut img = filled(Size::new(32, 32), Rgba::new(10, 200, 30, 128));
        assert!(blackout_region(&mut img, Rect::new(8.0, 8.0, 8.0, 8.0)));
        for y in 8..16 {
            for x in 8..16 {
                assert_eq!(img.get_pixel(x, y).0, [0, 0, 0, 255]);
            }
        }
        assert_eq!(img.get_pixel(0, 0).0, [10, 200, 30, 128]);
        // Second time is a no-op.
        assert!(!blackout_region(&mut img, Rect::new(8.0, 8.0, 8.0, 8.0)));
    }

    #[test]
    fn apply_blur_dispatches_all_modes_on_a_clone() {
        let original = synthetic(48, 48);
        let rect = Rect::new(8.0, 8.0, 24.0, 24.0);

        let gauss = apply_blur(&original, rect, BlurMode::Gaussian, 4);
        assert!(variance(&gauss, 8, 8, 32, 32) < variance(&original, 8, 8, 32, 32));
        assert_eq!(original, original.clone());
        assert_ne!(gauss, original);
        // Outside the rect is untouched for every mode.
        assert_eq!(gauss.get_pixel(0, 0).0, original.get_pixel(0, 0).0);

        let pixels = apply_blur(&original, rect, BlurMode::Pixelate, 8);
        assert_ne!(pixels.get_pixel(8, 8).0, original.get_pixel(8, 8).0);
        assert_eq!(pixels.get_pixel(40, 40).0, original.get_pixel(40, 40).0);

        let black = apply_blur(&original, rect, BlurMode::Blackout, 0);
        assert_eq!(black.get_pixel(20, 20).0, [0, 0, 0, 255]);
        assert_eq!(black.get_pixel(40, 40).0, original.get_pixel(40, 40).0);
    }

    #[test]
    fn apply_blur_with_invalid_rect_is_a_plain_clone() {
        let original = synthetic(16, 16);
        let out = apply_blur(
            &original,
            Rect::new(f32::NAN, 0.0, 4.0, 4.0),
            BlurMode::Gaussian,
            3,
        );
        assert_eq!(out, original);
        let out = apply_blur(
            &original,
            Rect::new(-50.0, -50.0, 1.0, 1.0),
            BlurMode::Blackout,
            3,
        );
        assert_eq!(out, original);
    }
}
