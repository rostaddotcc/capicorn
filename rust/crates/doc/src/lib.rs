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
//! Magnify/Brush/Bucket/Crop land with the W2 tool-parity wave.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use snagcraft_color::Rgba;
use snagcraft_geom::{Point, Rect, Size};
use snagcraft_raster::Surface;

pub type ObjId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rect,
    Ellipse,
    /// Diagonal of `rect`: (x,y) -> (x+w, y+h).
    Line,
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
    },
    Shape {
        id: ObjId,
        shape: ShapeKind,
        rect: Rect,
        color: Rgba,
        width: f32,
    },
    Text {
        id: ObjId,
        pos: Point,
        text: String,
        color: Rgba,
        size: f32,
        bold: bool,
        italic: bool,
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
            | Self::Callout { id, .. } => *id,
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
            | Self::Callout { id, .. } => *id = new_id,
        }
    }

    /// Bounds for hit-testing / selection outlines (image coordinates).
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
            | Self::Spotlight { rect, .. } => *rect,
            Self::Step { pos, radius, .. } => {
                let r = radius.max(4.0);
                Rect::new(pos.x - r, pos.y - r, r * 2.0, r * 2.0)
            }
            Self::Callout { rect, .. } => *rect,
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
            | Self::Callout { rect, .. } => {
                rect.x += dx;
                rect.y += dy;
            }
            Self::Text { pos, .. } | Self::Step { pos, .. } => {
                pos.x += dx;
                pos.y += dy;
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

    /// Assigns a fresh id, appends, returns the id.
    pub fn add(&mut self, mut obj: AnnotObject) -> ObjId {
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

    /// Topmost object whose bounds contain `p`.
    pub fn hit_test(&self, p: Point) -> Option<ObjId> {
        self.objects
            .iter()
            .rev()
            .find(|o| o.bounds().contains(p))
            .map(|o| o.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrow() -> AnnotObject {
        AnnotObject::Arrow {
            id: 0,
            from: Point::new(0.0, 0.0),
            to: Point::new(50.0, 50.0),
            color: Rgba::new(255, 0, 0, 255),
            width: 3.0,
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
        });
        assert!(d.shift(t, 3.0, 4.0));
        assert!(!d.shift(999, 1.0, 1.0));
        match d.get(t) {
            Some(AnnotObject::Text { pos, .. }) => assert_eq!(*pos, Point::new(8.0, 9.0)),
            _ => panic!("wrong kind"),
        }
        let s = d.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(1.0, 1.0, 10.0, 10.0),
            color: Rgba::BLACK,
            width: 2.0,
        });
        d.shift(s, -1.0, -1.0);
        // rect.x is now 0; bounds pad by width (2) → -2.
        assert_eq!(d.get(s).map(|o| o.bounds().x), Some(-2.0));
    }

    #[test]
    fn hit_test_topmost_wins() {
        let mut d = Document::white(Size::new(64, 64));
        let bottom = d.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(0.0, 0.0, 40.0, 40.0),
            color: Rgba::BLACK,
            width: 2.0,
        });
        let top = d.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Rect,
            rect: Rect::new(10.0, 10.0, 40.0, 40.0),
            color: Rgba::BLACK,
            width: 2.0,
        });
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
            shape: ShapeKind::Ellipse,
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            color: Rgba::BLACK,
            width: 2.0,
        });
        d.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(1.0, 2.0),
            text: "x".into(),
            color: Rgba::BLACK,
            size: 12.0,
            bold: true,
            italic: false,
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
        });
        let json = serde_json::to_string(&d.objects).unwrap();
        let back: Vec<AnnotObject> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d.objects);
    }

    #[test]
    fn white_doc_has_white_base() {
        let d = Document::white(Size::new(8, 8));
        assert_eq!(d.size, Size::new(8, 8));
        assert_eq!(d.base.get_pixel(4, 4).0, [255, 255, 255, 255]);
    }
}
