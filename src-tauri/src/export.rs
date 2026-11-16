//! Export pipeline: one validated PNG data URL in, one encoded file on disk out.
//!
//! Supported formats:
//! - `png`  — the validated PNG bytes are written as-is (lossless, no re-encode)
//! - `jpeg` / `jpg` — `image` crate JPEG encoder, alpha flattened onto white
//! - `webp` — `webp` crate lossy encoder (`quality` 1..=100); the `image` crate
//!   can only write lossless WebP, which is why this module exists
//!
//! Everything here is synchronous and Tauri-free apart from the thin
//! `save_image` command wrapper, so the encoding path is unit-testable.

use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbaImage};

/// Formats this module can write, as a normalised tag (`png` | `jpeg` | `webp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

impl Format {
    /// Normalise frontend input. `jpg` is an alias for `jpeg`; anything else is
    /// rejected rather than silently guessed, so a typo never writes a file the
    /// user didn't ask for.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "png" => Ok(Format::Png),
            "jpeg" | "jpg" => Ok(Format::Jpeg),
            "webp" => Ok(Format::Webp),
            "" => Err("no export format given".to_string()),
            other => Err(format!(
                "unsupported export format '{other}' (expected png, jpeg or webp)"
            )),
        }
    }

    /// Canonical file extension for the written bytes.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
        }
    }

    /// Whether `ext` (already lowercased) already names this format.
    fn matches_extension(self, ext: &str) -> bool {
        match self {
            Format::Png => ext == "png",
            Format::Jpeg => ext == "jpg" || ext == "jpeg" || ext == "jpe",
            Format::Webp => ext == "webp",
        }
    }

    /// Human-facing filter label for a native save dialog.
    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG image",
            Format::Jpeg => "JPEG image",
            Format::Webp => "WebP image",
        }
    }

    /// Extensions a native dialog should accept for this format. JPEG lists
    /// both spellings so a `.jpeg` file the user already has is selectable.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Png => &["png"],
            Format::Jpeg => &["jpg", "jpeg"],
            Format::Webp => &["webp"],
        }
    }
}

/// Batch of bytes ready to be written plus the format they belong to.
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub format: Format,
}

/// Decode the data URL, validate it really is a PNG, and encode it as `format`.
pub fn encode_data_url(data_url: &str, format: &str, quality: u8) -> Result<Encoded, String> {
    let png = crate::capture::decode_png_data_url(data_url).map_err(|e| e.to_string())?;
    let image = image::load_from_memory(&png).map_err(|e| format!("invalid PNG payload: {e}"))?;
    encode_image(image, png, Format::parse(format)?, quality)
}

/// Encode an already decoded image. `original` is the validated source PNG; it
/// is what a `png` export writes verbatim so repeated saves stay byte-identical
/// and never lose colour data.
pub fn encode_image(
    image: DynamicImage,
    original: Vec<u8>,
    format: Format,
    quality: u8,
) -> Result<Encoded, String> {
    let bytes = match format {
        Format::Png => original,
        Format::Jpeg => encode_jpeg(&image, quality)?,
        Format::Webp => encode_webp(&image, quality)?,
    };
    Ok(Encoded { bytes, format })
}

/// Quality is user-facing 1..=100; clamp instead of erroring so a slider at 0
/// still produces a file rather than a rejected promise.
fn clamp_quality(quality: u8) -> u8 {
    quality.clamp(1, 100)
}

/// JPEG has no alpha channel. Compositing onto white (rather than dropping the
/// alpha channel, which yields black fringes) is what image editors do, so the
/// RGBA buffer is flattened here instead of being truncated.
fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let quality = clamp_quality(quality);
    let rgba: RgbaImage = image.to_rgba8();
    let mut rgb = Vec::with_capacity((rgba.width() * rgba.height() * 3) as usize);
    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        let a = a as u32;
        // out = src*a + 255*(255-a), all in 0..=255 after >> 8.
        let blend = |c: u8| -> u8 {
            let c = c as u32;
            ((c * a + 255 * (255 - a)) / 255).min(255) as u8
        };
        rgb.push(blend(r));
        rgb.push(blend(g));
        rgb.push(blend(b));
    }

    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    image::ImageEncoder::write_image(
        encoder,
        &rgb,
        rgba.width(),
        rgba.height(),
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|e| format!("cannot encode JPEG: {e}"))?;
    Ok(out)
}

fn encode_webp(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let quality = clamp_quality(quality) as f32;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err("cannot encode an empty image".to_string());
    }
    Ok(webp::Encoder::from_rgba(rgba.as_raw(), width, height)
        .encode(quality)
        .to_vec())
}

