#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L2 — image algorithms: region blur (3-pass separable box ≈ gaussian),
//! pixelate, irreversible blackout, flood fill, region resampling, and the
//! mode dispatch used by the compositor and export.
//!
//! Every entry point is hostile-input safe: rects are clamped with
//! [`Rect::clamp_to`], radii/blocks are capped, output dimensions are capped,
//! and all buffer access is bounds-checked with `.get()`. Work scales with the
//! region (+halo), not the whole image.

use snagcraft_color::Rgba;
use snagcraft_doc::BlurMode;
use snagcraft_geom::{Point, Rect};
use snagcraft_raster::Surface;

/// Smallest usable gaussian radius / pixelate block (live parity, `blur.ts:20`).
pub const BLUR_RADIUS_MIN: u32 = 2;
/// Largest gaussian radius / pixelate block (live parity, `blur.ts:21`).
pub const BLUR_RADIUS_MAX: u32 = 60;
/// Default radius/block for a fresh blur region (live parity, `blur.ts:19`).
pub const BLUR_RADIUS_DEFAULT: u32 = 12;
/// Redaction honesty: any blur under this is raised to it, so soft settings
/// cannot leave legible text (`blur.ts:24-25`).
pub const SECURE_BLUR_MIN: u32 = 10;
/// Blackout ink — `#111827` (`blur.ts:32`), opaque.
pub const BLACKOUT_INK: [u8; 4] = [0x11, 0x18, 0x27, 255];
/// Output dimension cap for [`resize_region`] (hostile-input rule).
pub const MAX_RESIZE_DIM: u32 = 4096;

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

/// Effective gaussian radius: clamp to `BLUR_RADIUS_MIN..=BLUR_RADIUS_MAX`, then
/// raise anything under [`SECURE_BLUR_MIN`] to it (redaction honesty).
pub fn effective_blur_radius(radius: u32) -> u32 {
    radius
        .clamp(BLUR_RADIUS_MIN, BLUR_RADIUS_MAX)
        .max(SECURE_BLUR_MIN)
}

