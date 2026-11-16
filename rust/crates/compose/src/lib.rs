#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L3 — CPU compositor: base surface + annotation objects, in vector order.
//!
//! Used for export and for any consumer that cannot afford a GPU pass. All
//! drawing goes through [`blend_px`], which clips to the image, and every
//! input-derived number (rects, sizes, widths, radii, dim, string length) is
//! clamped before use.
//!
//! W1 gaps, deliberate: `bold`/`italic` flags on `Text` are stored but render
//! regular, and without a font (`font = None`) all text paths are skipped.

use ab_glyph::{Font, PxScale, ScaleFont};
use snagcraft_algo::apply_blur;
use snagcraft_color::Rgba;
use snagcraft_doc::{AnnotObject, Document, ShapeKind};
use snagcraft_geom::{Point, Rect};
use snagcraft_raster::Surface;

/// Upper bound on line-walk steps (hostile coordinates still cost O(1)).
const MAX_LINE_STEPS: i64 = 65_536;
/// Stroke width cap in px.
const MAX_STROKE: f32 = 256.0;
/// Circle/sphere radius cap in px.
const MAX_RADIUS: f32 = 4096.0;
const MIN_TEXT_SIZE: f32 = 4.0;
const MAX_TEXT_SIZE: f32 = 512.0;
/// Polyline segments used to approximate an ellipse outline.
const ELLIPSE_SEGMENTS: u32 = 64;
/// Callout border width.
const CALLOUT_STROKE: f32 = 2.0;

/// Composite `doc` into a fresh surface: base clone, then every object in
/// vector order (so later objects paint over earlier ones). Never mutates `doc`.
pub fn composite<F: Font>(doc: &Document, font: Option<&F>) -> Surface {
    let mut img: Surface = doc.base.as_ref().clone();

    for obj in &doc.objects {
        match obj {
            AnnotObject::Arrow {
                from,
                to,
                color,
                width,
                ..
            } => {
                let head_len = (clean_stroke(*width) * 4.0).clamp(8.0, 48.0);
                draw_line(&mut img, *from, *to, *color, *width);
                fill_triangle(&mut img, arrow_head(*from, *to, head_len), *color);
            }
            AnnotObject::Shape {
                shape,
                rect,
                color,
                width,
                ..
            } => match shape {
                ShapeKind::Rect => draw_rect_outline(&mut img, *rect, *color, *width),
                ShapeKind::Ellipse => draw_ellipse_outline(&mut img, *rect, *color, *width),
                ShapeKind::Line => draw_line(
                    &mut img,
                    Point::new(rect.x, rect.y),
                    Point::new(rect.x + rect.w, rect.y + rect.h),
                    *color,
                    *width,
                ),
            },
            AnnotObject::Text {
                pos,
                text,
                color,
                size,
                ..
            } => {
                if let Some(font) = font {
                    draw_text(&mut img, font, *pos, text, *color, *size);
                }
            }
            AnnotObject::Blur {
                rect, mode, radius, ..
            } => {
                // Blurs the working image, i.e. everything painted so far.
                img = apply_blur(&img, *rect, *mode, *radius);
            }
            AnnotObject::Step {
                pos,
                number,
                color,
                radius,
                ..
            } => {
                let r = clean_radius(*radius);
                fill_circle(&mut img, *pos, r, *color);
                if let Some(font) = font {
                    draw_text_centered(
                        &mut img,
                        font,
                        *pos,
                        &number.to_string(),
                        Rgba::WHITE,
                        (r * 1.1).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE),
                    );
                }
            }
            AnnotObject::Highlighter { rect, color, .. } => {
                fill_rect(&mut img, *rect, *color);
            }
            AnnotObject::Spotlight { rect, dim, .. } => {
                dim_outside(&mut img, *rect, *dim);
            }
            AnnotObject::Callout {
                rect,
                text,
                color,
                size,
                ..
            } => {
                draw_rect_outline(&mut img, *rect, *color, CALLOUT_STROKE);
                if let Some(font) = font {
                    draw_text_centered(&mut img, font, rect.center(), text, *color, *size);
                }
            }
        }
    }
    img
}