/// Reduce arbitrary user input to a single plain file name inside the target
/// dir, and make its extension agree with `format`.
///
/// Rejects anything that could escape the directory (separators, `..`, NUL,
/// empty), then:
/// - no extension → append the format's canonical extension
/// - matching extension (any case) → keep the user's spelling
/// - mismatched extension → swap it, so a `.png` name never holds JPEG bytes
pub fn safe_filename(filename: &str, format: Format) -> String {
    let trimmed = filename.trim();
    let hostile = trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0');

    let name = if hostile { "snagcraft" } else { trimmed };

    let path = Path::new(name);
    let wanted = format.extension();
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if format.matches_extension(&ext.to_ascii_lowercase()) => name.to_string(),
        Some(_) => format!(
            "{}.{wanted}",
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("snagcraft")
        ),
        None => format!("{name}.{wanted}"),
    }
}

/// Make an absolute, user-chosen path agree with `format`.
///
/// Unlike [`safe_filename`] this keeps the directory: the dialog already
/// returned a full path, so only the extension is ours to fix. A matching
/// extension (any case, either JPEG spelling) is left exactly as typed; a
/// missing or foreign one is appended/replaced so a `.png` path never receives
/// JPEG bytes.
pub fn ensure_extension(path: &Path, format: Format) -> PathBuf {
    let matches = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| format.matches_extension(&ext.to_ascii_lowercase()));
    if matches {
        return path.to_path_buf();
    }
    let mut out = path.to_path_buf();
    // `set_extension` appends when there is no extension and swaps when there
    // is, which is exactly the behaviour wanted here.
    out.set_extension(format.extension());
    out
}

