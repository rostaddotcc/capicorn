//! Pure editor scene: document model, history, blur, compositing.
//! Sans-I/O: data in, data out. No fs/network/OS calls here.

use std::sync::Arc;

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

pub type Img = RgbaImage;
pub type Pt = [f32; 2];
/// x, y, w, h
pub type RectF = [f32; 4];

pub const MAX_UNDO: usize = 100;
pub const MAX_BLUR_RADIUS: u32 = 64;
/// Cap for input-derived image dimensions (hostile-input rule).
pub const MAX_DIM: u32 = 16_384;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Annotation {
    Arrow {
        from: Pt,
        to: Pt,
        color: [u8; 4],
        width: f32,
    },
    Rect {
        rect: RectF,
        color: [u8; 4],
        width: f32,
    },
    Text {
        pos: Pt,
        text: String,
        color: [u8; 4],
        size: f32,
    },
}

impl Annotation {
    /// Rough bounds for hit-testing / moving (image coordinates).
    pub fn bounds(&self) -> RectF {
        match self {
            Self::Arrow {
                from, to, width, ..
            } => {
                let pad = width.max(1.0) * 2.0 + 8.0;
                let x = from[0].min(to[0]) - pad;
                let y = from[1].min(to[1]) - pad;
                let w = (from[0] - to[0]).abs() + pad * 2.0;
                let h = (from[1] - to[1]).abs() + pad * 2.0;
                [x, y, w, h]
            }
            Self::Rect { rect, width, .. } => {
                let pad = width.max(1.0);
                [
                    rect[0] - pad,
                    rect[1] - pad,
                    rect[2] + pad * 2.0,
                    rect[3] + pad * 2.0,
                ]
            }
            Self::Text {
                pos, text, size, ..
            } => {
                let w = text.chars().count() as f32 * size * 0.6 + 4.0;
                [pos[0] - 2.0, pos[1] - 2.0, w, size * 1.4]
            }
        }
    }

