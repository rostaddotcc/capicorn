#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L0 color: RGBA8 + straight-alpha source-over blending. Pure math.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub const fn from_array([r, g, b, a]: [u8; 4]) -> Self {
        Self { r, g, b, a }
    }

    pub const BLACK: Self = Self::new(0, 0, 0, 255);
    pub const WHITE: Self = Self::new(255, 255, 255, 255);
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);

    /// Standard source-over with straight (unmultiplied) alpha.
    pub fn src_over(self, dst: Rgba) -> Rgba {
        let sa = self.a as f32 / 255.0;
        let da = dst.a as f32 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            return Self::TRANSPARENT;
        }
        let mix = |s: u8, d: u8| -> u8 {
            ((s as f32 * sa + d as f32 * da * (1.0 - sa)) / out_a).round() as u8
        };
        Rgba::new(
            mix(self.r, dst.r),
            mix(self.g, dst.g),
            mix(self.b, dst.b),
            (out_a * 255.0).round() as u8,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_over_anything_is_src() {
        let src = Rgba::new(10, 20, 30, 255);
        assert_eq!(src.src_over(Rgba::new(200, 200, 200, 255)), src);
    }

    #[test]
    fn transparent_over_dst_is_dst() {
        let dst = Rgba::new(1, 2, 3, 255);
        assert_eq!(Rgba::TRANSPARENT.src_over(dst), dst);
    }

    #[test]
    fn half_blend() {
        let src = Rgba::new(255, 0, 0, 128);
        let dst = Rgba::new(0, 0, 255, 255);
        let out = src.src_over(dst);
        assert!(out.r > 100 && out.r < 160, "red mixes in: {}", out.r);
        assert!(out.b > 100 && out.b < 160, "blue remains: {}", out.b);
        assert_eq!(out.a, 255);
    }

    #[test]
    fn array_round_trip() {
        let c = Rgba::new(9, 8, 7, 6);
        assert_eq!(Rgba::from_array(c.to_array()), c);
    }
}