// ---------------------------------------------------------------- primitives

fn clean_stroke(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(1.0, MAX_STROKE)
    } else {
        1.0
    }
}

fn clean_radius(radius: f32) -> f32 {
    if radius.is_finite() {
        radius.clamp(0.0, MAX_RADIUS)
    } else {
        MIN_TEXT_SIZE
    }
}

/// Over a pixel with `coverage` (0..=1) multiplied by the colour's own alpha.
/// Silently clips: out-of-bounds coordinates are dropped.
fn blend_px(img: &mut Surface, x: i64, y: i64, color: Rgba, coverage: f32) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    if x < 0 || y < 0 || x >= w || y >= h || !coverage.is_finite() {
        return;
    }
    let cov = coverage.clamp(0.0, 1.0) * (color.a as f32 / 255.0);
    if cov <= 0.0 {
        return;
    }
    let dst = img.get_pixel(x as u32, y as u32).0;
    let mix = |s: u8, d: u8| -> u8 {
        (s as f32 * cov + d as f32 * (1.0 - cov))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    let alpha = (cov * 255.0 + dst[3] as f32 * (1.0 - cov))
        .round()
        .clamp(0.0, 255.0) as u8;
    let out = [
        mix(color.r, dst[0]),
        mix(color.g, dst[1]),
        mix(color.b, dst[2]),
        alpha,
    ];
    img.get_pixel_mut(x as u32, y as u32).0 = out;
}

/// Liang–Barsky clip of `a..b` against the image rect grown by `pad`.
/// Returns None when the segment misses the image entirely.
fn clip_segment(a: Point, b: Point, pad: f32, w: f32, h: f32) -> Option<(Point, Point)> {
    if !a.x.is_finite() || !a.y.is_finite() || !b.x.is_finite() || !b.y.is_finite() {
        return None;
    }
    let (xmin, ymin) = (-pad, -pad);
    let (xmax, ymax) = (w + pad, h + pad);
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    for (p, q) in [
        (-dx, a.x - xmin),
        (dx, xmax - a.x),
        (-dy, a.y - ymin),
        (dy, ymax - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            if t > t1 {
                return None;
            }
            if t > t0 {
                t0 = t;
            }
        } else {
            if t < t0 {
                return None;
            }
            if t < t1 {
                t1 = t;
            }
        }
    }
    if t0 > t1 {
        return None;
    }
    Some((
        Point::new(a.x + dx * t0, a.y + dy * t0),
        Point::new(a.x + dx * t1, a.y + dy * t1),
    ))
}

/// Anti-aliased thick line: walks the (clipped) segment and stamps a radial
/// brush of `width / 2`. Step spacing follows the brush size (never above
/// half a pixel, never more than half the brush) so wide strokes stay cheap.
fn draw_line(img: &mut Surface, from: Point, to: Point, color: Rgba, width: f32) {
    let width = clean_stroke(width);
    let half = width / 2.0;
    let pad = half.ceil() + 1.0;
    let (w, h) = (img.width() as f32, img.height() as f32);
    let Some((a, b)) = clip_segment(from, to, pad, w, h) else {
        return;
    };
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    // Spacing: 0.5px for thin strokes, scales with the brush for thick ones
    // (the brush overlaps itself, so no gaps appear).
    let spacing = (half * 0.5).max(0.5);
    let steps = ((len / spacing).ceil() as i64).clamp(1, MAX_LINE_STEPS);
    let r = half.ceil() as i64;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let cx = a.x + dx * t;
        let cy = a.y + dy * t;
        if !cx.is_finite() || !cy.is_finite() {
            return;
        }
        let (cx, cy) = (cx as i64, cy as i64);
        for oy in -r..=r {
            for ox in -r..=r {
                let d = ((ox * ox + oy * oy) as f32).sqrt();
                let cov = (half + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    blend_px(img, cx + ox, cy + oy, color, cov);
                }
            }
        }
    }
}