    pub fn shifted(&self, delta: Pt) -> Self {
        let mut out = self.clone();
        match &mut out {
            Self::Arrow { from, to, .. } => {
                from[0] += delta[0];
                from[1] += delta[1];
                to[0] += delta[0];
                to[1] += delta[1];
            }
            Self::Rect { rect, .. } => {
                rect[0] += delta[0];
                rect[1] += delta[1];
            }
            Self::Text { pos, .. } => {
                pos[0] += delta[0];
                pos[1] += delta[1];
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub base: Arc<Img>,
    pub anns: Vec<Annotation>,
}

impl Doc {
    pub fn new(base: Img) -> Self {
        Self {
            base: Arc::new(base),
            anns: Vec::new(),
        }
    }
}

/// Undo/redo over cheap snapshots (Arc-shared base image).
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Doc>,
    redo: Vec<Doc>,
}

impl History {
    pub fn record(&mut self, before: &Doc) {
        self.undo.push(before.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &Doc) -> Option<Doc> {
        let prev = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(prev)
    }

    pub fn redo(&mut self, current: &Doc) -> Option<Doc> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// Editor state = document + history. All mutations go through here.
#[derive(Clone, Debug)]
pub struct Editor {
    pub doc: Doc,
    pub history: History,
}

impl Editor {
    pub fn new(base: Img) -> Self {
        Self {
            doc: Doc::new(base),
            history: History::default(),
        }
    }

    pub fn push_ann(&mut self, ann: Annotation) {
        self.history.record(&self.doc);
        self.doc.anns.push(ann);
    }

    pub fn delete_ann(&mut self, idx: usize) {
        if idx < self.doc.anns.len() {
            self.history.record(&self.doc);
            self.doc.anns.remove(idx);
        }
    }

    pub fn move_ann(&mut self, idx: usize, delta: Pt) {
        if let Some(ann) = self.doc.anns.get(idx) {
            let moved = ann.shifted(delta);
            self.history.record(&self.doc);
            if let Some(slot) = self.doc.anns.get_mut(idx) {
                *slot = moved;
            }
        }
    }

    /// Destructive multi-pass blur of a region of the base image.
    pub fn blur_region(&mut self, rect: RectF, radius: u32) {
        let mut img = (*self.doc.base).clone();
        if gaussian_blur_region(&mut img, rect, radius) {
            self.history.record(&self.doc);
            self.doc.base = Arc::new(img);
        }
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.history.undo(&self.doc) {
            self.doc = prev;
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.history.redo(&self.doc) {
            self.doc = next;
        }
    }
}

/// Clamp a float rect to integer image bounds. Returns None for empty results.
pub fn clamp_rect(rect: RectF, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    if w == 0 || h == 0 {
        return None;
    }
    let [x, y, rw, rh] = rect;
    if !x.is_finite() || !y.is_finite() || !rw.is_finite() || !rh.is_finite() {
        return None;
    }
    let x0 = x.floor().clamp(0.0, w as f32) as u32;
    let y0 = y.floor().clamp(0.0, h as f32) as u32;
    let x1 = (x + rw).ceil().clamp(0.0, w as f32) as u32;
    let y1 = (y + rh).ceil().clamp(0.0, h as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1, y1))
}

/// 3-pass separable box blur ≈ gaussian. REPLACES region pixels (no ghosting).
/// Reads clamp to the full image (no dark halo at region edges); writes stay
/// inside the region. Returns false when nothing changed.
pub fn gaussian_blur_region(img: &mut Img, rect: RectF, radius: u32) -> bool {
    let (w, h) = img.dimensions();
    let Some((x0, y0, x1, y1)) = clamp_rect(rect, w, h) else {
        return false;
    };
    let r = radius.clamp(1, MAX_BLUR_RADIUS) as i64;
    let wi = w as i64;
    let hi = h as i64;

    let mut a = img.clone();
    let mut b = img.clone();

    for _pass in 0..3 {
        // Horizontal: a -> b, rows y0..y1, write x0..x1.
        for y in y0..y1 {
            let mut sum = [0u32; 4];
            let mut left = (x0 as i64 - r).max(0);
            let mut right = (x0 as i64 + r).min(wi - 1);
            for x in left..=right {
                let p = a.get_pixel(x as u32, y).0;
                for c in 0..4 {
                    sum[c] += p[c] as u32;
                }
            }
            for x in x0..x1 {
                let count = (right - left + 1) as u32;
                let mut out = [0u8; 4];
                for c in 0..4 {
                    out[c] = (sum[c] / count) as u8;
                }
                b.put_pixel(x, y, Rgba(out));
                // Slide window one pixel right (clamped edges duplicate).
                let new_right = ((x as i64 + 1) + r).min(wi - 1);
                let old_left = (x as i64 - r).max(0);
                if new_right != right {
                    let p = a.get_pixel(new_right as u32, y).0;
                    for c in 0..4 {
                        sum[c] += p[c] as u32;
                    }
                    right = new_right;
                }
                if old_left != left {
                    let p = a.get_pixel(left as u32, y).0;
                    for c in 0..4 {
                        sum[c] = sum[c].saturating_sub(p[c] as u32);
                    }
                    left = old_left;
                }
            }
        }
        // Vertical: b -> a, cols x0..x1, write y0..y1.
        for x in x0..x1 {
            let mut sum = [0u32; 4];
            let mut top = (y0 as i64 - r).max(0);
            let mut bottom = (y0 as i64 + r).min(hi - 1);
            for y in top..=bottom {
                let p = b.get_pixel(x, y as u32).0;
                for c in 0..4 {
                    sum[c] += p[c] as u32;
                }
            }
            for y in y0..y1 {
                let count = (bottom - top + 1) as u32;
                let mut out = [0u8; 4];
                for c in 0..4 {
                    out[c] = (sum[c] / count) as u8;
                }
                a.put_pixel(x, y, Rgba(out));
                let new_bottom = ((y as i64 + 1) + r).min(hi - 1);
                let old_top = (y as i64 - r).max(0);
                if new_bottom != bottom {
                    let p = b.get_pixel(x, new_bottom as u32).0;
                    for c in 0..4 {
                        sum[c] += p[c] as u32;
                    }
                    bottom = new_bottom;
                }
                if old_top != top {
                    let p = b.get_pixel(x, top as u32).0;
                    for c in 0..4 {
                        sum[c] = sum[c].saturating_sub(p[c] as u32);
                    }
                    top = old_top;
                }
            }
        }
    }

    // Copy the blurred region back; pixels outside stay untouched.
    for y in y0..y1 {
        for x in x0..x1 {
            img.put_pixel(x, y, *a.get_pixel(x, y));
        }
    }
    true
}

/// Arrowhead triangle: tip at `to`, two base points behind it.
pub fn arrow_head(from: Pt, to: Pt, head_len: f32) -> [Pt; 3] {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 || !len.is_finite() {
        return [to, to, to];
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let half = head_len * 0.5;
    let bx = to[0] - ux * head_len;
    let by = to[1] - uy * head_len;
    [
        to,
        [bx + px * half, by + py * half],
        [bx - px * half, by - py * half],
    ]
}

fn blend_px(img: &mut Img, x: i64, y: i64, color: [u8; 4], coverage: f32) {
    let (w, h) = img.dimensions();
    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 || coverage <= 0.0 {
        return;
    }
    let cov = coverage.clamp(0.0, 1.0) * (color[3] as f32 / 255.0);
    let dst = img.get_pixel(x as u32, y as u32).0;
    let mut out = [0u8; 4];
    for c in 0..3 {
        out[c] = (color[c] as f32 * cov + dst[c] as f32 * (1.0 - cov)).round() as u8;
    }
    out[3] = (cov * 255.0 + dst[3] as f32 * (1.0 - cov)).round() as u8;
    img.put_pixel(x as u32, y as u32, Rgba(out));
}

fn draw_line(img: &mut Img, from: Pt, to: Pt, color: [u8; 4], width: f32) {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let len = (dx * dx + dy * dy).sqrt().max(1e-3);
    let steps = (len * 2.0).ceil() as i64;
    let half = width.max(1.0) / 2.0;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let cx = from[0] + dx * t;
        let cy = from[1] + dy * t;
        let r = half.ceil() as i64;
        for oy in -r..=r {
            for ox in -r..=r {
                let d = ((ox as f32) * (ox as f32) + (oy as f32) * (oy as f32)).sqrt();
                let cov = (half + 0.5 - d).clamp(0.0, 1.0);
                blend_px(img, cx as i64 + ox, cy as i64 + oy, color, cov);
            }
        }
    }
}

fn draw_rect_outline(img: &mut Img, rect: RectF, color: [u8; 4], width: f32) {
    let [x, y, w, h] = rect;
    let p = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    for i in 0..4 {
        draw_line(img, p[i], p[(i + 1) % 4], color, width);
    }
}

fn fill_triangle(img: &mut Img, tri: [Pt; 3], color: [u8; 4]) {
    let min_x = tri.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor() as i64;
    let max_x = tri.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as i64;
    let min_y = tri.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor() as i64;
    let max_y = tri.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as i64;
    let area = |a: Pt, b: Pt, c: Pt| (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
    let total = area(tri[0], tri[1], tri[2]);
    if total.abs() < 1e-6 {
        return;
    }
    // Bound the scan to something sane even for hostile coordinates.
    let (iw, ih) = img.dimensions();
    let max_x = max_x.min(iw as i64 + 1);
    let max_y = max_y.min(ih as i64 + 1);
    let min_x = min_x.max(-1);
    let min_y = min_y.max(-1);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = area(tri[1], tri[2], p) / total;
            let w1 = area(tri[2], tri[0], p) / total;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                blend_px(img, x, y, color, 1.0);
            }
        }
    }
}

/// CPU composite: base + annotations. Text uses the supplied font (egui's
/// bundled face in the app); without a font, text is skipped (spike export note).
pub fn composite<F: ab_glyph::Font>(doc: &Doc, font: Option<&F>) -> Img {
    let mut img = (*doc.base).clone();
    for ann in &doc.anns {
        match ann {
            Annotation::Arrow {
                from,
                to,
                color,
                width,
            } => {
                let head_len = (width.max(1.0) * 4.0).clamp(8.0, 48.0);
                draw_line(&mut img, *from, *to, *color, *width);
                fill_triangle(&mut img, arrow_head(*from, *to, head_len), *color);
            }
            Annotation::Rect { rect, color, width } => {
                draw_rect_outline(&mut img, *rect, *color, *width);
            }
            Annotation::Text {
                pos,
                text,
                color,
                size,
            } => {
                if let Some(font) = font {
                    draw_text(&mut img, font, *pos, text, *color, *size);
                }
            }
        }
    }
    img
}

fn draw_text<F: ab_glyph::Font>(
    img: &mut Img,
    font: &F,
    pos: Pt,
    text: &str,
    color: [u8; 4],
    size: f32,
) {
    use ab_glyph::PxScale;
    let size = size.clamp(4.0, 512.0);
    let scale = PxScale::from(size);
    let ascent = font.ascent_unscaled() * scale.y;
    let mut x = pos[0];
    let baseline = pos[1] + ascent;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(x, baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|px, py, cov| {
                blend_px(
                    img,
                    bounds.min.x as i64 + px as i64,
                    bounds.min.y as i64 + py as i64,
                    color,
                    cov,
                );
            });
        }
        x += font.h_advance_unscaled(gid) * scale.x;
        if x > img.width() as f32 + size {
            break; // hostile-input bound
        }
    }
}

