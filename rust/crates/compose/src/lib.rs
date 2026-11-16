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
//! input-derived number (rects, sizes, widths, radii, dim, zoom, brush points,
//! shadow offsets) is clamped before use.
//!
//! Parity notes (live `src/editor/blur.ts`, `src/editor/tools.ts`):
//! * spotlight paints `rgba(8, 12, 18, 0.68)` **outside** the hole (`blur.ts:30,283`)
//! * highlighter **multiplies** its ink at alpha `0.36` (`blur.ts:28,311`)
//! * arrow heads scale off the stroke width (`tools.ts:1014-1018`)
//! * the star silhouette is `snagcraft_doc::star_outline` (live `STAR_PATH`)
//!
//! Fonts are looked up per object through the [`composite_styled`] callback;
//! [`composite`] is the single-font wrapper. Without a font every text path is
//! skipped (geometry still paints).

use ab_glyph::{Font, PxScale, ScaleFont};
use snagcraft_algo::{apply_blur, resize_region};
use snagcraft_color::Rgba;
use snagcraft_doc::{AnnotObject, Document, Shadow, ShapeKind, star_outline};
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

/// Spotlight dim ink — `rgba(8, 12, 18, 0.68)` in the live app (`blur.ts:29-30`).
const SPOTLIGHT_INK: [u8; 3] = [8, 12, 18];
const SPOTLIGHT_ALPHA: f32 = 0.68;
/// Highlighter tint opacity (`blur.ts:28`).
const HIGHLIGHTER_ALPHA: f32 = 0.36;

/// Arrow head: `thickness = strokeWidth * 2`, then `max(16, thickness * 1.7)`
/// long and `max(18, thickness * 2)` wide (`tools.ts:1014-1018`).
const ARROW_THICKNESS: f32 = 2.0;
const ARROW_HEAD_LEN_MIN: f32 = 16.0;
const ARROW_HEAD_LEN_RATIO: f32 = 1.7;
const ARROW_HEAD_HALF_MIN: f32 = 9.0;

/// Magnify lens (`tools.ts:193-199`).
const MAGNIFY_ZOOM_DEFAULT: f32 = 3.0;
const MAGNIFY_ZOOM_MIN: f32 = 1.5;
const MAGNIFY_ZOOM_MAX: f32 = 10.0;
/// Corner radius as a fraction of the shortest side (`tools.ts:290`).
const LENS_CORNER: f32 = 0.12;
/// Lens border: 2 px `#1c1c1e`.
const LENS_BORDER: f32 = 2.0;
const LENS_INK: Rgba = Rgba::new(0x1c, 0x1c, 0x1e, 255);
/// Zoom chip: `rgba(12, 16, 24, 0.82)` (`tools.ts:369`).
const LENS_CHIP: Rgba = Rgba::new(12, 16, 24, 209);

/// Brush nib width range (`ToolsUI` "Brush size", 2–44 px).
const BRUSH_WIDTH_MIN: f32 = 2.0;
const BRUSH_WIDTH_MAX: f32 = 44.0;
/// Hostile-input cap on stroke points (mirrors `snagcraft_doc::MAX_BRUSH_POINTS`).
const MAX_BRUSH_POINTS: usize = 100_000;

/// Hostile-input cap on a shadow mask.
const MAX_MASK_DIM: u32 = 4096;
const MAX_MASK_PIXELS: u64 = 4096 * 4096;

/// Composite `doc` into a fresh surface with one font for every text object.
/// Base clone, then every object in vector order (later objects paint over
/// earlier ones). Never mutates `doc`.
pub fn composite<F: Font>(doc: &Document, font: Option<&F>) -> Surface {
    let lookup = |_: &str, _: bool| font;
    composite_styled(doc, &lookup, 0.0)
}

/// Composite with a per-object font lookup and synthetic italic shear.
///
/// `fonts(family, bold)` returns the font to use for one text object: `family`
/// is the object's own `family` (`Text`/`Callout`, live default `"Inter"`) and
/// `bold` is `Text`'s bold flag (`Callout` is never bold, live parity).
/// `italic_shear` is the horizontal run per pixel of height applied to `Text`
/// objects with `italic: true` (`0.0` = upright, live canvas uses ≈`0.2`).
pub fn composite_styled<'f, 'a, F: Font>(
    doc: &Document,
    fonts: &'f dyn Fn(&str, bool) -> Option<&'a F>,
    italic_shear: f32,
) -> Surface {
    let ctx = Ctx {
        fonts,
        shear: clean_shear(italic_shear),
        base: doc.base.as_ref(),
    };
    let mut img: Surface = doc.base.as_ref().clone();

    for obj in &doc.objects {
        if let Some(shadow) = shadow_of(obj)
            && shadow.enabled
        {
            draw_shadow(&mut img, obj, shadow, &ctx);
        }
        draw_object(&mut img, obj, &ctx);
    }
    img
}

/// Everything a draw pass needs that is not the object itself.
struct Ctx<'f, 'a, F> {
    fonts: &'f dyn Fn(&str, bool) -> Option<&'a F>,
    shear: f32,
    base: &'f Surface,
}

/// The drop shadow of `obj`, when the kind carries one.
fn shadow_of(obj: &AnnotObject) -> Option<Shadow> {
    match obj {
        AnnotObject::Arrow { shadow, .. }
        | AnnotObject::Shape { shadow, .. }
        | AnnotObject::Text { shadow, .. }
        | AnnotObject::Step { shadow, .. }
        | AnnotObject::Callout { shadow, .. } => Some(*shadow),
        AnnotObject::Blur { .. }
        | AnnotObject::Highlighter { .. }
        | AnnotObject::Spotlight { .. }
        | AnnotObject::Magnify { .. }
        | AnnotObject::BrushStroke { .. } => None,
    }
}

// ------------------------------------------------------------------- objects