/// Corner points of the arrowhead: tip at `to`, base `head_len` behind it.
pub fn arrow_head(from: Point, to: Point, head_len: f32) -> [Point; 3] {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 || !len.is_finite() || !head_len.is_finite() {
        return [to, to, to];
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let half = head_len * 0.5;
    let (bx, by) = (to.x - ux * head_len, to.y - uy * head_len);
    [
        to,
        Point::new(bx + px * half, by + py * half),
        Point::new(bx - px * half, by - py * half),
    ]
}

/// Scanline-free filled triangle via barycentric coverage on pixel centres.
fn fill_triangle(img: &mut Surface, tri: [Point; 3], color: Rgba) {
    let [p0, p1, p2] = tri;
    if !p0.x.is_finite()
        || !p0.y.is_finite()
        || !p1.x.is_finite()
        || !p1.y.is_finite()
        || !p2.x.is_finite()
        || !p2.y.is_finite()
    {
        return;
    }
    let area = |a: Point, b: Point, c: Point| -> f32 {
        (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
    };
    let total = area(p0, p1, p2);
    if total.abs() < 1e-6 {
        return;
    }
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let xs = [p0.x, p1.x, p2.x];
    let ys = [p0.y, p1.y, p2.y];
    let min_x = xs.iter().copied().fold(f32::MAX, f32::min).floor() as i64;
    let max_x = xs.iter().copied().fold(f32::MIN, f32::max).ceil() as i64;
    let min_y = ys.iter().copied().fold(f32::MAX, f32::min).floor() as i64;
    let max_y = ys.iter().copied().fold(f32::MIN, f32::max).ceil() as i64;
    let min_x = min_x.max(-1);
    let min_y = min_y.max(-1);
    let max_x = max_x.min(iw + 1);
    let max_y = max_y.min(ih + 1);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = Point::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = area(p1, p2, p) / total;
            let w1 = area(p2, p0, p) / total;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                blend_px(img, x, y, color, 1.0);
            }
        }
    }
}

fn draw_rect_outline(img: &mut Surface, rect: Rect, color: Rgba, width: f32) {
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    let pts = [
        Point::new(x, y),
        Point::new(x + w, y),
        Point::new(x + w, y + h),
        Point::new(x, y + h),
    ];
    for i in 0..4 {
        let (a, b) = match (pts.get(i), pts.get((i + 1) % 4)) {
            (Some(a), Some(b)) => (*a, *b),
            _ => return,
        };
        draw_line(img, a, b, color, width);
    }
}

/// 64-segment parametric polyline approximating the ellipse inscribed in `rect`.
fn draw_ellipse_outline(img: &mut Surface, rect: Rect, color: Rgba, width: f32) {
    let center = rect.center();
    let rx = rect.w / 2.0;
    let ry = rect.h / 2.0;
    if !center.x.is_finite() || !center.y.is_finite() || !rx.is_finite() || !ry.is_finite() {
        return;
    }
    if rx.abs() < 0.5 && ry.abs() < 0.5 {
        // Degenerate: a dot is still better than nothing.
        fill_rect(
            img,
            Rect::new(center.x - 0.5, center.y - 0.5, 1.0, 1.0),
            color,
        );
        return;
    }
    let seg = ELLIPSE_SEGMENTS.max(4) as f32;
    let mut prev = Point::new(center.x + rx, center.y);
    for i in 1..=ELLIPSE_SEGMENTS.max(4) {
        let t = i as f32 / seg * std::f32::consts::TAU;
        let p = Point::new(center.x + rx * t.cos(), center.y + ry * t.sin());
        draw_line(img, prev, p, color, width);
        prev = p;
    }
}