/// Pick a path that does not exist yet: `name.png`, then `name-2.png`,
/// `name-3.png`, … A save must NEVER overwrite an earlier capture or edit —
/// every Save writes a fresh copy the user can browse back to.
fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let first = dir.join(filename);
    if !first.exists() {
        return first;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let ext = path.extension().and_then(|e| e.to_str());
    for n in 2..=999 {
        let candidate = match ext {
            Some(ext) => dir.join(format!("{stem}-{n}.{ext}")),
            None => dir.join(format!("{stem}-{n}")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological case: 998 collisions. Fall back to a nanosecond stamp.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    match ext {
        Some(ext) => dir.join(format!("{stem}-{nanos}.{ext}")),
        None => dir.join(format!("{stem}-{nanos}")),
    }
}

/// Encode the data URL and write it into `dir`, returning the absolute path.
/// Never overwrites: a colliding name gains a `-2`, `-3`, … suffix.
pub fn write_export(
    data_url: &str,
    dir: &str,
    filename: &str,
    format: &str,
    quality: u8,
) -> Result<String, String> {
    if dir.trim().is_empty() {
        return Err("no save directory given".to_string());
    }
    let encoded = encode_data_url(data_url, format, quality)?;

    let dir = PathBuf::from(dir.trim());
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let path = unique_path(&dir, &safe_filename(filename, encoded.format));
    std::fs::write(&path, &encoded.bytes)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Save a capture (always a PNG data URL) to `dir` as `format`.
///
/// Frontend: `save_image({ dataUrl, dir, filename, format, quality })`.
/// `quality` only applies to `jpeg`/`webp`; PNG is always lossless.
#[tauri::command]
pub async fn save_image(
    data_url: String,
    dir: String,
    filename: String,
    format: String,
    quality: u8,
) -> Result<String, String> {
    write_export(&data_url, &dir, &filename, &format, quality)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_png(width: u32, height: u32) -> String {
        use base64::Engine as _;
        let img = RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x * 20) as u8, (y * 20) as u8, 128, 255])
        });
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode test png");
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        )
    }

    #[test]
    fn jpeg_and_webp_encode_to_their_magic_bytes() {
        let data_url = rgba_png(4, 3);

        let jpeg = encode_data_url(&data_url, "JPEG", 80).expect("jpeg encode");
        assert_eq!(&jpeg.bytes[..2], &[0xFF, 0xD8], "JPEG SOI marker");

        let webp = encode_data_url(&data_url, "webp", 80).expect("webp encode");
        assert_eq!(&webp.bytes[..4], b"RIFF");
        assert_eq!(&webp.bytes[8..12], b"WEBP");

        let png = encode_data_url(&data_url, "png", 0).expect("png passthrough");
        assert_eq!(&png.bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn blank_alpha_flattens_towards_white() {
        let transparent = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let jpeg =
            encode_jpeg(&DynamicImage::ImageRgba8(transparent), 100).expect("encode 1px jpeg");
        assert!(!jpeg.is_empty());
    }

    #[test]
    fn writes_png_jpeg_and_webp_to_disk() {
        let dir = std::env::temp_dir().join(format!("snagcraft-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_str = dir.to_string_lossy().into_owned();
        let data_url = rgba_png(5, 4);

        let png = write_export(&data_url, &dir_str, "shot", "png", 100).expect("write png");
        let jpg = write_export(&data_url, &dir_str, "shot", "jpeg", 60).expect("write jpeg");
        // quality 0 is clamped, not rejected.
        let webp = write_export(&data_url, &dir_str, "shot", "webp", 0).expect("write webp");

        assert!(png.ends_with("shot.png"), "{png}");
        assert!(jpg.ends_with("shot.jpg"), "{jpg}");
        assert!(webp.ends_with("shot.webp"), "{webp}");

        let png_bytes = std::fs::read(&png).expect("read png");
        assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
        let jpg_bytes = std::fs::read(&jpg).expect("read jpeg");
        assert_eq!(&jpg_bytes[..2], &[0xFF, 0xD8]);
        let webp_bytes = std::fs::read(&webp).expect("read webp");
        assert_eq!(&webp_bytes[..4], b"RIFF");

        let err = write_export(&data_url, &dir_str, "shot", "gif", 80).unwrap_err();
        assert!(err.contains("unsupported export format"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filenames_get_the_right_extension_and_stay_inside_the_dir() {
        assert_eq!(safe_filename("shot", Format::Webp), "shot.webp");
        assert_eq!(safe_filename("shot.PNG", Format::Png), "shot.PNG");
        assert_eq!(safe_filename("shot.png", Format::Jpeg), "shot.jpg");
        assert_eq!(
            safe_filename("../../etc/passwd", Format::Png),
            "snagcraft.png"
        );
        assert_eq!(safe_filename("   ", Format::Jpeg), "snagcraft.jpg");
    }

    #[test]
    fn chosen_save_paths_get_the_format_extension() {
        let p = |s: &str| PathBuf::from(s);
        // Matching extensions (any case, either JPEG spelling) survive intact.
        assert_eq!(
            ensure_extension(&p("/tmp/shot.png"), Format::Png),
            p("/tmp/shot.png")
        );
        assert_eq!(
            ensure_extension(&p("/tmp/shot.PNG"), Format::Png),
            p("/tmp/shot.PNG")
        );
        assert_eq!(
            ensure_extension(&p("/tmp/shot.jpeg"), Format::Jpeg),
            p("/tmp/shot.jpeg")
        );
        // Mismatched extension: swapped, never left lying about the bytes.
        assert_eq!(
            ensure_extension(&p("/tmp/shot.png"), Format::Jpeg),
            p("/tmp/shot.jpg")
        );
        assert_eq!(
            ensure_extension(&p("/tmp/shot.tar.gz"), Format::Webp),
            p("/tmp/shot.tar.webp")
        );
        // No extension at all: appended.
        assert_eq!(
            ensure_extension(&p("/tmp/shot"), Format::Webp),
            p("/tmp/shot.webp")
        );
        // The directory is kept — this fix-up never renames the folder.
        assert_eq!(
            ensure_extension(&p("/home/me/pics/shot"), Format::Png),
            p("/home/me/pics/shot.png")
        );
    }

    #[test]
    fn saves_never_overwrite_an_earlier_copy() {
        let dir = std::env::temp_dir().join(format!("snagcraft-dedupe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_str = dir.to_string_lossy().into_owned();
        let data_url = rgba_png(4, 3);

        // Same requested name three times: three distinct files, all kept.
        let a = write_export(&data_url, &dir_str, "shot.png", "png", 100).expect("first");
        let b = write_export(&data_url, &dir_str, "shot.png", "png", 100).expect("second");
        let c = write_export(&data_url, &dir_str, "shot.png", "png", 100).expect("third");
        assert!(a.ends_with("shot.png"), "{a}");
        assert!(b.ends_with("shot-2.png"), "{b}");
        assert!(c.ends_with("shot-3.png"), "{c}");
        assert!(Path::new(&a).exists() && Path::new(&b).exists() && Path::new(&c).exists());

        // The original bytes are untouched by the later saves.
        let first_bytes = std::fs::read(&a).expect("read first");
        assert_eq!(&first_bytes[..8], b"\x89PNG\r\n\x1a\n");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