/// Paint one object (its shadow, if any, is handled by the caller).
fn draw_object<F: Font>(img: &mut Surface, obj: &AnnotObject, ctx: &Ctx<'_, '_, F>) {
    match obj {
        AnnotObject::Arrow {
            from,
            to,
            color,
            width,
            ..
        } => {
            let stroke = clean_stroke(*width);
            let thickness = stroke * ARROW_THICKNESS;
            let head_len = (thickness * ARROW_HEAD_LEN_RATIO).max(ARROW_HEAD_LEN_MIN);
            let head_half = thickness.max(ARROW_HEAD_HALF_MIN);
            // Stop the shaft short of the head so the stroke cannot poke out
            // through the tip (live parity: `headLength * 0.9`).
            let neck = neck_point(*from, *to, head_len * 0.9);
            draw_line(img, *from, neck, *color, thickness);
            fill_triangle(
                img,
                arrow_head_sized(*from, *to, head_len, head_half),
                *color,
            );
        }
        AnnotObject::Shape {
            shape,
            rect,
            color,
            width,
            ..
        } => match shape {
            ShapeKind::Rect => draw_rect_outline(img, *rect, *color, *width),
            ShapeKind::Ellipse => draw_ellipse_outline(img, *rect, *color, *width),
            ShapeKind::Line => draw_line(
                img,
                Point::new(rect.x, rect.y),
                Point::new(rect.x + rect.w, rect.y + rect.h),
                *color,
                *width,
            ),
            ShapeKind::Triangle => {
                // Apex top-centre, base along the bottom edge (live
                // `fabric.Triangle`).
                let tri = [
                    Point::new(rect.x + rect.w / 2.0, rect.y),
                    Point::new(rect.x, rect.y + rect.h),
                    Point::new(rect.x + rect.w, rect.y + rect.h),
                ];
                stroke_polyline(img, &tri, *color, *width, true);
            }
            ShapeKind::Star => {
                let pts = star_outline(*rect);
                stroke_polyline(img, &pts, *color, *width, true);
            }
        },
        AnnotObject::Text {
            pos,
            text,
            color,
            size,
            bold,
            italic,
            family,
            ..
        } => {
            if let Some(font) = (ctx.fonts)(family.as_str(), *bold) {
                let shear = if *italic { ctx.shear } else { 0.0 };
                draw_text(img, font, *pos, text, *color, Run { size: *size, shear });
            }
        }
        // Blurs the working image, i.e. everything painted so far.
        AnnotObject::Blur {
            rect, mode, radius, ..
        } => {
            *img = apply_blur(img, *rect, *mode, *radius);
        }
        AnnotObject::Step {
            pos,
            number,
            color,
            radius,
            ..
        } => {
            let r = clean_radius(*radius);
            fill_circle(img, *pos, r, *color);
            if let Some(font) = (ctx.fonts)("", true) {
                draw_text_centered(
                    img,
                    font,
                    *pos,
                    &number.to_string(),
                    Rgba::WHITE,
                    Run {
                        size: (r * 1.1).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE),
                        shear: 0.0,
                    },
                );
            }
        }
        // Multiplied into the image pixels, exactly like the live canvas
        // highlighter (`blur.ts:311`).
        AnnotObject::Highlighter { rect, color, .. } => highlighter_tint(img, *rect, *color),
        AnnotObject::Spotlight { rect, dim, .. } => dim_outside(img, *rect, *dim),
        AnnotObject::Callout {
            rect,
            text,
            color,
            size,
            family,
            ..
        } => {
            draw_rect_outline(img, *rect, *color, CALLOUT_STROKE);
            if let Some(font) = (ctx.fonts)(family.as_str(), false) {
                draw_text_centered(
                    img,
                    font,
                    rect.center(),
                    text,
                    *color,
                    Run {
                        size: *size,
                        shear: 0.0,
                    },
                );
            }
        }
        AnnotObject::Magnify { rect, zoom, .. } => draw_magnify(img, *rect, *zoom, ctx),
        AnnotObject::BrushStroke {
            points,
            color,
            width,
            ..
        } => draw_brush_stroke(img, points, *color, *width),
    }
}

// ------------------------------------------------------------------- shadows

/// Drop shadow: render the object's silhouette (fills *and* outlines, i.e.
/// canvas `affectStroke: true`) into an alpha mask, box-blur the mask, tint it
/// with the shadow colour and composite it behind the object at
/// (`offset_x`, `offset_y`).
///
/// The mask covers the object bounds grown by the blur halo, clipped to the
/// image and capped at [`MAX_MASK_DIM`]² — so a hostile object costs a bounded
/// allocation.
fn draw_shadow<F: Font>(
    img: &mut Surface,
    obj: &AnnotObject,
    shadow: Shadow,
    ctx: &Ctx<'_, '_, F>,
) {
    let shadow = shadow.clamped();
    if !shadow.enabled {
        return;
    }
    let (iw, ih) = img.dimensions();
    if iw == 0 || ih == 0 {
        return;
    }
    let b = obj.bounds();
    if !b.is_finite() || b.w <= 0.0 || b.h <= 0.0 {
        return;
    }
    let halo = f32::from(u16::try_from(shadow.blur).unwrap_or(u16::MAX)).ceil() + 2.0;
    // Destination rect in image space: silhouette bounds moved by the offset,
    // grown by the blur halo.
    let dx0 = (b.x + shadow.offset_x - halo).floor();
    let dy0 = (b.y + shadow.offset_y - halo).floor();
    let dx1 = (b.x + b.w + shadow.offset_x + halo).ceil();
    let dy1 = (b.y + b.h + shadow.offset_y + halo).ceil();
    if !dx0.is_finite() || !dy0.is_finite() || !dx1.is_finite() || !dy1.is_finite() {
        return;
    }
    let x0 = dx0.clamp(0.0, iw as f32) as u32;
    let y0 = dy0.clamp(0.0, ih as f32) as u32;
    let x1 = dx1.clamp(0.0, iw as f32) as u32;
    let y1 = dy1.clamp(0.0, ih as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (mw, mh) = (x1 - x0, y1 - y0);
    if mw > MAX_MASK_DIM || mh > MAX_MASK_DIM {
        return;
    }
    if u64::from(mw) * u64::from(mh) > MAX_MASK_PIXELS {
        return;
    }

    // Object space -> mask space: the silhouette is drawn at its own place, and
    // the shadow offset is applied when compositing.
    let ox = x0 as f32 - shadow.offset_x;
    let oy = y0 as f32 - shadow.offset_y;
    let shifted = obj.shifted(-ox, -oy);
    let mut mask = Surface::new(mw, mh);
    draw_object(&mut mask, &shifted, ctx);

    let mut alpha: Vec<u8> = mask.pixels().map(|p| p.0[3]).collect();
    let radius = usize::try_from(shadow.blur).unwrap_or(usize::MAX).min(60);
    // Three box passes ≈ gaussian (live canvas shadowBlur is gaussian-ish);
    // one pass reads as a hard halo. Same total spread, softer falloff.
    if radius > 0 {
        let pass = (radius / 3).max(1);
        for _ in 0..3 {
            box_blur_alpha(&mut alpha, mw, mh, pass);
        }
    }

    let tint = shadow.color;
    for y in 0..mh {
        for x in 0..mw {
            let Some(&a) = alpha.get((y as usize) * (mw as usize) + (x as usize)) else {
                continue;
            };
            if a == 0 {
                continue;
            }
            let scaled = (u32::from(a) * u32::from(tint.a) / 255) as u8;
            if scaled == 0 {
                continue;
            }
            let src = Rgba::new(tint.r, tint.g, tint.b, scaled);
            let dst = Rgba::from_array(img.get_pixel(x0 + x, y0 + y).0);
            img.get_pixel_mut(x0 + x, y0 + y).0 = src.src_over(dst).to_array();
        }
    }
}

/// One box-blur pass over an 8-bit alpha buffer, separable and O(n) via prefix
/// sums. `radius = 0` is a no-op.
fn box_blur_alpha(buf: &mut [u8], w: u32, h: u32, radius: usize) {
    let (w, h) = (w as usize, h as usize);
    if radius == 0 || w == 0 || h == 0 || buf.len() < w * h {
        return;
    }
    let mut tmp = vec![0u8; w * h];
    let mut prefix = vec![0u32; w.max(h) + 1];
    // Horizontal, src -> tmp.
    for y in 0..h {
        let row = y * w;
        prefix[0] = 0;
        for x in 0..w {
            prefix[x + 1] = prefix[x] + u32::from(buf[row + x]);
        }
        for x in 0..w {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(w - 1);
            let n = (hi - lo + 1) as u32;
            tmp[row + x] = ((prefix[hi + 1] - prefix[lo] + n / 2) / n) as u8;
        }
    }
    // Vertical, tmp -> buf.
    for x in 0..w {
        prefix[0] = 0;
        for y in 0..h {
            prefix[y + 1] = prefix[y] + u32::from(tmp[y * w + x]);
        }
        for y in 0..h {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius).min(h - 1);
            let n = (hi - lo + 1) as u32;
            buf[y * w + x] = ((prefix[hi + 1] - prefix[lo] + n / 2) / n) as u8;
        }
    }
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

fn clean_brush_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(BRUSH_WIDTH_MIN, BRUSH_WIDTH_MAX)
    } else {
        BRUSH_WIDTH_MIN
    }
}