/// Solid axis-aligned fill with straight-alpha source-over.
fn fill_rect(img: &mut Surface, rect: Rect, color: Rgba) {
    let Some((x0, y0, x1, y1)) = rect.clamp_to(img.width(), img.height()) else {
        return;
    };
    for y in y0..y1 {
        for x in x0..x1 {
            let dst = Rgba::from_array(img.get_pixel(x, y).0);
            img.get_pixel_mut(x, y).0 = color.src_over(dst).to_array();
        }
    }
}

fn fill_circle(img: &mut Surface, center: Point, radius: f32, color: Rgba) {
    if !center.x.is_finite() || !center.y.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return;
    }
    let r = radius.clamp(0.0, MAX_RADIUS);
    let (w, h) = (img.width() as f32, img.height() as f32);
    let x0 = ((center.x - r).floor()).clamp(0.0, w) as i64;
    let y0 = ((center.y - r).floor()).clamp(0.0, h) as i64;
    let x1 = ((center.x + r).ceil()).clamp(0.0, w) as i64;
    let y1 = ((center.y + r).ceil()).clamp(0.0, h) as i64;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - center.x;
            let dy = y as f32 + 0.5 - center.y;
            let d = (dx * dx + dy * dy).sqrt();
            let cov = (r + 0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(img, x, y, color, cov);
            }
        }
    }
}

/// Multiplies RGB (alpha kept) of every pixel OUTSIDE the clamped `rect` by
/// `1 - dim`, so `dim = 0` is a no-op and `dim = 1` is black.
fn dim_outside(img: &mut Surface, rect: Rect, dim: f32) {
    let dim = if dim.is_finite() {
        dim.clamp(0.0, 1.0)
    } else {
        return;
    };
    if dim <= 0.0 {
        return;
    }
    let (w, h) = (img.width(), img.height());
    let Some((x0, y0, x1, y1)) = rect.clamp_to(w, h) else {
        return;
    };
    let factor = 1.0 - dim;
    for y in 0..h {
        for x in 0..w {
            if x >= x0 && x < x1 && y >= y0 && y < y1 {
                continue;
            }
            let p = img.get_pixel_mut(x, y);
            for v in p.0.iter_mut().take(3) {
                *v = (*v as f32 * factor).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

// --------------------------------------------------------------------- text

fn clean_text_size(size: f32) -> Option<f32> {
    if size.is_finite() {
        Some(size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE))
    } else {
        None
    }
}

fn text_width<F: Font>(font: &F, text: &str, scale: PxScale) -> Option<f32> {
    let mut x = 0.0f32;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let adv = font.as_scaled(scale).h_advance(gid);
        if !adv.is_finite() {
            return None;
        }
        x += adv;
        if !x.is_finite() || x > 1.0e6 {
            return None;
        }
    }
    Some(x)
}

/// Draw `text` with its top-left at (`left`, `baseline - ascent`).
fn draw_text_at<F: Font>(
    img: &mut Surface,
    font: &F,
    left: f32,
    baseline: f32,
    text: &str,
    color: Rgba,
    size: f32,
) {
    let Some(size) = clean_text_size(size) else {
        return;
    };
    if !left.is_finite() || !baseline.is_finite() {
        return;
    }
    let scale = PxScale::from(size);
    let right_limit = img.width() as f32 + size;
    let mut x = left;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(x, baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|px, py, cov| {
                blend_px(
                    img,
                    bounds.min.x as i64 + i64::from(px),
                    bounds.min.y as i64 + i64::from(py),
                    color,
                    cov,
                );
            });
        }
        let adv = font.as_scaled(scale).h_advance(gid);
        if !adv.is_finite() {
            return;
        }
        x += adv;
        if x > right_limit {
            return; // hostile-input bound: stop painting off-canvas text
        }
    }
}

