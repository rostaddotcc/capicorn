#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L0 geometry: points, rects, sizes. Pure data, serde-friendly, hostile-input safe.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Normalized rect from two corner points (any order).
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        }
    }

    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }

    pub fn contains(&self, p: Point) -> bool {
        self.is_finite()
            && p.x >= self.x
            && p.x <= self.x + self.w
            && p.y >= self.y
            && p.y <= self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Integer pixel bounds clamped to an image of `w`×`h`. None when empty.
    pub fn clamp_to(&self, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        if w == 0 || h == 0 || !self.is_finite() {
            return None;
        }
        let x0 = self.x.floor().clamp(0.0, w as f32) as u32;
        let y0 = self.y.floor().clamp(0.0, h as f32) as u32;
        let x1 = (self.x + self.w).ceil().clamp(0.0, w as f32) as u32;
        let y1 = (self.y + self.h).ceil().clamp(0.0, h as f32) as u32;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0, y0, x1, y1))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub const fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_corners_normalizes() {
        let r = Rect::from_corners(Point::new(10.0, 20.0), Point::new(2.0, 5.0));
        assert_eq!(r, Rect::new(2.0, 5.0, 8.0, 15.0));
    }

    #[test]
    fn contains_edges() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(r.contains(Point::new(10.0, 10.0)));
        assert!(!r.contains(Point::new(10.1, 5.0)));
        assert!(!Rect::new(f32::NAN, 0.0, 1.0, 1.0).contains(Point::new(0.0, 0.0)));
    }

    #[test]
    fn clamp_to_bounds() {
        let r = Rect::new(-5.0, -5.0, 10.0, 10.0);
        assert_eq!(r.clamp_to(8, 8), Some((0, 0, 5, 5)));
        assert_eq!(Rect::new(9.0, 9.0, 2.0, 2.0).clamp_to(8, 8), None);
        assert_eq!(Rect::new(0.0, 0.0, 4.0, 4.0).clamp_to(0, 0), None);
        assert_eq!(Rect::new(f32::NAN, 0.0, 1.0, 1.0).clamp_to(8, 8), None);
        assert_eq!(
            Rect::new(2.0, 2.0, 100.0, 100.0).clamp_to(8, 8),
            Some((2, 2, 8, 8))
        );
    }
}