fn clean_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(MAGNIFY_ZOOM_MIN, MAGNIFY_ZOOM_MAX)
    } else {
        MAGNIFY_ZOOM_DEFAULT
    }
}

fn clean_shear(shear: f32) -> f32 {
    if shear.is_finite() {
        shear.clamp(-1.0, 1.0)
    } else {
        0.0
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
/// brush of `width / 2`. This is the round-cap/round-join primitive the brush
/// tool is built from, so its dab spacing (`width / 4`, floor 0.5 px) is the
/// no-gap guarantee.
fn draw_line(img: &mut Surface, from: Point, to: Point, color: Rgba, width: f32) {
    let width = clean_stroke(width);
    let half = width / 2.0;
    let pad = half.ceil() + 1.0;
    let (w, h) = (img.width() as f32, img.height() as f32);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
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
    let mut budget = MAX_LINE_STEPS * 4;
    for i in 0..=steps {
        if budget <= 0 {
            return;
        }
        budget -= 1;
        let t = i as f32 / steps as f32;
        let cx = a.x + dx * t;
        let cy = a.y + dy * t;
        if !cx.is_finite() || !cy.is_finite() {
            return;
        }
        stamp_dab(img, cx, cy, half, color);
    }
}

/// One circular brush dab (the round cap / round join of a stroke).
fn stamp_dab(img: &mut Surface, cx: f32, cy: f32, half: f32, color: Rgba) {
    let r = half.ceil() as i64;
    let (ix, iy) = (cx.floor() as i64, cy.floor() as i64);
    for oy in -r..=r {
        for ox in -r..=r {
            let d = ((ox * ox + oy * oy) as f32).sqrt();
            let cov = (half + 0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(img, ix + ox, iy + oy, color, cov);
            }
        }
    }
}

/// Point `distance` back from `to` along the `from -> to` direction.
fn neck_point(from: Point, to: Point, distance: f32) -> Point {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 || !len.is_finite() || !distance.is_finite() {
        return to;
    }
    let d = distance.min(len);
    Point::new(to.x - dx / len * d, to.y - dy / len * d)
}

/// Corner points of an arrowhead: tip at `to`, base `head_len` behind it and
/// `half_width` to either side.
pub fn arrow_head_sized(from: Point, to: Point, head_len: f32, half_width: f32) -> [Point; 3] {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3
        || !len.is_finite()
        || !head_len.is_finite()
        || !half_width.is_finite()
        || head_len <= 0.0
        || half_width <= 0.0
    {
        return [to, to, to];
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let (bx, by) = (to.x - ux * head_len, to.y - uy * head_len);
    [
        to,
        Point::new(bx + px * half_width, by + py * half_width),
        Point::new(bx - px * half_width, by - py * half_width),
    ]
}

/// Symmetric arrowhead (half width = half the length). Kept for callers that
/// only know the head length; the arrow tool uses [`arrow_head_sized`].
pub fn arrow_head(from: Point, to: Point, head_len: f32) -> [Point; 3] {
    arrow_head_sized(from, to, head_len, head_len * 0.5)
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
    stroke_polyline(img, &pts, color, width, true);
}

/// Stroke a polyline (optionally closed) with round caps and joins.
fn stroke_polyline(img: &mut Surface, pts: &[Point], color: Rgba, width: f32, closed: bool) {
    if pts.len() < 2 {
        if let Some(p) = pts.first() {
            stamp_dab(img, p.x, p.y, clean_stroke(width) / 2.0, color);
        }
        return;
    }
    for pair in pts.windows(2) {
        draw_line(img, pair[0], pair[1], color, width);
    }
    if closed && let (Some(first), Some(last)) = (pts.first(), pts.last()) {
        draw_line(img, *last, *first, color, width);
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

/// Highlighter: multiply the ink into the pixels at [`HIGHLIGHTER_ALPHA`],
/// leaving alpha alone — `out_c = dst_c * (1 - a + a * ink_c / 255)` with
/// `a = 0.36` (`blur.ts:311`).
fn highlighter_tint(img: &mut Surface, rect: Rect, color: Rgba) {
    let Some((x0, y0, x1, y1)) = rect.clamp_to(img.width(), img.height()) else {
        return;
    };
    let a = HIGHLIGHTER_ALPHA * (f32::from(color.a) / 255.0);
    if a <= 0.0 {
        return;
    }
    let ink = [color.r, color.g, color.b].map(f32::from);
    // Live rounds the band corners: radius = min(8, min(w,h)/4); <=0.5 → hard
    // rect (blur.ts:325-333).
    let radius = 8.0f32.min(rect.w.min(rect.h) / 4.0);
    let rounded = radius > 0.5;
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let (hw, hh) = (rect.w / 2.0, rect.h / 2.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let cov = if rounded {
                (0.5 - round_rect_sdf(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw, hh, radius))
                    .clamp(0.0, 1.0)
            } else {
                1.0
            };
            if cov <= 0.0 {
                continue;
            }
            let a_eff = a * cov;
            let dst = img.get_pixel_mut(x, y);
            for c in 0..3 {
                let d = f32::from(dst.0[c]);
                dst.0[c] = (d * (1.0 - a_eff + a_eff * ink[c] / 255.0))
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Spotlight: source-over `rgba(8, 12, 18, 0.68 * dim)` OUTSIDE the clamped
/// `rect` (`blur.ts:283`). `dim = 0` is a no-op; a hole entirely off-canvas
/// dims the whole image.
fn dim_outside(img: &mut Surface, rect: Rect, dim: f32) {
    let dim = if dim.is_finite() {
        dim.clamp(0.0, 1.0)
    } else {
        return;
    };
    if dim <= 0.0 || !rect.is_finite() {
        return;
    }
    let alpha = (SPOTLIGHT_ALPHA * dim * 255.0).round().clamp(0.0, 255.0) as u8;
    if alpha == 0 {
        return;
    }
    let ink = Rgba::new(SPOTLIGHT_INK[0], SPOTLIGHT_INK[1], SPOTLIGHT_INK[2], alpha);
    let (w, h) = img.dimensions();
    let hole = rect.clamp_to(w, h);
    // Live rounds the hole corners: radius = min(12, min(w,h)/6) (blur.ts:297).
    let radius = 12.0f32.min(rect.w.min(rect.h) / 6.0).max(0.0);
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let (hw, hh) = (rect.w / 2.0, rect.h / 2.0);
    for y in 0..h {
        for x in 0..w {
            let mut cov = 0.0f32;
            if let Some((x0, y0, x1, y1)) = hole
                && x >= x0
                && x < x1
                && y >= y0
                && y < y1
            {
                cov = (0.5
                    - round_rect_sdf(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw, hh, radius))
                .clamp(0.0, 1.0);
            }
            let eff = (f32::from(alpha) * (1.0 - cov)).round() as u8;
            if eff == 0 {
                continue;
            }
            let ink_eff = Rgba::new(ink.r, ink.g, ink.b, eff);
            let dst = Rgba::from_array(img.get_pixel(x, y).0);
            img.get_pixel_mut(x, y).0 = ink_eff.src_over(dst).to_array();
        }
    }
}

/// Signed distance to a rounded rect (negative inside).
fn round_rect_sdf(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Coverage (0..=1) of a rounded rect at a pixel centre.
fn round_rect_coverage(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    (0.5 - round_rect_sdf(px, py, cx, cy, hw, hh, r)).clamp(0.0, 1.0)
}

/// Fill a rounded rect (chip / aperture backgrounds).
fn fill_round_rect(img: &mut Surface, rect: Rect, radius: f32, color: Rgba) {
    let Some((x0, y0, x1, y1)) = rect.clamp_to(img.width(), img.height()) else {
        return;
    };
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let r = radius.clamp(0.0, rect.w.min(rect.h) / 2.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let cov = round_rect_coverage(
                x as f32 + 0.5,
                y as f32 + 0.5,
                cx,
                cy,
                rect.w / 2.0,
                rect.h / 2.0,
                r,
            );
            if cov > 0.0 {
                blend_px(img, i64::from(x), i64::from(y), color, cov);
            }
        }
    }
}

/// Magnify lens (`tools.ts:264-380`): sample the *capture* around the lens
/// centre (a rect `1/zoom` of the lens size), scale it up to the lens rect and
/// clip it to a rounded aperture, then draw the 2 px border and zoom chip.
fn draw_magnify<F: Font>(img: &mut Surface, rect: Rect, zoom: f32, ctx: &Ctx<'_, '_, F>) {
    if !rect.is_finite() || rect.w < 1.0 || rect.h < 1.0 {
        return;
    }
    let zoom = clean_zoom(zoom);
    let ox = rect.x.floor();
    let oy = rect.y.floor();
    // A lens larger than the canvas can show nothing new: cap its working
    // surface to the image so a hostile rect cannot allocate 4096².
    let dw = (rect.w.round() as i64)
        .clamp(1, MAX_RADIUS as i64)
        .min(i64::from(img.width().max(1))) as u32;
    let dh = (rect.h.round() as i64)
        .clamp(1, MAX_RADIUS as i64)
        .min(i64::from(img.height().max(1))) as u32;
    let (cx, cy) = (ox + dw as f32 / 2.0, oy + dh as f32 / 2.0);
    let hw = dw as f32 / 2.0;
    let hh = dh as f32 / 2.0;
    let radius = (hw.min(hh) * LENS_CORNER).min(hw.min(hh));

    // Source rect: the lens rect shrunk about its centre by 1/zoom.
    let sw = (hw * 2.0 / zoom).max(1.0);
    let sh = (hh * 2.0 / zoom).max(1.0);
    let src = Rect::new(cx - sw / 2.0, cy - sh / 2.0, sw, sh);
    let sampled = resize_region(ctx.base, src, dw, dh);

    for y in 0..dh {
        for x in 0..dw {
            let px = ox + x as f32 + 0.5;
            let py = oy + y as f32 + 0.5;
            let cov = round_rect_coverage(px, py, cx, cy, hw, hh, radius);
            if cov <= 0.0 {
                continue;
            }
            let s = sampled.get_pixel(x, y).0;
            if s[3] == 0 {
                continue;
            }
            blend_px(
                img,
                i64::from(x) + ox as i64,
                i64::from(y) + oy as i64,
                Rgba::from_array(s),
                cov,
            );
        }
    }

    // Border: a 2 px band centred on the aperture edge.
    let band = LENS_BORDER * 0.5;
    let pad = (band + 2.0).ceil() as i64;
    for y in (oy as i64 - pad)..=(oy as i64 + dh as i64 + pad) {
        for x in (ox as i64 - pad)..=(ox as i64 + dw as i64 + pad) {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let d = round_rect_sdf(px, py, cx, cy, hw, hh, radius);
            let cov = (band + 0.5 - d.abs()).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(img, x, y, LENS_INK, cov);
            }
        }
    }

    // Zoom chip, bottom-right inside the aperture.
    // Lens chrome uses the live default family (`FONT_STACK` in the app).
    let Some(font) = (ctx.fonts)(&snagcraft_doc::default_text_family(), true) else {
        return;
    };
    let label = format!("{zoom:.1}×");
    let size = (hw.min(hh) * 2.0 * 0.16).clamp(9.0, 20.0);
    let scale = PxScale::from(size);
    let Some(tw) = text_width(font, &label, scale) else {
        return;
    };
    let pad = (size * 0.34).max(3.0);
    let chip_w = tw + pad * 2.0;
    let chip_h = size + pad * 1.6;
    let inset = LENS_BORDER + 3.0;
    if chip_w + inset * 2.0 > dw as f32 || chip_h + inset * 2.0 > dh as f32 {
        return;
    }
    let chip = Rect::new(
        ox + dw as f32 - inset - chip_w,
        oy + dh as f32 - inset - chip_h,
        chip_w,
        chip_h,
    );
    fill_round_rect(img, chip, chip_h / 2.0, LENS_CHIP);
    draw_text_centered(
        img,
        font,
        chip.center(),
        &label,
        Rgba::WHITE,
        Run { size, shear: 0.0 },
    );
}

/// Freehand stroke: round-cap, round-join polyline of dabs spaced at most
/// `width / 4` apart (so neighbouring dabs always overlap), nib clamped 2–44 px,
/// points capped at [`MAX_BRUSH_POINTS`].
fn draw_brush_stroke(img: &mut Surface, points: &[Point], color: Rgba, width: f32) {
    if points.is_empty() {
        return;
    }
    let width = clean_brush_width(width);
    let half = width / 2.0;
    let spacing = (width * 0.25).max(0.5);
    let (w, h) = (img.width() as f32, img.height() as f32);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let pts = &points[..points.len().min(MAX_BRUSH_POINTS)];
    if pts.len() == 1 {
        if let Some(p) = pts.first()
            && p.x.is_finite()
            && p.y.is_finite()
        {
            stamp_dab(img, p.x, p.y, half, color);
        }
        return;
    }
    let pad = half.ceil() + 1.0;
    let mut budget = MAX_LINE_STEPS;
    for pair in pts.windows(2) {
        if budget <= 0 {
            return;
        }
        let Some((a, b)) = clip_segment(pair[0], pair[1], pad, w, h) else {
            continue;
        };
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();
        if !len.is_finite() {
            continue;
        }
        let steps = ((len / spacing).ceil() as i64).clamp(1, budget);
        budget -= steps;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let cx = a.x + dx * t;
            let cy = a.y + dy * t;
            if !cx.is_finite() || !cy.is_finite() {
                break;
            }
            stamp_dab(img, cx, cy, half, color);
        }
    }
}

// --------------------------------------------------------------------- text

/// One text run's typography: size in px and the synthetic italic shear.
#[derive(Clone, Copy)]
struct Run {
    size: f32,
    shear: f32,
}

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
///
/// Synthetic italic: every glyph pixel row is shifted right by
/// `shear * (baseline - y)`, so the top of the glyph leans right and the
/// baseline stays put (`shear = 0` is pixel-identical to upright).
fn draw_text_at<F: Font>(
    img: &mut Surface,
    font: &F,
    left: f32,
    baseline: f32,
    text: &str,
    color: Rgba,
    run: Run,
) {
    let Some(size) = clean_text_size(run.size) else {
        return;
    };
    if !left.is_finite() || !baseline.is_finite() {
        return;
    }
    let shear = clean_shear(run.shear);
    let scale = PxScale::from(size);
    let right_limit = img.width() as f32 + size;
    let mut x = left;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(x, baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|px, py, cov| {
                let row = bounds.min.y + py as f32;
                let shift = shear * (baseline - row);
                blend_px(
                    img,
                    (bounds.min.x + px as f32 + shift) as i64,
                    (bounds.min.y + py as f32) as i64,
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
fn draw_text<F: Font>(img: &mut Surface, font: &F, pos: Point, text: &str, color: Rgba, run: Run) {
    let Some(size) = clean_text_size(run.size) else {
        return;
    };
    if !pos.x.is_finite() || !pos.y.is_finite() {
        return;
    }
    let scale = PxScale::from(size);
    let baseline = pos.y + font.as_scaled(scale).ascent();
    draw_text_at(img, font, pos.x, baseline, text, color, run);
}

/// Text centred (optically) on `center`: horizontal advance midpoint on `x`,
/// baseline half an ascent below `y`.
fn draw_text_centered<F: Font>(
    img: &mut Surface,
    font: &F,
    center: Point,
    text: &str,
    color: Rgba,
    run: Run,
) {
    let Some(size) = clean_text_size(run.size) else {
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
    draw_text_at(img, font, left, baseline, text, color, run);
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagcraft_algo::BLACKOUT_INK;
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

    fn is_white(p: [u8; 4]) -> bool {
        p == [255, 255, 255, 255]
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

    /// A real font for the text paths. Tests that need glyphs skip when the
    /// system has none (the library itself never loads fonts).
    fn test_font() -> Option<ab_glyph::FontVec> {
        for path in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ] {
            if let Ok(bytes) = std::fs::read(path)
                && let Ok(font) = ab_glyph::FontVec::try_from_vec(bytes)
            {
                return Some(font);
            }
        }
        None
    }

    fn no_font(_: &str, _: bool) -> Option<&'static ab_glyph::FontVec> {
        None
    }

    #[test]
    fn empty_document_is_a_base_clone() {
        let doc = white_doc();
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out, *doc.base);
        let styled = composite_styled(&doc, &no_font, 0.2);
        assert_eq!(styled, *doc.base);
    }

    // ------------------------------------------------------------------ arrow

    #[test]
    fn arrow_draws_shaft_and_head() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Arrow {
            id: 0,
            from: Point::new(10.0, 40.0),
            to: Point::new(50.0, 40.0),
            color: BLUE,
            width: 3.0,
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(is_blueish(px(&out, 30, 40)), "shaft {:?}", px(&out, 30, 40));
        // Head: the tip is at x=50; the triangle narrows to nothing there, so
        // the last covered centre is 48.
        assert!(is_blueish(px(&out, 48, 40)), "tip {:?}", px(&out, 48, 40));
        assert!(
            is_blueish(px(&out, 44, 40)),
            "head body {:?}",
            px(&out, 44, 40)
        );
        assert_eq!(px(&out, 0, 0), [255, 255, 255, 255]);
        // Live head is wider than the shaft: headWidth/2 = max(9, thickness) = 9,
        // so near the base of the head the ink reaches y = 40 ± ~8.
        assert!(is_blueish(px(&out, 36, 45)), "{:?}", px(&out, 36, 45));
        // ...but not past the head's half width.
        assert!(is_white(px(&out, 36, 49)), "{:?}", px(&out, 36, 49));
    }

    #[test]
    fn arrow_head_matches_live_stroke_width_scaling() {
        // tools.ts:1014-1018 — thickness = strokeWidth * 2;
        // headLength = max(16, thickness * 1.7); headWidth = max(18, thickness * 2).
        let from = Point::new(0.0, 0.0);
        let to = Point::new(100.0, 0.0);
        let thickness = 3.0 * 2.0;
        let head_len = (thickness * 1.7f32).max(16.0);
        let head_half = (thickness * 2.0f32).max(18.0) / 2.0;
        let tri = arrow_head_sized(from, to, head_len, head_half);
        assert_eq!(tri[0], to, "tip sits on the endpoint");
        assert!((tri[1].x - (100.0 - head_len)).abs() < 1e-4);
        assert!((tri[1].y - head_half).abs() < 1e-4);
        assert!((tri[2].y + head_half).abs() < 1e-4);
        // Thick arrows scale the head linearly (no clamp at the top).
        assert!((arrow_head_sized(from, to, 40.0 * 1.7, 40.0)[1].y - 40.0).abs() < 1e-4);
        // Degenerate input collapses to the endpoint instead of drawing rubbish.
        assert_eq!(arrow_head(from, from, 16.0), [from, from, from]);
    }

    // ------------------------------------------------------------------ shape

    #[test]
    fn shape_rect_is_an_outline() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(10.0, 10.0, 30.0, 30.0),
            color: RED,
            width: 2.0,
            shadow: Shadow::default(),
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
        assert!(is_white(px(&out, 25, 25)), "interior must stay clear");
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
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(is_reddish(px(&out, 10, 30)), "left {:?}", px(&out, 10, 30));
        assert!(is_reddish(px(&out, 30, 10)), "top {:?}", px(&out, 30, 10));
        assert!(is_white(px(&out, 30, 30)), "centre must stay clear");
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
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(
            is_reddish(px(&out, 25, 25)),
            "diagonal {:?}",
            px(&out, 25, 25)
        );
        assert!(is_white(px(&out, 40, 10)));
    }

    #[test]
    fn shape_triangle_outline_follows_live_geometry() {
        // Apex top-centre, base along the bottom edge.
        let mut doc = white_doc();
        let rect = Rect::new(10.0, 10.0, 40.0, 40.0);
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Triangle,
            rect,
            color: RED,
            width: 2.0,
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // Apex pixels: (10 + 40/2, 10) = (30, 10).
        assert!(is_reddish(px(&out, 30, 10)), "apex {:?}", px(&out, 30, 10));
        // Base corners.
        assert!(
            is_reddish(px(&out, 11, 49)),
            "left base {:?}",
            px(&out, 11, 49)
        );
        assert!(
            is_reddish(px(&out, 49, 49)),
            "right base {:?}",
            px(&out, 49, 49)
        );
        // Half way up the left edge sits around x = 20, y = 30.
        assert!(
            is_reddish(px(&out, 20, 30)),
            "left edge {:?}",
            px(&out, 20, 30)
        );
        // The middle of the base is empty (outline only) and the top corners
        // are outside the triangle.
        assert!(is_white(px(&out, 30, 45)));
        assert!(is_white(px(&out, 12, 12)));
    }

    #[test]
    fn shape_star_outline_uses_the_doc_path() {
        let mut doc = white_doc();
        let rect = Rect::new(8.0, 8.0, 48.0, 45.5);
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Star,
            rect,
            color: RED,
            width: 2.0,
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // Point-up tip: STAR_OUTLINE[0] is (50, 0) of a 96x91 box -> centre-x.
        assert!(
            is_reddish(px(&out, 32, 8)),
            "star tip {:?}",
            px(&out, 32, 8)
        );
        // Bottom-left leg: (20.6, 91) -> x = 8 + 20.6/96*48 ≈ 18.3, y = 8 + 45.5.
        assert!(
            is_reddish(px(&out, 18, 53)),
            "star leg {:?}",
            px(&out, 18, 53)
        );
        // The star's concave waist stays empty.
        assert!(is_white(px(&out, 32, 30)));
    }

    // ------------------------------------------------------------------- step

    #[test]
    fn step_draws_a_filled_circle() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(32.0, 32.0),
            number: 1,
            color: RED,
            radius: 10.0,
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(
            is_reddish(px(&out, 32, 32)),
            "centre {:?}",
            px(&out, 32, 32)
        );
        assert!(px(&out, 32, 23)[0] > 100, "rim is painted");
        assert!(is_white(px(&out, 0, 0)), "outside untouched");
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
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(px(&out, 23, 20)[0] > 100, "{:?}", px(&out, 23, 20));
    }

    // ------------------------------------------------------ highlighter/spot

    #[test]
    fn highlighter_multiplies_at_36_percent() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Highlighter {
            id: 0,
            rect: Rect::new(10.0, 10.0, 40.0, 20.0),
            color: Rgba::new(255, 255, 0, 255),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        let p = px(&out, 20, 15);
        // White * (1 - 0.36 + 0.36 * ink/255): R,G stay 255, B drops to 163.
        assert_eq!(p[0], 255, "red untouched by a yellow ink: {p:?}");
        assert_eq!(p[1], 255, "green untouched: {p:?}");
        assert!(
            (160..=166).contains(&p[2]),
            "blue multiplied by 0.64: {p:?}"
        );
        assert_eq!(p[3], 255, "highlighter never changes alpha");
        assert!(is_white(px(&out, 5, 5)), "outside untouched");
    }

    #[test]
    fn highlighter_tints_mid_grey_by_the_same_formula() {
        let base = snagcraft_raster::filled(Size::new(16, 16), Rgba::new(100, 100, 100, 255));
        let mut doc = Document::new(base);
        doc.add(AnnotObject::Highlighter {
            id: 0,
            rect: Rect::new(0.0, 0.0, 16.0, 16.0),
            color: Rgba::new(255, 255, 0, 255),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // 100 * (0.64 + 0.36 * 1.0) = 100 for R/G, 100 * 0.64 = 64 for B.
        let p = px(&out, 8, 8);
        assert_eq!(p[0], 100);
        assert!(
            (62..=66).contains(&p[2]),
            "blue multiplied from grey: {p:?}"
        );
    }

    #[test]
    fn spotlight_overlays_rgba_8_12_18_outside_the_hole() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(16.0, 16.0, 32.0, 32.0),
            dim: 0.5,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // src-over of rgba(8,12,18, 0.68*0.5 = 0.34) on white:
        // 8*0.34 + 255*0.66 = 171 (R), 12 -> 172, 18 -> 174.
        let outside = px(&out, 2, 2);
        assert!(
            (168..=176).contains(&outside[0]),
            "outside dimmed by the live overlay: {outside:?}"
        );
        assert!(
            outside[1] >= outside[0],
            "ink is slightly blue: {outside:?}"
        );
        assert_eq!(px(&out, 30, 30), [255, 255, 255, 255], "inside untouched");
        assert_eq!(px(&out, 15, 30)[0], outside[0], "hole edge is sharp");
    }

    #[test]
    fn spotlight_full_dim_is_almost_black_and_zero_is_a_noop() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(16.0, 16.0, 32.0, 32.0),
            dim: 1.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        let p = px(&out, 2, 2);
        assert!(p[0] < 95, "68% ink over white: {p:?}");

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

    // ---------------------------------------------------------------- callout

    #[test]
    fn callout_draws_outline_and_skips_text_without_font() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Callout {
            id: 0,
            rect: Rect::new(10.0, 10.0, 40.0, 30.0),
            text: "note".into(),
            color: RED,
            size: 14.0,
            family: "Inter".into(),
            shadow: Shadow::default(),
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
        assert!(is_white(px(&out, 30, 25)), "no font, no glyphs");
    }

    // ------------------------------------------------------------------ blur

    #[test]
    fn blur_object_lowers_region_variance() {
        let base = checkerboard(Size::new(64, 64));
        let mut doc = Document::new(base.clone());
        doc.add(AnnotObject::Blur {
            id: 0,
            rect: Rect::new(8.0, 8.0, 32.0, 32.0),
            mode: BlurMode::Gaussian,
            radius: 4, // below SECURE_BLUR_MIN -> effective 10
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
    fn blackout_uses_the_security_ink() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Blur {
            id: 0,
            rect: Rect::new(8.0, 8.0, 16.0, 16.0),
            mode: BlurMode::Blackout,
            radius: 0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(px(&out, 12, 12), BLACKOUT_INK);
        assert!(is_white(px(&out, 40, 40)));
    }

    // ----------------------------------------------------------------- lens

    #[test]
    fn magnify_enlarges_the_capture_about_the_lens_centre() {
        // Base: a 4px dark square centred on (32,32) of a white 64x64 capture.
        let mut base = snagcraft_raster::filled(Size::new(64, 64), Rgba::WHITE);
        for y in 30..34 {
            for x in 30..34 {
                base.get_pixel_mut(x, y).0 = [20, 20, 20, 255];
            }
        }
        let original = base.clone();
        let mut doc = Document::new(base);
        // Lens centred on the square, zoom 4: the square covers 32 +- 8.
        let rect = Rect::new(10.0, 10.0, 44.0, 44.0);
        doc.add(AnnotObject::Magnify {
            id: 0,
            rect,
            zoom: 4.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);

        // (38,30) is white in the capture but the lens shows the enlarged
        // square there -> proof the lens really magnifies.
        assert!(is_white(px(&original, 38, 30)), "capture is white there");
        let inner = px(&out, 38, 30);
        assert!(
            inner[0] < 90,
            "lens must magnify the dark square: {inner:?}"
        );
        // Centre of the lens stays dark.
        assert!(px(&out, 32, 32)[0] < 90);
        // 2px #1c1c1e border on the aperture edge.
        let border = px(&out, 10, 32);
        assert!(
            border[0] < 90 && border[1] < 90 && border[2] < 90,
            "lens border {:?}",
            border
        );
        assert!(is_white(px(&out, 5, 5)), "outside the lens untouched");
    }

    #[test]
    fn magnify_zoom_and_rect_are_hostile_safe() {
        let mut doc = white_doc();
        for (rect, zoom) in [
            (Rect::new(f32::NAN, 0.0, 10.0, 10.0), 3.0),
            (Rect::new(0.0, 0.0, 0.0, 0.0), 3.0),
            (Rect::new(-1e9, -1e9, 1e9, 1e9), f32::INFINITY),
            (Rect::new(10.0, 10.0, 40.0, 40.0), -5.0),
            (Rect::new(10.0, 10.0, 40.0, 40.0), 1e30),
        ] {
            doc.add(AnnotObject::Magnify { id: 0, rect, zoom });
        }
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out.dimensions(), (64, 64));
    }

    // ---------------------------------------------------------------- brush

    #[test]
    fn brush_stroke_has_no_gaps_at_max_width() {
        // Width 44 is the widest nib; spacing is width/4 = 11px, so the dabs
        // must still overlap along the whole stroke.
        let mut doc = white_doc();
        doc.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![Point::new(4.0, 32.0), Point::new(60.0, 32.0)],
            color: RED,
            width: 44.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        for x in 6..59 {
            let p = px(&out, x, 32);
            assert!(p[0] > 200 && p[1] < 60, "gap at x={x}: {p:?}");
        }
        // The nib really is 44 wide: 32 ± 20 is inked, ± 23 is not.
        assert!(px(&out, 30, 32 + 20)[0] > 100);
        assert!(is_white(px(&out, 30, 32 + 23)));
    }

    #[test]
    fn brush_stroke_clamps_width_and_point_count() {
        let mut doc = white_doc();
        doc.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![Point::new(20.0, 20.0), Point::new(44.0, 20.0)],
            color: RED,
            width: 1.0, // clamps up to 2
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(is_reddish(px(&out, 32, 20)));
        assert!(is_white(px(&out, 32, 24)), "2px nib stays thin");

        // A hostile point count plus wild coordinates must not hang or panic.
        let mut doc = white_doc();
        let mut points = vec![Point::new(10.0, 10.0); 120_000];
        points.push(Point::new(f32::INFINITY, f32::NAN));
        doc.add(AnnotObject::BrushStroke {
            id: 0,
            points,
            color: RED,
            width: f32::INFINITY,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out.dimensions(), (64, 64));
    }

    #[test]
    fn brush_single_point_is_a_round_dab() {
        let mut doc = white_doc();
        doc.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![Point::new(32.0, 32.0)],
            color: RED,
            width: 10.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(is_reddish(px(&out, 32, 32)));
        assert!(is_reddish(px(&out, 32, 35)));
        assert!(is_white(px(&out, 32, 40)));
    }

    // --------------------------------------------------------------- shadows

    #[test]
    fn highlighter_corners_are_rounded_live_radius() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Highlighter {
            id: 0,
            rect: Rect::new(10.0, 10.0, 40.0, 20.0),
            color: Rgba::new(255, 255, 0, 255),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // radius = min(8, 20/4) = 5: the very corner pixel is (mostly) outside
        // the rounded band, the top-edge middle is fully inside.
        let corner = px(&out, 10, 10);
        let edge = px(&out, 30, 10);
        assert!(
            corner[2] > edge[2] + 20,
            "corner less tinted than edge: corner {corner:?} edge {edge:?}"
        );
        assert!(is_white(px(&out, 5, 5)), "outside untouched");
    }

    #[test]
    fn spotlight_hole_corners_are_rounded_live_radius() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(16.0, 16.0, 32.0, 32.0),
            dim: 1.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // radius = min(12, 32/6) ≈ 5.3: pixel (17,17) sits on the rounded
        // corner arc (partial coverage), hole centre untouched, far outside
        // fully dimmed. The exact corner tip (16,16) is outside the rounding.
        let corner = px(&out, 17, 17);
        let inside = px(&out, 32, 32);
        let outside = px(&out, 2, 2);
        assert_eq!(inside, [255, 255, 255, 255], "hole centre untouched");
        assert!(
            corner[0] > outside[0] + 10 && corner[0] < 250,
            "corner partially dimmed: {corner:?} vs outside {outside:?}"
        );
    }

    #[test]
    fn shadow_render_is_offset_tinted_and_behind_the_object() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(20.0, 20.0, 20.0, 20.0),
            color: RED,
            width: 2.0,
            shadow: Shadow {
                enabled: true,
                color: Rgba::new(0, 0, 0, 255),
                blur: 0,
                offset_x: 4.0,
                offset_y: 4.0,
            },
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        // The shadow of the top edge lands at y = 24..26, x = 24..44.
        let s = px(&out, 30, 25);
        assert!(s[0] < 60 && s[1] < 60 && s[2] < 60, "shadow {:?}", s);
        // The object paints over its own shadow.
        assert!(is_reddish(px(&out, 30, 21)), "object on top");
        // No shadow above/left of the offset: those pixels are clean.
        assert!(is_white(px(&out, 15, 15)));
        // Far side of the shape still clean.
        assert!(is_white(px(&out, 50, 50)), "{:?}", px(&out, 50, 50));
    }

    #[test]
    fn shadow_blur_softens_the_edges() {
        let doc_with = |blur: u32| {
            let mut doc = white_doc();
            doc.add(AnnotObject::Shape {
                id: 0,
                shape: ShapeKind::Rect,
                rect: Rect::new(20.0, 20.0, 12.0, 12.0),
                color: RED,
                width: 2.0,
                shadow: Shadow {
                    enabled: true,
                    color: Rgba::new(0, 0, 0, 255),
                    blur,
                    offset_x: 0.0,
                    offset_y: 8.0,
                },
            });
            composite::<ab_glyph::FontVec>(&doc, None)
        };
        let hard = doc_with(0);
        let soft = doc_with(30);
        // 6px below the offset shadow's lower edge is clean without blur...
        let y = 20 + 12 + 8 + 6;
        assert!(is_white(px(&hard, 26, y)), "{:?}", px(&hard, 26, y));
        // ...and still tinted (but not solid) with a 30px blur.
        let p = px(&soft, 26, y);
        assert!(!is_white(p), "blur spreads the shadow: {p:?}");
        assert!(p[0] > 40, "the spread is a gradient, not solid ink: {p:?}");
    }

    #[test]
    fn shadow_is_skipped_when_disabled() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(20.0, 20.0, 20.0, 20.0),
            color: RED,
            width: 2.0,
            shadow: Shadow {
                enabled: false,
                color: Rgba::new(0, 0, 0, 255),
                blur: 10,
                offset_x: 8.0,
                offset_y: 8.0,
            },
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(is_white(px(&out, 35, 35)));
    }

    #[test]
    fn text_shadow_casts_from_the_glyph_silhouette() {
        let Some(font) = test_font() else {
            return;
        };
        let mut doc = white_doc();
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(8.0, 8.0),
            text: "HH".into(),
            color: RED,
            size: 32.0,
            bold: true,
            italic: false,
            family: "Inter".into(),
            shadow: Shadow {
                enabled: true,
                color: Rgba::new(0, 0, 0, 255),
                blur: 0,
                offset_x: 6.0,
                offset_y: 6.0,
            },
        });
        let out = composite::<ab_glyph::FontVec>(&doc, Some(&font));
        // Some pixel is dark grey shadow and some is red glyph ink.
        let mut has_shadow = false;
        let mut has_ink = false;
        for y in 0..64 {
            for x in 0..64 {
                let p = px(&out, x, y);
                if p[0] < 90 && p[1] < 90 && p[2] < 90 {
                    has_shadow = true;
                }
                if is_reddish(p) {
                    has_ink = true;
                }
            }
        }
        assert!(has_shadow, "text shadow silhouette missing");
        assert!(has_ink, "glyphs missing");
    }

    // ----------------------------------------------------------- styled/text

    #[test]
    fn styled_composite_with_zero_shear_matches_composite() {
        let Some(font) = test_font() else {
            return;
        };
        let mut doc = white_doc();
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(6.0, 10.0),
            text: "italic ok".into(),
            color: RED,
            size: 24.0,
            bold: false,
            italic: true,
            family: "Inter".into(),
            shadow: Shadow::default(),
        });
        let plain = composite(&doc, Some(&font));
        let styled = composite_styled(&doc, &|_: &str, _: bool| Some(&font), 0.0);
        assert_eq!(plain, styled, "shear 0 is pixel-identical to upright");
    }

    #[test]
    fn italic_shear_leans_the_glyphs_right() {
        let Some(font) = test_font() else {
            return;
        };
        let doc = {
            let mut doc = white_doc();
            doc.add(AnnotObject::Text {
                id: 0,
                pos: Point::new(10.0, 14.0),
                text: "H".into(),
                color: Rgba::BLACK,
                size: 40.0,
                bold: false,
                italic: true,
                family: "Inter".into(),
                shadow: Shadow::default(),
            });
            doc
        };
        let upright = composite_styled(&doc, &|_: &str, _: bool| Some(&font), 0.0);
        let slanted = composite_styled(&doc, &|_: &str, _: bool| Some(&font), 0.2);

        // Leftmost dark pixel per row, top rows vs bottom rows.
        let first_dark_x = |img: &Surface, y0: u32, y1: u32| -> Option<u32> {
            (y0..y1).find_map(|y| (0..img.width()).find(|&x| img.get_pixel(x, y).0[0] < 128))
        };
        let top_u = first_dark_x(&upright, 18, 28);
        let top_s = first_dark_x(&slanted, 18, 28);
        let bot_u = first_dark_x(&upright, 45, 55);
        let bot_s = first_dark_x(&slanted, 45, 55);
        let (Some(top_u), Some(top_s), Some(bot_u), Some(bot_s)) = (top_u, top_s, bot_u, bot_s)
        else {
            return; // no glyph pixels: font renderer unavailable
        };
        assert_eq!(top_u, bot_u, "upright H is vertical");
        assert!(
            top_s > bot_s + 2,
            "sheared H leans right: top x {top_s} vs bottom x {bot_s}"
        );
        assert_eq!(bot_s, bot_u, "the baseline row stays put");
    }

    #[test]
    fn font_lookup_receives_each_objects_family_and_bold_flag() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(4.0, 4.0),
            text: "x".into(),
            color: RED,
            size: 20.0,
            bold: true,
            italic: false,
            family: "Inter".into(),
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(4.0, 30.0),
            text: "y".into(),
            color: RED,
            size: 20.0,
            bold: false,
            italic: false,
            family: "JetBrains Mono".into(),
            shadow: Shadow::default(),
        });
        // Callout: family is honoured, bold is always false.
        doc.add(AnnotObject::Callout {
            id: 0,
            rect: Rect::new(2.0, 40.0, 40.0, 18.0),
            text: "note".into(),
            color: RED,
            size: 12.0,
            family: "Caveat".into(),
            shadow: Shadow::default(),
        });
        let calls = std::cell::RefCell::new(Vec::<(String, bool)>::new());
        let out = composite_styled::<ab_glyph::FontVec>(
            &doc,
            &|family: &str, bold: bool| {
                calls.borrow_mut().push((family.to_owned(), bold));
                None
            },
            0.0,
        );
        assert_eq!(out.dimensions(), (64, 64), "no font, no glyphs");
        assert_eq!(calls.borrow().len(), 3, "one lookup per text object");
        assert_eq!(calls.borrow()[0], ("Inter".to_string(), true));
        assert_eq!(calls.borrow()[1], ("JetBrains Mono".to_string(), false));
        assert_eq!(
            calls.borrow()[2],
            ("Caveat".to_string(), false),
            "callouts forward their family and are never bold"
        );
    }

    // -------------------------------------------------------------- hostile

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
            family: "Inter".into(),
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(4.0, 30.0),
            text: String::new(),
            color: RED,
            size: f32::NAN,
            bold: false,
            italic: false,
            family: "Inter".into(),
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Callout {
            id: 0,
            rect: Rect::new(2.0, 40.0, 50.0, 20.0),
            text: "x".repeat(10_000),
            color: RED,
            size: 9999.0,
            family: "Inter".into(),
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(50.0, 12.0),
            number: 42,
            color: RED,
            radius: 8.0,
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert!(px(&out, 50, 12)[0] > 100);
    }

    #[test]
    fn thick_stroke_has_no_gaps() {
        let mut doc = white_doc();
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Line,
            rect: Rect::new(4.0, 12.0, 56.0, 40.0),
            color: RED,
            width: 24.0,
            shadow: Shadow::default(),
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
            shadow: Shadow::default(),
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(px(&out, 5, 5), BLACKOUT_INK, "blackout painted first");
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
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Ellipse,
            rect: Rect::new(f32::NAN, 0.0, 1e30, 1e30),
            color: RED,
            width: f32::INFINITY,
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(-1e30, -1e30, 2e30, 2e30),
            color: RED,
            width: 2.0,
            shadow: Shadow {
                enabled: true,
                color: Rgba::new(0, 0, 0, 255),
                blur: u32::MAX,
                offset_x: -1e30,
                offset_y: 1e30,
            },
        });
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Star,
            rect: Rect::new(f32::NAN, f32::NAN, 10.0, 10.0),
            color: RED,
            width: 2.0,
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Triangle,
            rect: Rect::new(0.0, 0.0, f32::INFINITY, f32::INFINITY),
            color: RED,
            width: 2.0,
            shadow: Shadow::default(),
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
            shadow: Shadow::default(),
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
            family: "Inter".into(),
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Magnify {
            id: 0,
            rect: Rect::new(f32::INFINITY, 0.0, 10.0, 10.0),
            zoom: f32::NAN,
        });
        doc.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![Point::new(f32::NAN, 1.0), Point::new(f32::INFINITY, 2.0)],
            color: RED,
            width: f32::NAN,
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
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            dim: 0.4,
        });
        doc.add(AnnotObject::Magnify {
            id: 0,
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            zoom: 3.0,
        });
        let out = composite::<ab_glyph::FontVec>(&doc, None);
        assert_eq!(out.dimensions(), (0, 0));
    }
}