/// `Text` objects: `pos` is LEFT_TOP, baseline sits one ascent below it.
fn draw_text<F: Font>(img: &mut Surface, font: &F, pos: Point, text: &str, color: Rgba, size: f32) {
    let Some(size) = clean_text_size(size) else {
        return;
    };
    if !pos.x.is_finite() || !pos.y.is_finite() {
        return;
    }
    let scale = PxScale::from(size);
    let baseline = pos.y + font.as_scaled(scale).ascent();
    draw_text_at(img, font, pos.x, baseline, text, color, size);
}

/// Text centred (optically) on `center`: horizontal advance midpoint on `x`,
/// baseline half an ascent below `y`.
fn draw_text_centered<F: Font>(
    img: &mut Surface,
    font: &F,
    center: Point,
    text: &str,
    color: Rgba,
    size: f32,
) {
    let Some(size) = clean_text_size(size) else {
        return;
    };
    if !center.x.is_finite() || !center.y.is_finite() {
        return;
    }
    let scale = PxScale::from(size);
    let Some(tw) = text_width(font, text, scale) else {
        return;
    };
    let left = center.x - tw / 2.0;
    let baseline = center.y + font.as_scaled(scale).ascent() * 0.5;
    draw_text_at(img, font, left, baseline, text, color, size);
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagcraft_doc::BlurMode;
    use snagcraft_geom::Size;

    const RED: Rgba = Rgba::new(255, 0, 0, 255);
    const BLUE: Rgba = Rgba::new(0, 0, 255, 255);

    fn white_doc() -> Document {
        Document::white(Size::new(64, 64))
    }

    fn px(img: &Surface, x: u32, y: u32) -> [u8; 4] {
        img.get_pixel(x, y).0
    }

    fn is_reddish(p: [u8; 4]) -> bool {
        p[0] > 180 && p[1] < 90 && p[2] < 90
    }

    fn is_blueish(p: [u8; 4]) -> bool {
        p[2] > 180 && p[0] < 90 && p[1] < 90
    }

    fn variance(img: &Surface, r: Rect) -> f64 {
        let Some((x0, y0, x1, y1)) = r.clamp_to(img.width(), img.height()) else {
            return 0.0;
        };
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

    fn checkerboard(size: Size) -> Surface {
        let mut img = snagcraft_raster::filled(size, Rgba::new(100, 100, 100, 255));
        for y in 0..img.height() {
            for x in 0..img.width() {
                if ((x / 3) + (y / 3)) % 2 == 0 {
                    img.get_pixel_mut(x, y).0 = [160, 160, 160, 255];
                }
            }
        }
        img
    }

    #[test]
    fn empty_document_is_a_base_clone() {
        let doc = white_doc();
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out, *doc.base);
    }

    #[test]
    fn arrow_draws_shaft_and_head() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Arrow {
            id: 0,
            from: Point::new(10.0, 40.0),
            to: Point::new(50.0, 40.0),
            color: BLUE,
            width: 3.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(is_blueish(px(&out, 30, 40)), "shaft {:?}", px(&out, 30, 40));
        // Head: the tip is at x=50, so the last covered pixel centre is 49.
        assert!(is_blueish(px(&out, 49, 40)), "tip {:?}", px(&out, 49, 40));
        assert!(
            is_blueish(px(&out, 44, 40)),
            "head body {:?}",
            px(&out, 44, 40)
        );
        assert_eq!(px(&out, 0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn shape_rect_is_an_outline() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(10.0, 10.0, 30.0, 30.0),
            color: RED,
            width: 2.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(
            is_reddish(px(&out, 25, 10)),
            "top edge {:?}",
            px(&out, 25, 10)
        );
        assert!(
            is_reddish(px(&out, 10, 25)),
            "left edge {:?}",
            px(&out, 10, 25)
        );
        assert_eq!(
            px(&out, 25, 25),
            [255, 255, 255, 255],
            "interior must stay clear"
        );
    }

    #[test]
    fn shape_ellipse_outline_hits_the_axes() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Ellipse,
            rect: Rect::new(10.0, 10.0, 40.0, 40.0),
            color: RED,
            width: 2.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(is_reddish(px(&out, 10, 30)), "left {:?}", px(&out, 10, 30));
        assert!(is_reddish(px(&out, 30, 10)), "top {:?}", px(&out, 30, 10));
        assert_eq!(
            px(&out, 30, 30),
            [255, 255, 255, 255],
            "centre must stay clear"
        );
    }

    #[test]
    fn shape_line_is_the_rect_diagonal() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Line,
            rect: Rect::new(10.0, 10.0, 30.0, 30.0),
            color: RED,
            width: 2.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(
            is_reddish(px(&out, 25, 25)),
            "diagonal {:?}",
            px(&out, 25, 25)
        );
        // Far off the diagonal stays clean.
        assert_eq!(px(&out, 40, 10), [255, 255, 255, 255]);
    }

    #[test]
    fn step_draws_a_filled_circle() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(32.0, 32.0),
            number: 1,
            color: RED,
            radius: 10.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(
            is_reddish(px(&out, 32, 32)),
            "centre {:?}",
            px(&out, 32, 32)
        );
        assert!(px(&out, 32, 23)[0] > 100, "rim is painted");
        assert_eq!(px(&out, 0, 0), [255, 255, 255, 255], "outside untouched");
    }

    #[test]
    fn step_radius_is_never_smaller_than_four() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(20.0, 20.0),
            number: 7,
            color: RED,
            radius: 0.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // radius.max(4) means (23,20) is still inside the disc.
        assert!(px(&out, 23, 20)[0] > 100, "{:?}", px(&out, 23, 20));
    }

    #[test]
    fn highlighter_blends_semi_transparent_fill() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Highlighter {
            id: 0,
            rect: Rect::new(10.0, 10.0, 40.0, 20.0),
            color: Rgba::new(255, 255, 0, 90),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        let p = px(&out, 20, 15);
        assert_eq!(p[0], 255, "red stays saturated: {p:?}");
        assert!(
            (130..=210).contains(&p[2]),
            "blue is knocked down by the alpha: {p:?}"
        );
        assert_eq!(p[3], 255);
        assert_eq!(px(&out, 5, 5), [255, 255, 255, 255], "outside untouched");
    }

    #[test]
    fn spotlight_dims_outside_and_keeps_inside() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(16.0, 16.0, 32.0, 32.0),
            dim: 0.5,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        let outside = px(&out, 2, 2);
        assert!(
            (120..=135).contains(&outside[0]),
            "outside dimmed by half: {outside:?}"
        );
        assert_eq!(px(&out, 30, 30), [255, 255, 255, 255], "inside untouched");
    }

    #[test]
    fn spotlight_dim_zero_and_nan_are_noops() {
        for dim in [0.0, f32::NAN, -1.0] {
            let mut doc = white_doc();
            doc.add(AnnotObject::Spotlight {
                id: 0,
                rect: Rect::new(16.0, 16.0, 32.0, 32.0),
                dim,
            });
            let out = composite::<ab_glyph::FontVec>(&doc, None);
            assert_eq!(out, *doc.base, "dim {dim} must not change pixels");
        }
    }

    #[test]
    fn callout_draws_outline_and_skips_text_without_font() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Callout {
            id: 0,
            rect: Rect::new(10.0, 10.0, 40.0, 30.0),
            text: "note".into(),
            color: RED,
            size: 14.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(
            is_reddish(px(&out, 10, 25)),
            "left edge {:?}",
            px(&out, 10, 25)
        );
        assert!(
            is_reddish(px(&out, 30, 10)),
            "top edge {:?}",
            px(&out, 30, 10)
        );
        assert_eq!(px(&out, 30, 25), [255, 255, 255, 255], "no font, no glyphs");
    }

    #[test]
    fn no_font_text_paths_are_safe() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(4.0, 4.0),
            text: "hello there".into(),
            color: RED,
            size: 18.0,
            bold: true,
            italic: true,
        });
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(4.0, 30.0),
            text: String::new(),
            color: RED,
            size: f32::NAN,
            bold: false,
            italic: false,
        });
        doc.add(AnnotObject::Callout {
            id: 0,
            rect: Rect::new(2.0, 40.0, 50.0, 20.0),
            text: "x".repeat(10_000),
            color: RED,
            size: 9999.0,
        });
        doc.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(50.0, 12.0),
            number: 42,
            color: RED,
            radius: 8.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // Only the layout-independent geometry painted; nothing panicked.
        assert!(px(&out, 50, 12)[0] > 100);
    }

    #[test]
    fn blur_object_lowers_region_variance() {
        let base = checkerboard(Size::new(64, 64));
        let mut doc = Document::new(base.clone());
        doc.add(AnnotObject::Blur {
            id: 0,
            rect: Rect::new(8.0, 8.0, 32.0, 32.0),
            mode: BlurMode::Gaussian,
            radius: 4,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        let region = Rect::new(8.0, 8.0, 32.0, 32.0);
        let before = variance(&base, region);
        let after = variance(&out, region);
        assert!(before > 100.0, "test base must carry detail: {before}");
        assert!(
            after < before * 0.2,
            "blur must smooth: {before} -> {after}"
        );
        assert_eq!(px(&out, 0, 0), px(&base, 0, 0), "outside untouched");
    }

    #[test]
    fn thick_stroke_has_no_gaps() {
        // Wide strokes advance the brush in bigger steps; verify coverage is
        // still continuous along the whole stroke.
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Line,
            rect: Rect::new(4.0, 12.0, 56.0, 40.0),
            color: RED,
            width: 24.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        for i in 0..40 {
            let t = i as f32 / 39.0;
            let x = (4.0 + 56.0 * t).round() as u32;
            let y = (12.0 + 40.0 * t).round() as u32;
            let p = px(&out, x, y);
            assert!(p[0] > 200 && p[1] < 60, "gap at ({x},{y}): {p:?}");
        }
    }

    #[test]
    fn objects_apply_in_vector_order() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Blur {
            id: 0,
            rect: Rect::new(0.0, 0.0, 64.0, 64.0),
            mode: BlurMode::Blackout,
            radius: 0,
        });
        doc.add(AnnotObject::Arrow {
            id: 0,
            from: Point::new(10.0, 50.0),
            to: Point::new(50.0, 50.0),
            color: RED,
            width: 3.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(px(&out, 5, 5), [0, 0, 0, 255], "blackout painted first");
        assert!(
            is_reddish(px(&out, 30, 50)),
            "arrow painted over the blackout"
        );
    }

    #[test]
    fn hostile_objects_never_panic() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Arrow {
            id: 0,
            from: Point::new(f32::NAN, f32::NAN),
            to: Point::new(f32::INFINITY, 10.0),
            color: RED,
            width: f32::NAN,
        });
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Ellipse,
            rect: Rect::new(f32::NAN, 0.0, 1e30, 1e30),
            color: RED,
            width: f32::INFINITY,
        });
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(-1e30, -1e30, 2e30, 2e30),
            color: RED,
            width: 2.0,
        });
        doc.add(AnnotObject::Highlighter {
            id: 0,
            rect: Rect::new(f32::INFINITY, f32::INFINITY, 5.0, 5.0),
            color: RED,
        });
        doc.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(f32::NAN, 3.0),
            number: 1,
            color: RED,
            radius: f32::INFINITY,
        });
        doc.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(f32::NAN, f32::NAN, f32::NAN, f32::NAN),
            dim: f32::INFINITY,
        });
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(f32::NAN, f32::NAN),
            text: "nope".into(),
            color: RED,
            size: f32::INFINITY,
            bold: false,
            italic: false,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out.dimensions(), (64, 64));
    }

    #[test]
    fn empty_base_composites_without_panicking() {
        let mut doc = Document::new(Surface::new(0, 0));
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: RED,
            width: 2.0,
        });
        doc.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            dim: 0.4,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out.dimensions(), (0, 0));
    }
}