/// 3-pass separable box blur ≈ gaussian. REPLACES the region pixels (no
/// ghosting of the original), reads clamp to the full image (radius-wide halo,
/// so no dark fringe at region edges), and writes only inside the region.
/// `radius` is normalised by [`effective_blur_radius`]. Returns false when the
/// region was empty/invalid or every pixel already matched the blurred result.
pub fn gaussian_blur_region(img: &mut Surface, rect: Rect, radius: u32) -> bool {
    let (w, h) = img.dimensions();
    let Some((x0, y0, x1, y1)) = rect.clamp_to(w, h) else {
        return false;
    };
    let r = effective_blur_radius(radius) as usize;

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

/// Average-colour pixelation of `rect`. `block` is clamped to
/// `BLUR_RADIUS_MIN..=BLUR_RADIUS_MAX` (same range as the gaussian radius) and
/// blocks are aligned to the (clamped) region origin; partial edge blocks are
/// averaged over the pixels that actually exist inside the region.
/// Returns false when nothing changed.
pub fn pixelate_region(img: &mut Surface, rect: Rect, block: u32) -> bool {
    let (w, h) = img.dimensions();
    let Some((x0, y0, x1, y1)) = rect.clamp_to(w, h) else {
        return false;
    };
    let b = block.clamp(BLUR_RADIUS_MIN, BLUR_RADIUS_MAX);
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

/// Solid opaque [`BLACKOUT_INK`] fill — irreversible redaction (no recoverable
/// pixels). Returns false when nothing changed.
pub fn blackout_region(img: &mut Surface, rect: Rect) -> bool {
    let (w, h) = img.dimensions();
    let Some((x0, y0, x1, y1)) = rect.clamp_to(w, h) else {
        return false;
    };
    let mut changed = false;
    for y in y0..y1 {
        for x in x0..x1 {
            let dst = img.get_pixel_mut(x, y);
            if dst.0 != BLACKOUT_INK {
                dst.0 = BLACKOUT_INK;
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

/// Scanline seed flood fill. Returns a filled clone of `img` (input untouched).
///
/// A pixel matches when the maximum per-channel absolute difference against the
/// seed pixel is `<= tolerance` (so `tolerance = 255` fills everything
/// reachable, and `0` fills the exact seed colour). Out-of-bounds/non-finite
/// seeds and zero-sized images return an unchanged clone. Work is bounded by
/// `w * h` filled pixels — each pixel is filled at most once.
pub fn flood_fill(img: &Surface, seed: Point, color: Rgba, tolerance: u8) -> Surface {
    let mut out = img.clone();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || !seed.x.is_finite() || !seed.y.is_finite() {
        return out;
    }
    let fx = seed.x.floor();
    let fy = seed.y.floor();
    if fx < 0.0 || fy < 0.0 || fx >= w as f32 || fy >= h as f32 {
        return out;
    }
    let (sx, sy) = (fx as u32, fy as u32);

    let target = img.get_pixel(sx, sy).0;
    let tol = f32::from(tolerance);
    let matches = |p: [u8; 4]| -> bool {
        p.iter()
            .zip(target.iter())
            .all(|(a, b)| (f32::from(*a) - f32::from(*b)).abs() <= tol)
    };

    let total = (w as usize) * (h as usize);
    let mut visited = vec![false; total];
    let fill = color.to_array();
    let mut stack: Vec<(u32, u32)> = vec![(sx, sy)];

    while let Some((x, y)) = stack.pop() {
        let idx = (y as usize) * (w as usize) + (x as usize);
        if visited.get(idx).copied().unwrap_or(true) {
            continue;
        }
        if !matches(img.get_pixel(x, y).0) {
            visited[idx] = true;
            continue;
        }
        // Widen to the full horizontal span of matching pixels.
        let row = (y as usize) * (w as usize);
        let mut x0 = x;
        while x0 > 0
            && !visited.get(row + x0 as usize - 1).copied().unwrap_or(true)
            && matches(img.get_pixel(x0 - 1, y).0)
        {
            x0 -= 1;
        }
        let mut x1 = x;
        while x1 + 1 < w
            && !visited.get(row + x1 as usize + 1).copied().unwrap_or(true)
            && matches(img.get_pixel(x1 + 1, y).0)
        {
            x1 += 1;
        }
        for xx in x0..=x1 {
            let i = (y as usize) * (w as usize) + (xx as usize);
            visited[i] = true;
            out.get_pixel_mut(xx, y).0 = fill;
        }
        // Queue the first matching pixel of every run directly above/below.
        for ny in [y.checked_sub(1), (y + 1 < h).then_some(y + 1)]
            .into_iter()
            .flatten()
        {
            let mut run = false;
            for xx in x0..=x1 {
                let i = (ny as usize) * (w as usize) + (xx as usize);
                let already = visited.get(i).copied().unwrap_or(true);
                if !already && matches(img.get_pixel(xx, ny).0) {
                    if !run && stack.len() < total {
                        stack.push((xx, ny));
                    }
                    run = true;
                } else {
                    run = false;
                }
            }
        }
    }
    out
}

/// Bilinear resample of `src` (clamped to the source image) into a fresh
/// `dst_w`×`dst_h` surface. Dimensions are capped at [`MAX_RESIZE_DIM`];
/// either dimension `0` (or an empty/non-finite source) yields an empty or
/// fully transparent surface of the requested size. Edge pixels clamp, so no
/// dark halo appears at the border.
pub fn resize_region(img: &Surface, src: Rect, dst_w: u32, dst_h: u32) -> Surface {
    let dw = dst_w.min(MAX_RESIZE_DIM);
    let dh = dst_h.min(MAX_RESIZE_DIM);
    let mut out = Surface::new(dw, dh);
    if dw == 0 || dh == 0 {
        return out;
    }
    let (sw_img, sh_img) = img.dimensions();
    if sw_img == 0 || sh_img == 0 || !src.is_finite() {
        return out;
    }
    let x0 = src.x.floor().clamp(0.0, sw_img as f32 - 1.0);
    let y0 = src.y.floor().clamp(0.0, sh_img as f32 - 1.0);
    let x1 = (src.x + src.w).ceil().clamp(x0 + 1.0, sw_img as f32);
    let y1 = (src.y + src.h).ceil().clamp(y0 + 1.0, sh_img as f32);
    let (x0, y0) = (x0 as u32, y0 as u32);
    let (sw, sh) = (x1 as u32 - x0, y1 as u32 - y0);
    let swf = sw as f32;
    let shf = sh as f32;
    let dwf = dw as f32;
    let dhf = dh as f32;

    for dy in 0..dh {
        // Pixel-centre sampling: dst centre -> src coordinate, edges clamped.
        let v = ((dy as f32 + 0.5) / dhf * shf - 0.5).clamp(0.0, shf - 1.0);
        let sy0 = v.floor() as u32;
        let sy1 = (sy0 + 1).min(sh - 1);
        let fy = v - sy0 as f32;
        for dx in 0..dw {
            let u = ((dx as f32 + 0.5) / dwf * swf - 0.5).clamp(0.0, swf - 1.0);
            let sx0 = u.floor() as u32;
            let sx1 = (sx0 + 1).min(sw - 1);
            let fx = u - sx0 as f32;
            let p00 = img.get_pixel(x0 + sx0, y0 + sy0).0;
            let p10 = img.get_pixel(x0 + sx1, y0 + sy0).0;
            let p01 = img.get_pixel(x0 + sx0, y0 + sy1).0;
            let p11 = img.get_pixel(x0 + sx1, y0 + sy1).0;
            let mut p = [0u8; 4];
            for c in 0..4 {
                let top = f32::from(p00[c]) * (1.0 - fx) + f32::from(p10[c]) * fx;
                let bottom = f32::from(p01[c]) * (1.0 - fx) + f32::from(p11[c]) * fx;
                p[c] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
            }
            out.get_pixel_mut(dx, dy).0 = p;
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
    fn constants_match_live_parity() {
        assert_eq!(BLUR_RADIUS_MIN, 2);
        assert_eq!(BLUR_RADIUS_MAX, 60);
        assert_eq!(BLUR_RADIUS_DEFAULT, 12);
        assert_eq!(SECURE_BLUR_MIN, 10);
        assert_eq!(BLACKOUT_INK, [0x11, 0x18, 0x27, 255]);
    }

    #[test]
    fn blur_reduces_variance() {
        let mut img = checkerboard(96, 96);
        let before = variance(&img, 10, 10, 80, 80);
        assert!(before > 100.0, "test image must carry detail: {before}");
        assert!(gaussian_blur_region(
            &mut img,
            Rect::new(10.0, 10.0, 70.0, 70.0),
            10
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
            10
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
        let _ = gaussian_blur_region(&mut tiny, Rect::new(0.0, 0.0, 1.0, 1.0), 60);
        let mut empty = Surface::new(0, 0);
        assert!(!gaussian_blur_region(
            &mut empty,
            Rect::new(0.0, 0.0, 1.0, 1.0),
            4
        ));
    }

    #[test]
    fn blur_radius_follows_live_clamp_and_secure_minimum() {
        // Clamp 2..=60, then raise anything under SECURE_BLUR_MIN to 10.
        assert_eq!(effective_blur_radius(0), 10);
        assert_eq!(effective_blur_radius(1), 10);
        assert_eq!(effective_blur_radius(2), 10);
        assert_eq!(effective_blur_radius(3), 10);
        assert_eq!(effective_blur_radius(9), 10);
        assert_eq!(effective_blur_radius(10), 10);
        assert_eq!(effective_blur_radius(12), 12);
        assert_eq!(effective_blur_radius(60), 60);
        assert_eq!(effective_blur_radius(61), 60);
        assert_eq!(effective_blur_radius(999), 60);
        assert_eq!(effective_blur_radius(u32::MAX), 60);

        let blur = |r: u32| {
            let mut img = synthetic(48, 48);
            gaussian_blur_region(&mut img, Rect::new(0.0, 0.0, 48.0, 48.0), r);
            img
        };
        // radius 3 -> effective 10; radius 1 -> 2 -> 10.
        assert_eq!(blur(3), blur(10));
        assert_eq!(blur(1), blur(10));
        assert_eq!(blur(0), blur(10));
        // radius 999 -> 60.
        assert_eq!(blur(999), blur(60));
        // ...and 60 really is a stronger blur than 10.
        assert!(variance(&blur(60), 0, 0, 48, 48) < variance(&blur(10), 0, 0, 48, 48));
    }

    #[test]
    fn blur_is_deterministic() {
        let run = || {
            let mut img = synthetic(80, 80);
            gaussian_blur_region(&mut img, Rect::new(5.0, 5.0, 60.0, 60.0), 12);
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
        // block below 2 clamps to 2; above the cap clamps to 60 (same range as
        // the gaussian radius).
        let mut b2 = synthetic(24, 24);
        let mut b0 = synthetic(24, 24);
        let mut b60 = synthetic(24, 24);
        let mut bmax = synthetic(24, 24);
        pixelate_region(&mut b2, Rect::new(0.0, 0.0, 24.0, 24.0), 2);
        pixelate_region(&mut b0, Rect::new(0.0, 0.0, 24.0, 24.0), 0);
        pixelate_region(&mut b60, Rect::new(0.0, 0.0, 24.0, 24.0), 60);
        pixelate_region(&mut bmax, Rect::new(0.0, 0.0, 24.0, 24.0), u32::MAX);
        assert_eq!(b0, b2);
        assert_eq!(bmax, b60);
        let mut empty = Surface::new(0, 0);
        assert!(!pixelate_region(
            &mut empty,
            Rect::new(0.0, 0.0, 4.0, 4.0),
            4
        ));
    }

    #[test]
    fn blackout_fills_security_ink_and_is_false_when_already_inked() {
        let mut img = filled(Size::new(32, 32), Rgba::new(10, 200, 30, 128));
        assert!(blackout_region(&mut img, Rect::new(8.0, 8.0, 8.0, 8.0)));
        for y in 8..16 {
            for x in 8..16 {
                assert_eq!(img.get_pixel(x, y).0, BLACKOUT_INK);
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

        let gauss = apply_blur(&original, rect, BlurMode::Gaussian, 10);
        assert!(variance(&gauss, 8, 8, 32, 32) < variance(&original, 8, 8, 32, 32));
        assert_eq!(original, original.clone());
        assert_ne!(gauss, original);
        // Outside the rect is untouched for every mode.
        assert_eq!(gauss.get_pixel(0, 0).0, original.get_pixel(0, 0).0);

        let pixels = apply_blur(&original, rect, BlurMode::Pixelate, 8);
        assert_ne!(pixels.get_pixel(8, 8).0, original.get_pixel(8, 8).0);
        assert_eq!(pixels.get_pixel(40, 40).0, original.get_pixel(40, 40).0);

        let black = apply_blur(&original, rect, BlurMode::Blackout, 0);
        assert_eq!(black.get_pixel(20, 20).0, BLACKOUT_INK);
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

    // ------------------------------------------------------------ flood fill

    /// White sheet with a black 2px frame, so a fill can be contained.
    fn framed(w: u32, h: u32) -> Surface {
        let mut img = filled(Size::new(w, h), Rgba::WHITE);
        for x in 0..w {
            for y in [0, 1, h - 2, h - 1] {
                img.get_pixel_mut(x, y).0 = Rgba::BLACK.to_array();
            }
        }
        for y in 0..h {
            for x in [0, 1, w - 2, w - 1] {
                img.get_pixel_mut(x, y).0 = Rgba::BLACK.to_array();
            }
        }
        img
    }

    #[test]
    fn flood_fill_stops_at_a_sealed_border() {
        let img = framed(32, 32);
        let red = Rgba::new(255, 0, 0, 255);
        let out = flood_fill(&img, Point::new(16.0, 16.0), red, 0);
        assert_eq!(out.get_pixel(16, 16).0, red.to_array());
        assert_eq!(out.get_pixel(2, 2).0, red.to_array(), "corner reached");
        assert_eq!(out.get_pixel(29, 29).0, red.to_array());
        // The frame itself never matches white, so it survives.
        assert_eq!(out.get_pixel(0, 0).0, Rgba::BLACK.to_array());
        assert_eq!(out.get_pixel(31, 16).0, Rgba::BLACK.to_array());
        // Input untouched.
        assert_eq!(img.get_pixel(16, 16).0, Rgba::WHITE.to_array());
    }

    #[test]
    fn flood_fill_tolerance_accepts_near_colours() {
        let mut img = filled(Size::new(16, 16), Rgba::new(100, 100, 100, 255));
        // A wall of (100,100,100); one stray pixel at +12 on each channel.
        img.get_pixel_mut(8, 8).0 = [112, 108, 112, 255];
        let red = Rgba::new(255, 0, 0, 255);
        // Tolerance 8: the stray pixel is outside, so it stays.
        let tight = flood_fill(&img, Point::new(0.0, 0.0), red, 8);
        assert_eq!(tight.get_pixel(8, 8).0, [112, 108, 112, 255]);
        // Tolerance 12: it is inside the tolerance and gets filled too.
        let loose = flood_fill(&img, Point::new(0.0, 0.0), red, 12);
        assert_eq!(loose.get_pixel(8, 8).0, red.to_array());
        assert_eq!(loose.get_pixel(15, 15).0, red.to_array());

        // Tolerance 0 only fills the exact seed colour.
        let only_exact = flood_fill(&img, Point::new(0.0, 0.0), red, 0);
        assert_eq!(only_exact.get_pixel(8, 8).0, [112, 108, 112, 255]);
    }

    #[test]
    fn flood_fill_tolerance_255_fills_everything_reachable() {
        let img = synthetic(16, 16);
        let out = flood_fill(&img, Point::new(0.0, 0.0), Rgba::new(1, 2, 3, 255), 255);
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(out.get_pixel(x, y).0, [1, 2, 3, 255], "({x},{y})");
            }
        }
    }

    #[test]
    fn flood_fill_hostile_seeds_return_an_unchanged_clone() {
        let img = synthetic(8, 8);
        for seed in [
            Point::new(-1.0, 3.0),
            Point::new(3.0, -0.5),
            Point::new(8.0, 3.0),
            Point::new(3.0, 8.0),
            Point::new(f32::NAN, 0.0),
            Point::new(0.0, f32::INFINITY),
        ] {
            assert_eq!(flood_fill(&img, seed, Rgba::new(9, 9, 9, 255), 10), img);
        }
        let empty = Surface::new(0, 0);
        assert_eq!(
            flood_fill(&empty, Point::new(0.0, 0.0), Rgba::new(9, 9, 9, 255), 10),
            empty
        );
        // Fractional seeds floor into the pixel.
        let out = flood_fill(&img, Point::new(3.9, 2.2), Rgba::new(9, 9, 9, 255), 255);
        assert_eq!(out.get_pixel(0, 0).0, [9, 9, 9, 255]);
    }

    #[test]
    fn flood_fill_is_bounded_on_a_large_flat_image() {
        // 512x512 all-white: the fill must terminate and cover everything.
        let img = filled(Size::new(512, 512), Rgba::WHITE);
        let out = flood_fill(&img, Point::new(256.0, 256.0), Rgba::BLACK, 0);
        assert_eq!(out.get_pixel(0, 0).0, Rgba::BLACK.to_array());
        assert_eq!(out.get_pixel(511, 511).0, Rgba::BLACK.to_array());
    }

    // --------------------------------------------------------- resize_region

    #[test]
    fn resize_region_bilinearly_scales_up() {
        // 2x2: black, white / white, black — the upscaled centre must be the
        // 50% blend (127/128), not a nearest-neighbour hard edge.
        let mut img = filled(Size::new(2, 2), Rgba::BLACK);
        img.get_pixel_mut(1, 0).0 = Rgba::WHITE.to_array();
        img.get_pixel_mut(0, 1).0 = Rgba::WHITE.to_array();
        let out = resize_region(&img, Rect::new(0.0, 0.0, 2.0, 2.0), 8, 8);
        assert_eq!(out.dimensions(), (8, 8));
        let mid = out.get_pixel(4, 4).0;
        assert!(
            (100..=155).contains(&mid[0]),
            "centre should be a blend, got {mid:?}"
        );
        // Corners clamp to the source corner colours.
        assert_eq!(out.get_pixel(0, 0).0, Rgba::BLACK.to_array());
        assert_eq!(out.get_pixel(7, 0).0, Rgba::WHITE.to_array());
    }

    #[test]
    fn resize_region_downscales_and_averages() {
        let img = checkerboard(64, 64);
        let out = resize_region(&img, Rect::new(0.0, 0.0, 64.0, 64.0), 8, 8);
        assert_eq!(out.dimensions(), (8, 8));
        let p = out.get_pixel(4, 4).0;
        assert!(
            (100..=160).contains(&p[0]),
            "downscale must average the checkerboard, got {p:?}"
        );
    }

    #[test]
    fn resize_region_clamps_src_and_caps_dst() {
        let img = synthetic(16, 16);
        // Source rect hanging off the image: clamped, not panicking.
        let out = resize_region(&img, Rect::new(-8.0, -8.0, 16.0, 16.0), 4, 4);
        assert_eq!(out.dimensions(), (4, 4));
        // Zero source/wild rects produce a transparent surface of the right size.
        let out = resize_region(&img, Rect::new(f32::NAN, 0.0, 4.0, 4.0), 3, 3);
        assert_eq!(out.dimensions(), (3, 3));
        assert_eq!(out.get_pixel(1, 1).0, [0, 0, 0, 0]);
        // dst 0 -> empty
        assert_eq!(
            resize_region(&img, Rect::new(0.0, 0.0, 8.0, 8.0), 0, 4).dimensions(),
            (0, 4)
        );
        // dst capped at MAX_RESIZE_DIM
        let out = resize_region(&img, Rect::new(0.0, 0.0, 4.0, 4.0), u32::MAX, 8);
        assert_eq!(out.dimensions(), (MAX_RESIZE_DIM, 8));
        // Empty source image -> transparent output.
        let empty = Surface::new(0, 0);
        let out = resize_region(&empty, Rect::new(0.0, 0.0, 4.0, 4.0), 2, 2);
        assert_eq!(out.dimensions(), (2, 2));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 0]);
    }
}