/// Synthetic test image: gradient + a few shapes, so the spike opens with
/// something annotatable and the blur has visible detail to chew on.
pub fn synthetic_image(w: u32, h: u32) -> Img {
    let w = w.clamp(1, MAX_DIM);
    let h = h.clamp(1, MAX_DIM);
    let mut img = Img::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let r = (40.0 + 180.0 * fx) as u8;
            let g = (70.0 + 120.0 * fy) as u8;
            let b = (160.0 + 60.0 * (1.0 - fx)) as u8;
            // Fine checker detail so blur is obvious.
            let check = ((x / 8) + (y / 8)) % 2 == 0;
            let boost = if check { 22 } else { 0 };
            img.put_pixel(
                x,
                y,
                Rgba([
                    r.saturating_add(boost),
                    g.saturating_add(boost),
                    b.saturating_add(boost),
                    255,
                ]),
            );
        }
    }
    // A white plate and a dark disc: recognizable targets.
    for y in (h / 5)..(h / 5 + h / 4) {
        for x in (w / 6)..(w / 6 + w / 3) {
            img.put_pixel(x, y, Rgba([245, 245, 245, 255]));
        }
    }
    let (cx, cy, rad) = (w as f32 * 0.72, h as f32 * 0.62, (w.min(h) as f32) * 0.16);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d < rad {
                img.put_pixel(x, y, Rgba([30, 30, 34, 255]));
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variance(img: &Img, x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
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
        let mean = sum / n;
        sum2 / n - mean * mean
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut ed = Editor::new(synthetic_image(64, 64));
        let ann = Annotation::Rect {
            rect: [4.0, 4.0, 20.0, 20.0],
            color: [255, 0, 0, 255],
            width: 2.0,
        };
        ed.push_ann(ann.clone());
        assert_eq!(ed.doc.anns.len(), 1);
        ed.undo();
        assert!(ed.doc.anns.is_empty());
        ed.redo();
        assert_eq!(ed.doc.anns, vec![ann]);
        // New edit after undo clears redo.
        ed.undo();
        ed.push_ann(Annotation::Rect {
            rect: [0.0, 0.0, 5.0, 5.0],
            color: [0, 0, 255, 255],
            width: 1.0,
        });
        assert!(!ed.history.can_redo());
    }

    #[test]
    fn undo_restores_blurred_base() {
        let mut ed = Editor::new(synthetic_image(64, 64));
        let before = ed.doc.base.clone();
        ed.blur_region([8.0, 8.0, 40.0, 40.0], 4);
        assert!(!Arc::ptr_eq(&before, &ed.doc.base));
        ed.undo();
        assert!(Arc::ptr_eq(&before, &ed.doc.base));
    }

    #[test]
    fn history_is_bounded() {
        let mut ed = Editor::new(synthetic_image(8, 8));
        for i in 0..(MAX_UNDO + 25) {
            ed.push_ann(Annotation::Rect {
                rect: [0.0, 0.0, i as f32, 1.0],
                color: [0, 0, 0, 255],
                width: 1.0,
            });
        }
        assert!(ed.history.undo.len() <= MAX_UNDO);
    }

    #[test]
    fn blur_reduces_variance_and_replaces_pixels() {
        let mut img = synthetic_image(96, 96);
        let v_before = variance(&img, 10, 10, 80, 80);
        assert!(gaussian_blur_region(&mut img, [10.0, 10.0, 70.0, 70.0], 5));
        let v_after = variance(&img, 10, 10, 80, 80);
        assert!(
            v_after < v_before * 0.5,
            "blur must smooth: {v_before} -> {v_after}"
        );
    }

    #[test]
    fn blur_uniform_region_is_invariant() {
        let mut img = Img::from_pixel(48, 48, Rgba([120, 130, 140, 255]));
        let copy = img.clone();
        gaussian_blur_region(&mut img, [8.0, 8.0, 32.0, 32.0], 6);
        assert_eq!(img, copy);
    }

    #[test]
    fn blur_outside_pixels_untouched() {
        let mut img = synthetic_image(64, 64);
        let copy = img.clone();
        gaussian_blur_region(&mut img, [20.0, 20.0, 20.0, 20.0], 3);
        for y in 0..64 {
            for x in 0..64 {
                if !(19..=41).contains(&x) || !(19..=41).contains(&y) {
                    assert_eq!(
                        img.get_pixel(x, y),
                        copy.get_pixel(x, y),
                        "outside pixel ({x},{y}) changed"
                    );
                }
            }
        }
    }

    #[test]
    fn blur_hostile_rects_never_panic() {
        let mut img = synthetic_image(32, 32);
        for rect in [
            [-1e9, -1e9, 1e9, 1e9],
            [f32::NAN, 0.0, 10.0, 10.0],
            [100.0, 100.0, 10.0, 10.0],
            [0.0, 0.0, 0.0, 0.0],
            [10.0, 10.0, -5.0, -5.0],
        ] {
            let _ = gaussian_blur_region(&mut img, rect, 7);
        }
        let mut tiny = Img::new(1, 1);
        let _ = gaussian_blur_region(&mut tiny, [0.0, 0.0, 1.0, 1.0], 64);
        let mut empty = Img::new(0, 0);
        assert!(!gaussian_blur_region(&mut empty, [0.0, 0.0, 1.0, 1.0], 4));
    }

    #[test]
    fn blur_is_deterministic() {
        let a = {
            let mut img = synthetic_image(80, 80);
            gaussian_blur_region(&mut img, [5.0, 5.0, 60.0, 60.0], 4);
            img
        };
        let b = {
            let mut img = synthetic_image(80, 80);
            gaussian_blur_region(&mut img, [5.0, 5.0, 60.0, 60.0], 4);
            img
        };
        assert_eq!(a, b);
    }

    #[test]
    fn arrow_head_geometry() {
        let from = [0.0, 0.0];
        let to = [100.0, 0.0];
        let [tip, b1, b2] = arrow_head(from, to, 20.0);
        assert_eq!(tip, to);
        assert!((b1[0] - 80.0).abs() < 1e-3);
        assert!((b2[0] - 80.0).abs() < 1e-3);
        assert!((b1[1] - 10.0).abs() < 1e-3);
        assert!((b2[1] + 10.0).abs() < 1e-3);
        // Degenerate arrow does not panic.
        let z = arrow_head([5.0, 5.0], [5.0, 5.0], 10.0);
        assert_eq!(z, [[5.0, 5.0]; 3]);
    }

    #[test]
    fn composite_draws_rect_and_arrow_pixels() {
        let mut doc = Doc::new(Img::from_pixel(64, 64, Rgba([255, 255, 255, 255])));
        doc.anns.push(Annotation::Rect {
            rect: [10.0, 10.0, 30.0, 30.0],
            color: [255, 0, 0, 255],
            width: 2.0,
        });
        doc.anns.push(Annotation::Arrow {
            from: [5.0, 55.0],
            to: [55.0, 55.0],
            color: [0, 0, 255, 255],
            width: 2.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out.get_pixel(10, 25).0[0], 255); // red edge
        assert!(out.get_pixel(10, 25).0[1] < 60);
        assert!(out.get_pixel(30, 55).0[2] > 200); // blue shaft
        assert!(out.get_pixel(55, 55).0[2] > 200); // head tip
        assert_eq!(out.get_pixel(0, 0).0, [255, 255, 255, 255]); // untouched
    }

    #[test]
    fn move_and_delete() {
        let mut ed = Editor::new(synthetic_image(16, 16));
        ed.push_ann(Annotation::Rect {
            rect: [1.0, 1.0, 4.0, 4.0],
            color: [0, 0, 0, 255],
            width: 1.0,
        });
        ed.move_ann(0, [2.0, 3.0]);
        assert_eq!(ed.doc.anns[0].bounds()[0], 1.0 - 1.0 + 2.0);
        ed.delete_ann(0);
        assert!(ed.doc.anns.is_empty());
        ed.delete_ann(99); // out of range: no-op, no panic
        ed.move_ann(99, [1.0, 1.0]);
    }

    #[test]
    fn clamp_rect_edges() {
        assert_eq!(
            clamp_rect([-5.0, -5.0, 10.0, 10.0], 8, 8),
            Some((0, 0, 5, 5))
        );
        assert_eq!(
            clamp_rect([2.0, 2.0, 100.0, 100.0], 8, 8),
            Some((2, 2, 8, 8))
        );
        assert_eq!(clamp_rect([9.0, 9.0, 2.0, 2.0], 8, 8), None);
        assert_eq!(clamp_rect([0.0, 0.0, 4.0, 4.0], 0, 0), None);
    }
}
