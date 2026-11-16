#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L0 raster: the surface type used across the engine. W1 keeps a flat
//! `image::RgbaImage`; tiled COW storage is a later-wave upgrade behind this alias.

use snagcraft_color::Rgba;
use snagcraft_geom::Size;

pub type Surface = image::RgbaImage;

/// Cap for input-derived image dimensions (hostile-input rule).
pub const MAX_DIM: u32 = 16_384;

pub fn filled(size: Size, color: Rgba) -> Surface {
    let w = size.w.min(MAX_DIM);
    let h = size.h.min(MAX_DIM);
    Surface::from_pixel(w, h, image::Rgba(color.to_array()))
}

/// True when both dimensions are within the engine cap.
pub fn dims_ok(w: u32, h: u32) -> bool {
    w > 0 && h > 0 && w <= MAX_DIM && h <= MAX_DIM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filled_is_exact() {
        let s = filled(Size::new(4, 3), Rgba::new(1, 2, 3, 255));
        assert_eq!(s.dimensions(), (4, 3));
        assert_eq!(s.get_pixel(2, 2).0, [1, 2, 3, 255]);
    }

    #[test]
    fn filled_caps_hostile_sizes() {
        let s = filled(Size::new(u32::MAX, u32::MAX), Rgba::WHITE);
        assert!(s.width() <= MAX_DIM && s.height() <= MAX_DIM);
    }

    #[test]
    fn dims_ok_rejects_zero_and_huge() {
        assert!(!dims_ok(0, 5));
        assert!(!dims_ok(5, 0));
        assert!(!dims_ok(MAX_DIM + 1, 5));
        assert!(dims_ok(1920, 1080));
    }
}
