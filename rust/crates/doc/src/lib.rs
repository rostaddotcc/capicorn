#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L1 document model: base image + annotation objects. Pure data + serde.
//! W1 object set: Arrow, Shape, Text, Blur, Step, Highlighter, Spotlight, Callout.
//! W2 adds Magnify + BrushStroke, the Triangle/Star shape kinds and the shared
//! [`Shadow`] styling carried by the five "stroked" kinds.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use snagcraft_color::Rgba;
use snagcraft_geom::{Point, Rect, Size};
use snagcraft_raster::Surface;

pub type ObjId = u64;

/// Hostile-input cap on one brush stroke's point count (applied by [`AnnotObject::sanitize`]).
pub const MAX_BRUSH_POINTS: usize = 100_000;

/// Largest drop shadow blur (px) the tools expose — also the load-time clamp.
pub const SHADOW_BLUR_MAX: u32 = 60;

/// Largest absolute drop shadow offset (px) — also the load-time clamp.
pub const SHADOW_OFFSET_MAX: f32 = 50.0;

/// The nominal box of the star outline below (live parity with `src/editor/tools.ts`).
pub const STAR_BOX_W: f32 = 96.0;
pub const STAR_BOX_H: f32 = 91.0;

/// rostad's own 5-point star outline, ported verbatim from the live `STAR_PATH`
/// (`M 50 0 L 61.8 34.5 …`). Units are the nominal 96 × 91 box; x spans 2..98.
pub const STAR_OUTLINE: [(f32, f32); 10] = [
    (50.0, 0.0),
    (61.8, 34.5),
    (98.0, 34.5),
    (68.7, 56.5),
    (79.4, 91.0),
    (50.0, 69.4),
    (20.6, 91.0),
    (31.3, 56.5),
    (2.0, 34.5),
    (38.2, 34.5),
];

/// Map [`STAR_OUTLINE`] into `rect` (renderers call this for [`ShapeKind::Star`]).
pub fn star_outline(rect: Rect) -> [Point; 10] {
    let mut out = [Point::new(0.0, 0.0); 10];
    for (slot, (bx, by)) in out.iter_mut().zip(STAR_OUTLINE) {
        *slot = Point::new(
            rect.x + bx / STAR_BOX_W * rect.w,
            rect.y + by / STAR_BOX_H * rect.h,
        );
    }
    out
}

fn clamp_offset(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-SHADOW_OFFSET_MAX, SHADOW_OFFSET_MAX)
    }
}

/// Drop-shadow styling shared by Arrow / Shape / Text / Step / Callout.
/// Off by default; the compositor draws it behind the object, export bakes it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    pub color: Rgba,
    pub blur: u32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Rgba::BLACK,
            blur: 4,
            offset_x: 0.0,
            offset_y: 4.0,
        }
    }
}

impl Shadow {
    /// Clamp to the documented ranges (`blur` 0..=60, offsets ±50) — hostile-input rule.
    pub fn clamped(self) -> Self {
        Self {
            enabled: self.enabled,
            color: self.color,
            blur: self.blur.min(SHADOW_BLUR_MAX),
            offset_x: clamp_offset(self.offset_x),
            offset_y: clamp_offset(self.offset_y),
        }
    }
}

