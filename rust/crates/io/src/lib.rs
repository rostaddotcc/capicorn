#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L4 — image codecs: decode any supported format to a [`Surface`], encode PNG / JPEG / WebP.
//!
//! Notes locked in for W1:
//! * JPEG has no alpha channel, so encoding an RGBA surface drops alpha (see [`encode`]).
//! * WebP is **lossless only** in W1 (`image`'s encoder has no lossy path); lossy WebP is a
//!   documented W2 gap. `quality` is ignored for PNG and WebP.
//! * Input-derived dimensions are capped by [`snagcraft_raster::MAX_DIM`] (hostile input).

use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ExtendedColorType};
use snagcraft_raster::{MAX_DIM, Surface};

/// Hostile-input cap on bytes handed to the decoder (256 MB).
pub const MAX_INPUT_BYTES: usize = 256 * 1024 * 1024;

/// Container format used by [`encode`] / [`save`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
}

impl ImageFormat {
    /// Conventional file extension (no dot).
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }
}

/// L4 errors. Everything decodable-from-disk is a non-fatal error here, never a panic.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("io error: {0}")]
    Io(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("encode error: {0}")]
    Encode(String),
    /// Zero, or larger than [`MAX_DIM`], or more input bytes than we accept.
    #[error("image is too large or has a zero dimension (cap {MAX_DIM}px)")]
    TooLarge,
    #[error("unsupported image operation")]
    Unsupported,
}

/// Decode `bytes` into an RGBA surface, rejecting zero or oversized dimensions.
pub fn decode(bytes: &[u8]) -> Result<Surface, IoError> {
    if bytes.is_empty() {
        return Err(IoError::Decode("empty input".to_string()));
    }
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(IoError::TooLarge);
    }
    let img = image::load_from_memory(bytes).map_err(|e| IoError::Decode(e.to_string()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if !snagcraft_raster::dims_ok(w, h) {
        return Err(IoError::TooLarge);
    }
    Ok(rgba)
}

/// Encode `img` in `format`. `quality` (1..=100, clamped) only affects JPEG; PNG and WebP
/// (lossless) ignore it. JPEG output is RGB, so alpha is dropped.
pub fn encode(img: &Surface, format: ImageFormat, quality: u8) -> Result<Vec<u8>, IoError> {
    let (w, h) = img.dimensions();
    if !snagcraft_raster::dims_ok(w, h) {
        return Err(IoError::TooLarge);
    }
    let map = |e: image::ImageError| IoError::Encode(e.to_string());

    match format {
        ImageFormat::Png => {
            let mut cur = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(img.clone())
                .write_to(&mut cur, image::ImageFormat::Png)
                .map_err(map)?;
            Ok(cur.into_inner())
        }
        ImageFormat::Jpeg => {
            let mut cur = Cursor::new(Vec::new());
            let q = quality.clamp(1, 100);
            JpegEncoder::new_with_quality(&mut cur, q)
                .encode_image(&DynamicImage::ImageRgba8(img.clone()))
                .map_err(map)?;
            Ok(cur.into_inner())
        }
        ImageFormat::WebP => {
            let mut cur = Cursor::new(Vec::new());
            WebPEncoder::new_lossless(&mut cur)
                .encode(img.as_raw(), w, h, ExtendedColorType::Rgba8)
                .map_err(map)?;
            Ok(cur.into_inner())
        }
    }
}

/// Read `path` and decode it.
pub fn open(path: &Path) -> Result<Surface, IoError> {
    let bytes = std::fs::read(path).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))?;
    decode(&bytes)
}

/// Encode `img` and write it to `path`.
pub fn save(img: &Surface, path: &Path, format: ImageFormat, quality: u8) -> Result<(), IoError> {
    let bytes = encode(img, format, quality)?;
    std::fs::write(path, &bytes).map_err(|e| IoError::Io(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn surface(w: u32, h: u32) -> Surface {
        Surface::from_fn(w, h, |x, y| {
            image::Rgba([
                (x * 7 % 256) as u8,
                (y * 11 % 256) as u8,
                ((x + y) * 3 % 256) as u8,
                255,
            ])
        })
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Surface {
        Surface::from_pixel(w, h, image::Rgba(px))
    }

    fn tmp_path(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("snagcraft-io-{}-{n}-{tag}", std::process::id());
        std::env::temp_dir().join(name)
    }

    #[test]
    fn png_round_trip_is_byte_exact() {
        let src = surface(17, 9);
        let bytes = encode(&src, ImageFormat::Png, 90).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.dimensions(), (17, 9));
        assert_eq!(back.as_raw(), src.as_raw());
    }

    #[test]
    fn png_preserves_alpha() {
        let src = solid(4, 4, [10, 20, 30, 0]);
        let back = decode(&encode(&src, ImageFormat::Png, 100).unwrap()).unwrap();
        assert_eq!(back.get_pixel(2, 2).0, [10, 20, 30, 0]);
    }

    #[test]
    fn jpeg_round_trip_keeps_dims_and_moves_color_less_than_tolerance() {
        let src = solid(24, 16, [128, 96, 64, 255]);
        let bytes = encode(&src, ImageFormat::Jpeg, 95).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.dimensions(), (24, 16));
        let got = back.get_pixel(12, 8).0;
        let want = [128u8, 96, 64, 255];
        for (g, w) in got.iter().zip(want.iter()) {
            let d = (*g as i32 - *w as i32).abs();
            assert!(d <= 12, "jpeg drift too high: got {got:?} want {want:?}");
        }
        assert_eq!(got[3], 255, "jpeg output is opaque");
    }

    #[test]
    fn webp_round_trip_is_lossless() {
        let src = surface(13, 7);
        let bytes = encode(&src, ImageFormat::WebP, 80).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        let back = decode(&bytes).unwrap();
        assert_eq!(back.dimensions(), (13, 7));
        assert_eq!(back.as_raw(), src.as_raw());
    }

    #[test]
    fn decode_rejects_garbage_and_empty() {
        assert!(matches!(decode(b""), Err(IoError::Decode(_))));
        assert!(matches!(
            decode(b"this is definitely not an image"),
            Err(IoError::Decode(_))
        ));
    }

    #[test]
    fn open_missing_file_errors() {
        let p = tmp_path("missing.png");
        assert!(matches!(open(&p), Err(IoError::Io(_))));
    }

    #[test]
    fn save_then_open_round_trip() {
        let src = surface(8, 8);
        let p = tmp_path("rt.png");
        save(&src, &p, ImageFormat::Png, 100).unwrap();
        let back = open(&p).unwrap();
        assert_eq!(back.as_raw(), src.as_raw());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_into_missing_dir_errors() {
        let p = std::env::temp_dir().join("snagcraft-no-such-dir-xyz/out.png");
        assert!(matches!(
            save(&solid(2, 2, [0, 0, 0, 255]), &p, ImageFormat::Png, 100),
            Err(IoError::Io(_))
        ));
    }

    #[test]
    fn quality_extremes_clamp_without_panic() {
        let src = solid(16, 16, [200, 30, 90, 255]);
        for q in [0u8, 1, 255] {
            let bytes = encode(&src, ImageFormat::Jpeg, q).unwrap();
            assert!(!bytes.is_empty());
            let back = decode(&bytes).unwrap();
            assert_eq!(back.dimensions(), (16, 16));
        }
    }

    #[test]
    fn extension_is_stable() {
        assert_eq!(ImageFormat::Png.extension(), "png");
        assert_eq!(ImageFormat::Jpeg.extension(), "jpg");
        assert_eq!(ImageFormat::WebP.extension(), "webp");
    }
}