/// Live parity default text family (TextStyle.fontFamily, tools.ts:395-400).
pub fn default_text_family() -> String {
    "Inter".to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rect,
    Ellipse,
    /// Diagonal of `rect`: (x,y) -> (x+w, y+h).
    Line,
    Triangle,
    /// 5-point star, see [`STAR_OUTLINE`].
    Star,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlurMode {
    Gaussian,
    Pixelate,
    Blackout,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum AnnotObject {
    Arrow {
        id: ObjId,
        from: Point,
        to: Point,
        color: Rgba,
        width: f32,
        #[serde(default)]
        shadow: Shadow,
    },
    Shape {
        id: ObjId,
        shape: ShapeKind,
        rect: Rect,
        color: Rgba,
        width: f32,
        #[serde(default)]
        shadow: Shadow,
    },
    Text {
        id: ObjId,
        pos: Point,
        text: String,
        color: Rgba,
        size: f32,
        bold: bool,
        italic: bool,
        #[serde(default = "default_text_family")]
        family: String,
        #[serde(default)]
        shadow: Shadow,
    },
    /// Non-destructive: the compositor applies it; export bakes it.
    Blur {
        id: ObjId,
        rect: Rect,
        mode: BlurMode,
        radius: u32,
    },
    Step {
        id: ObjId,
        pos: Point,
        number: u32,
        color: Rgba,
        radius: f32,
        #[serde(default)]
        shadow: Shadow,
    },
    Highlighter {
        id: ObjId,
        rect: Rect,
        color: Rgba,
    },
    /// Darkens everything OUTSIDE `rect` by `dim` (0..1).
    Spotlight {
        id: ObjId,
        rect: Rect,
        dim: f32,
    },
    Callout {
        id: ObjId,
        rect: Rect,
        text: String,
        color: Rgba,
        size: f32,
        #[serde(default = "default_text_family")]
        family: String,
        #[serde(default)]
        shadow: Shadow,
    },
    /// Magnified lens: `rect` on canvas shows the source around its centre at `zoom`×.
    Magnify {
        id: ObjId,
        rect: Rect,
        zoom: f32,
    },
    /// Freehand pen stroke through `points`, `width` wide.
    BrushStroke {
        id: ObjId,
        points: Vec<Point>,
        color: Rgba,
        width: f32,
    },
}

impl AnnotObject {
    pub fn id(&self) -> ObjId {
        match self {
            Self::Arrow { id, .. }
            | Self::Shape { id, .. }
            | Self::Text { id, .. }
            | Self::Blur { id, .. }
            | Self::Step { id, .. }
            | Self::Highlighter { id, .. }
            | Self::Spotlight { id, .. }
            | Self::Callout { id, .. }
            | Self::Magnify { id, .. }
            | Self::BrushStroke { id, .. } => *id,
        }
    }

    fn set_id(&mut self, new_id: ObjId) {
        match self {
            Self::Arrow { id, .. }
            | Self::Shape { id, .. }
            | Self::Text { id, .. }
            | Self::Blur { id, .. }
            | Self::Step { id, .. }
            | Self::Highlighter { id, .. }
            | Self::Spotlight { id, .. }
            | Self::Callout { id, .. }
            | Self::Magnify { id, .. }
            | Self::BrushStroke { id, .. } => *id = new_id,
        }
    }

    /// Clamp hostile input: shadow ranges, brush point count.
    pub fn sanitize(&mut self) {
        match self {
            Self::Arrow { shadow, .. }
            | Self::Shape { shadow, .. }
            | Self::Text { shadow, .. }
            | Self::Step { shadow, .. }
            | Self::Callout { shadow, .. } => *shadow = shadow.clamped(),
            Self::BrushStroke { points, .. } => points.truncate(MAX_BRUSH_POINTS),
            Self::Blur { .. }
            | Self::Highlighter { .. }
            | Self::Spotlight { .. }
            | Self::Magnify { .. } => {}
        }
    }

    /// Bounds for hit-testing / selection outlines (image coordinates).
    /// The drop shadow never extends these bounds.
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Arrow {
                from, to, width, ..
            } => {
                let pad = width.max(1.0) * 2.0 + 8.0;
                Rect::from_corners(
                    Point::new(from.x - pad, from.y - pad),
                    Point::new(to.x + pad, to.y + pad),
                )
            }
            Self::Shape { rect, width, .. } => {
                let pad = width.max(1.0);
                Rect::new(
                    rect.x - pad,
                    rect.y - pad,
                    rect.w + pad * 2.0,
                    rect.h + pad * 2.0,
                )
            }
            Self::Text {
                pos, text, size, ..
            } => {
                let w = text.chars().count() as f32 * size * 0.6 + 4.0;
                Rect::new(pos.x - 2.0, pos.y - 2.0, w, size * 1.4)
            }
            Self::Blur { rect, .. }
            | Self::Highlighter { rect, .. }
            | Self::Spotlight { rect, .. }
            | Self::Magnify { rect, .. } => *rect,
            Self::Step { pos, radius, .. } => {
                let r = radius.max(4.0);
                Rect::new(pos.x - r, pos.y - r, r * 2.0, r * 2.0)
            }
            Self::Callout { rect, .. } => *rect,
            Self::BrushStroke { points, width, .. } => {
                let pad = width.max(1.0);
                let mut it = points.iter().filter(|p| p.x.is_finite() && p.y.is_finite());
                match it.next() {
                    None => Rect::new(0.0, 0.0, 0.0, 0.0),
                    Some(first) => {
                        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
                        for p in it {
                            x0 = x0.min(p.x);
                            y0 = y0.min(p.y);
                            x1 = x1.max(p.x);
                            y1 = y1.max(p.y);
                        }
                        Rect::new(
                            x0 - pad,
                            y0 - pad,
                            (x1 - x0) + pad * 2.0,
                            (y1 - y0) + pad * 2.0,
                        )
                    }
                }
            }
        }
    }

    pub fn shifted(&self, dx: f32, dy: f32) -> Self {
        let mut out = self.clone();
        match &mut out {
            Self::Arrow { from, to, .. } => {
                from.x += dx;
                from.y += dy;
                to.x += dx;
                to.y += dy;
            }
            Self::Shape { rect, .. }
            | Self::Blur { rect, .. }
            | Self::Highlighter { rect, .. }
            | Self::Spotlight { rect, .. }
            | Self::Callout { rect, .. }
            | Self::Magnify { rect, .. } => {
                rect.x += dx;
                rect.y += dy;
            }
            Self::Text { pos, .. } | Self::Step { pos, .. } => {
                pos.x += dx;
                pos.y += dy;
            }
            Self::BrushStroke { points, .. } => {
                for p in points.iter_mut() {
                    p.x += dx;
                    p.y += dy;
                }
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    pub size: Size,
    pub base: Arc<Surface>,
    pub objects: Vec<AnnotObject>,
    pub next_id: ObjId,
}

impl Document {
    pub fn new(base: Surface) -> Self {
        let size = Size::new(base.width(), base.height());
        Self {
            size,
            base: Arc::new(base),
            objects: Vec::new(),
            next_id: 1,
        }
    }

    pub fn white(size: Size) -> Self {
        Self::new(snagcraft_raster::filled(size, Rgba::WHITE))
    }

    /// Assigns a fresh id, appends, returns the id. Sanitizes hostile geometry first.
    pub fn add(&mut self, mut obj: AnnotObject) -> ObjId {
        obj.sanitize();
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        obj.set_id(id);
        self.objects.push(obj);
        id
    }

    pub fn remove(&mut self, id: ObjId) -> bool {
        let before = self.objects.len();
        self.objects.retain(|o| o.id() != id);
        self.objects.len() < before
    }

    pub fn get(&self, id: ObjId) -> Option<&AnnotObject> {
        self.objects.iter().find(|o| o.id() == id)
    }

    pub fn get_mut(&mut self, id: ObjId) -> Option<&mut AnnotObject> {
        self.objects.iter_mut().find(|o| o.id() == id)
    }

    pub fn shift(&mut self, id: ObjId, dx: f32, dy: f32) -> bool {
        match self.get(id) {
            Some(obj) => {
                let moved = obj.shifted(dx, dy);
                if let Some(slot) = self.get_mut(id) {
                    *slot = moved;
                }
                true
            }
            None => false,
        }
    }

    /// Replace the text of a Text/Callout object (W14 inline re-edit).
    /// `false` for other kinds or an unknown id.
    pub fn set_text(&mut self, id: ObjId, text: String) -> bool {
        match self.get_mut(id) {
            Some(AnnotObject::Text { text: t, .. })
            | Some(AnnotObject::Callout { text: t, .. }) => {
                *t = text;
                true
            }
            _ => false,
        }
    }

    /// Replace the rect of a rect-based object (W14 select-mode resize).
    /// `false` for point/path kinds or an unknown id.
    pub fn set_rect(&mut self, id: ObjId, rect: Rect) -> bool {
        match self.get_mut(id) {
            Some(AnnotObject::Shape { rect: r, .. })
            | Some(AnnotObject::Blur { rect: r, .. })
            | Some(AnnotObject::Highlighter { rect: r, .. })
            | Some(AnnotObject::Spotlight { rect: r, .. })
            | Some(AnnotObject::Callout { rect: r, .. })
            | Some(AnnotObject::Magnify { rect: r, .. }) => {
                *r = rect;
                true
            }
            _ => false,
        }
    }

    /// Topmost object whose bounds contain `p`.
    pub fn hit_test(&self, p: Point) -> Option<ObjId> {
        self.objects
            .iter()
            .rev()
            .find(|o| o.bounds().contains(p))
            .map(|o| o.id())
    }

    /// Crop the canvas: `rect` is clamped to the base bounds (integer pixel grid),
    /// the base is copied into a smaller surface, every object is shifted by
    /// `(-x, -y)`, and objects entirely outside the crop are dropped. `None` when the
    /// clamped rect is empty (or non-finite). `next_id` is preserved.
    pub fn crop(&self, rect: Rect) -> Option<Document> {
        let (bw, bh) = self.base.dimensions();
        let (x0, y0, x1, y1) = rect.clamp_to(bw, bh)?;
        let (w, h) = (x1 - x0, y1 - y0);
        let base = image::imageops::crop_imm(&*self.base, x0, y0, w, h).to_image();
        let (fx, fy) = (x0 as f32, y0 as f32);
        let kept = Rect::new(fx, fy, w as f32, h as f32);

        let mut objects = Vec::with_capacity(self.objects.len());
        for obj in &self.objects {
            let b = obj.bounds();
            if !b.is_finite() {
                continue;
            }
            let outside = b.x + b.w <= kept.x
                || kept.x + kept.w <= b.x
                || b.y + b.h <= kept.y
                || kept.y + kept.h <= b.y;
            if outside {
                continue;
            }
            objects.push(obj.shifted(-fx, -fy));
        }

        Some(Document {
            size: Size::new(w, h),
            base: Arc::new(base),
            objects,
            next_id: self.next_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shadow_on() -> Shadow {
        Shadow {
            enabled: true,
            color: Rgba::new(10, 20, 30, 200),
            blur: 9,
            offset_x: -2.5,
            offset_y: 6.0,
        }
    }

    fn arrow() -> AnnotObject {
        AnnotObject::Arrow {
            id: 0,
            from: Point::new(0.0, 0.0),
            to: Point::new(50.0, 50.0),
            color: Rgba::new(255, 0, 0, 255),
            width: 3.0,
            shadow: Shadow::default(),
        }
    }

    fn shape(rect: Rect) -> AnnotObject {
        AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect,
            color: Rgba::BLACK,
            width: 2.0,
            shadow: Shadow::default(),
        }
    }

    #[test]
    fn add_assigns_sequential_ids() {
        let mut d = Document::white(Size::new(64, 64));
        let a = d.add(arrow());
        let b = d.add(arrow());
        assert_eq!((a, b), (1, 2));
        assert_eq!(d.get(a).map(|o| o.id()), Some(1));
        assert_eq!(d.next_id, 3);
    }

    #[test]
    fn remove_and_missing() {
        let mut d = Document::white(Size::new(64, 64));
        let a = d.add(arrow());
        assert!(d.remove(a));
        assert!(!d.remove(a));
        assert!(!d.remove(999));
        assert!(d.objects.is_empty());
    }

    #[test]
    fn shadow_default_is_disabled_black_blur4_offset_y4() {
        let s = Shadow::default();
        assert!(!s.enabled);
        assert_eq!(s.color, Rgba::BLACK);
        assert_eq!(s.blur, 4);
        assert_eq!((s.offset_x, s.offset_y), (0.0, 4.0));
    }

    #[test]
    fn shadow_is_clamped_on_add() {
        let mut d = Document::white(Size::new(16, 16));
        let id = d.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Star,
            rect: Rect::new(1.0, 1.0, 4.0, 4.0),
            color: Rgba::BLACK,
            width: 1.0,
            shadow: Shadow {
                enabled: true,
                color: Rgba::BLACK,
                blur: 5_000,
                offset_x: -900.0,
                offset_y: f32::NAN,
            },
        });
        match d.get(id) {
            Some(AnnotObject::Shape { shadow, .. }) => {
                assert_eq!(shadow.blur, SHADOW_BLUR_MAX);
                assert_eq!(shadow.offset_x, -SHADOW_OFFSET_MAX);
                assert_eq!(shadow.offset_y, 0.0);
                assert!(shadow.enabled);
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn shift_moves_all_kinds() {
        let mut d = Document::white(Size::new(64, 64));
        let t = d.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(5.0, 5.0),
            text: "hi".into(),
            color: Rgba::BLACK,
            size: 20.0,
            bold: false,
            italic: false,
            family: "Inter".to_string(),
            shadow: Shadow::default(),
        });
        assert!(d.shift(t, 3.0, 4.0));
        assert!(!d.shift(999, 1.0, 1.0));
        match d.get(t) {
            Some(AnnotObject::Text { pos, .. }) => assert_eq!(*pos, Point::new(8.0, 9.0)),
            _ => panic!("wrong kind"),
        }
        let s = d.add(shape(Rect::new(1.0, 1.0, 10.0, 10.0)));
        d.shift(s, -1.0, -1.0);
        // rect.x is now 0; bounds pad by width (2) → -2.
        assert_eq!(d.get(s).map(|o| o.bounds().x), Some(-2.0));
    }

    #[test]
    fn magnify_bounds_shift_and_hit() {
        let mut d = Document::white(Size::new(64, 64));
        let id = d.add(AnnotObject::Magnify {
            id: 0,
            rect: Rect::new(10.0, 20.0, 30.0, 15.0),
            zoom: 2.5,
        });
        // Bounds are exactly the rect (no stroke padding).
        assert_eq!(
            d.get(id).map(|o| o.bounds()),
            Some(Rect::new(10.0, 20.0, 30.0, 15.0))
        );
        assert_eq!(d.hit_test(Point::new(25.0, 25.0)), Some(id));
        assert_eq!(d.hit_test(Point::new(5.0, 5.0)), None);
        assert!(d.shift(id, 5.0, -5.0));
        match d.get(id) {
            Some(AnnotObject::Magnify { rect, zoom, .. }) => {
                assert_eq!(*rect, Rect::new(15.0, 15.0, 30.0, 15.0));
                assert_eq!(*zoom, 2.5);
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn brush_stroke_bounds_pad_by_width_and_shift() {
        let mut d = Document::white(Size::new(64, 64));
        let id = d.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![
                Point::new(10.0, 10.0),
                Point::new(30.0, 12.0),
                Point::new(12.0, 40.0),
            ],
            color: Rgba::BLACK,
            width: 4.0,
        });
        let b = d.get(id).map(|o| o.bounds()).unwrap_or_default();
        assert_eq!(b, Rect::new(6.0, 6.0, 28.0, 38.0));
        assert_eq!(d.hit_test(Point::new(20.0, 20.0)), Some(id));
        assert!(d.shift(id, 2.0, 3.0));
        match d.get(id) {
            Some(AnnotObject::BrushStroke { points, .. }) => {
                assert_eq!(points[0], Point::new(12.0, 13.0));
                assert_eq!(points[2], Point::new(14.0, 43.0));
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn brush_stroke_empty_points_is_empty_bounds() {
        let obj = AnnotObject::BrushStroke {
            id: 1,
            points: Vec::new(),
            color: Rgba::BLACK,
            width: 2.0,
        };
        assert_eq!(obj.bounds(), Rect::new(0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn brush_points_capped_on_add() {
        let mut d = Document::white(Size::new(16, 16));
        let pts = vec![Point::new(1.0, 1.0); MAX_BRUSH_POINTS + 250];
        let id = d.add(AnnotObject::BrushStroke {
            id: 0,
            points: pts,
            color: Rgba::BLACK,
            width: 1.0,
        });
        match d.get(id) {
            Some(AnnotObject::BrushStroke { points, .. }) => {
                assert_eq!(points.len(), MAX_BRUSH_POINTS)
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn star_outline_maps_into_rect() {
        let pts = star_outline(Rect::new(100.0, 50.0, 96.0, 91.0));
        assert_eq!(pts.len(), 10);
        // Top point of the star is centred horizontally, at the rect's top edge.
        assert_eq!(pts[0], Point::new(150.0, 50.0));
        // x spans 2..98 of the 96-unit box → 102 .. 198.
        assert_eq!(pts[2].x, 198.0);
        assert_eq!(pts[8].x, 102.0);
        // Every point stays inside the rect's vertical span.
        assert!(pts.iter().all(|p| p.y >= 50.0 && p.y <= 141.0));
    }

    #[test]
    fn hit_test_topmost_wins() {
        let mut d = Document::white(Size::new(64, 64));
        let bottom = d.add(shape(Rect::new(0.0, 0.0, 40.0, 40.0)));
        let top = d.add(shape(Rect::new(10.0, 10.0, 40.0, 40.0)));
        assert_eq!(d.hit_test(Point::new(15.0, 15.0)), Some(top));
        assert_eq!(d.hit_test(Point::new(2.0, 2.0)), Some(bottom));
        assert_eq!(d.hit_test(Point::new(60.0, 60.0)), None);
    }

    #[test]
    fn serde_round_trip_all_kinds() {
        let mut d = Document::white(Size::new(32, 32));
        d.add(arrow());
        d.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Star,
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: Rgba::BLACK,
            width: 2.0,
            shadow: shadow_on(),
        });
        d.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(1.0, 2.0),
            text: "x".into(),
            color: Rgba::BLACK,
            size: 12.0,
            bold: true,
            italic: false,
            family: "Inter".to_string(),
            shadow: shadow_on(),
        });
        d.add(AnnotObject::Blur {
            id: 0,
            rect: Rect::new(0.0, 0.0, 8.0, 8.0),
            mode: BlurMode::Pixelate,
            radius: 6,
        });
        d.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(4.0, 4.0),
            number: 1,
            color: Rgba::BLACK,
            radius: 12.0,
            shadow: shadow_on(),
        });
        d.add(AnnotObject::Highlighter {
            id: 0,
            rect: Rect::new(0.0, 0.0, 20.0, 6.0),
            color: Rgba::new(255, 255, 0, 90),
        });
        d.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(2.0, 2.0, 20.0, 20.0),
            dim: 0.6,
        });
        d.add(AnnotObject::Callout {
            id: 0,
            rect: Rect::new(0.0, 0.0, 30.0, 20.0),
            text: "note".into(),
            color: Rgba::BLACK,
            size: 14.0,
            family: "Inter".to_string(),
            shadow: shadow_on(),
        });
        d.add(AnnotObject::Magnify {
            id: 0,
            rect: Rect::new(1.0, 1.0, 12.0, 12.0),
            zoom: 3.0,
        });
        d.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![Point::new(0.0, 0.0), Point::new(5.0, 7.0)],
            color: Rgba::new(1, 2, 3, 255),
            width: 2.5,
        });
        let json = serde_json::to_string(&d.objects).unwrap();
        let back: Vec<AnnotObject> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d.objects);
        assert_eq!(back.len(), 10);
    }

    #[test]
    fn old_json_without_shadow_loads_default() {
        let json = r#"[
            {"kind":"Arrow","id":3,"from":{"x":0.0,"y":0.0},"to":{"x":1.0,"y":1.0},
             "color":{"r":0,"g":0,"b":0,"a":255},"width":2.0},
            {"kind":"Shape","id":4,"shape":"Triangle","rect":{"x":0.0,"y":0.0,"w":4.0,"h":4.0},
             "color":{"r":0,"g":0,"b":0,"a":255},"width":1.0},
            {"kind":"Callout","id":5,"rect":{"x":0.0,"y":0.0,"w":4.0,"h":4.0},
             "text":"hi","color":{"r":0,"g":0,"b":0,"a":255},"size":9.0}
        ]"#;
        let objs: Vec<AnnotObject> = serde_json::from_str(json).unwrap();
        assert_eq!(objs.len(), 3);
        assert_eq!(objs[0].id(), 3);
        for o in &objs {
            match o {
                AnnotObject::Arrow { shadow, .. }
                | AnnotObject::Shape { shadow, .. }
                | AnnotObject::Callout { shadow, .. } => assert_eq!(*shadow, Shadow::default()),
                _ => panic!("wrong kind"),
            }
        }
    }

    pub fn gradient(w: u32, h: u32) -> Surface {
        Surface::from_fn(w, h, |x, y| {
            image::Rgba([(x * 5) as u8, (y * 7) as u8, ((x + y) * 3) as u8, 255])
        })
    }

    #[test]
    fn crop_takes_pixels_and_resize() {
        let d = Document::new(gradient(40, 30));
        let c = d.crop(Rect::new(5.0, 4.0, 10.0, 8.0)).expect("crop");
        assert_eq!(c.size, Size::new(10, 8));
        assert_eq!(c.base.dimensions(), (10, 8));
        for (x, y) in [(0u32, 0u32), (9, 7), (4, 3)] {
            assert_eq!(
                c.base.get_pixel(x, y),
                d.base.get_pixel(x + 5, y + 4),
                "pixel {x},{y}"
            );
        }
        assert_eq!(c.next_id, d.next_id);
    }

    #[test]
    fn crop_clamps_to_base() {
        let d = Document::new(gradient(20, 10));
        let c = d.crop(Rect::new(-8.0, -4.0, 100.0, 100.0)).expect("crop");
        assert_eq!(c.size, Size::new(20, 10));
        assert_eq!(c.base.as_raw(), d.base.as_raw());
    }

    #[test]
    fn crop_empty_is_none() {
        let d = Document::white(Size::new(8, 8));
        assert!(d.crop(Rect::new(50.0, 50.0, 4.0, 4.0)).is_none());
        assert!(d.crop(Rect::new(0.0, 0.0, 0.0, 0.0)).is_none());
        assert!(d.crop(Rect::new(f32::NAN, 0.0, 4.0, 4.0)).is_none());
        assert!(d.crop(Rect::new(-20.0, -20.0, 10.0, 10.0)).is_none());
    }

    #[test]
    fn crop_shifts_and_drops_objects() {
        let mut d = Document::white(Size::new(100, 100));
        let inside = d.add(shape(Rect::new(20.0, 20.0, 10.0, 10.0)));
        let straddle = d.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(5.0, 45.0),
            text: "edge".into(),
            color: Rgba::BLACK,
            size: 10.0,
            bold: false,
            italic: false,
            family: "Inter".to_string(),
            shadow: Shadow::default(),
        });
        let outside = d.add(shape(Rect::new(80.0, 80.0, 10.0, 10.0)));
        d.next_id = 77;

        let c = d.crop(Rect::new(10.0, 10.0, 50.0, 50.0)).expect("crop");
        assert_eq!(c.size, Size::new(50, 50));
        assert_eq!(c.objects.len(), 2, "outside object must be dropped");
        assert_eq!(c.next_id, 77, "next_id survives a crop");
        assert!(c.get(outside).is_none());
        match c.get(inside) {
            Some(AnnotObject::Shape { rect, .. }) => {
                assert_eq!(*rect, Rect::new(10.0, 10.0, 10.0, 10.0))
            }
            _ => panic!("wrong kind"),
        }
        match c.get(straddle) {
            Some(AnnotObject::Text { pos, .. }) => {
                assert_eq!(*pos, Point::new(-5.0, 35.0), "partial overlap is kept")
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn crop_preserves_brush_and_magnify() {
        let mut d = Document::white(Size::new(60, 60));
        let m = d.add(AnnotObject::Magnify {
            id: 0,
            rect: Rect::new(20.0, 20.0, 10.0, 10.0),
            zoom: 2.0,
        });
        let b = d.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![Point::new(25.0, 25.0), Point::new(30.0, 30.0)],
            color: Rgba::BLACK,
            width: 2.0,
        });
        let c = d.crop(Rect::new(10.0, 10.0, 30.0, 30.0)).expect("crop");
        match c.get(m) {
            Some(AnnotObject::Magnify { rect, .. }) => assert_eq!(rect.x, 10.0),
            _ => panic!("magnify lost"),
        }
        match c.get(b) {
            Some(AnnotObject::BrushStroke { points, .. }) => {
                assert_eq!(points[0], Point::new(15.0, 15.0))
            }
            _ => panic!("brush lost"),
        }
    }

    #[test]
    fn white_doc_has_white_base() {
        let d = Document::white(Size::new(8, 8));
        assert_eq!(d.size, Size::new(8, 8));
        assert_eq!(d.base.get_pixel(4, 4).0, [255, 255, 255, 255]);
    }
}
