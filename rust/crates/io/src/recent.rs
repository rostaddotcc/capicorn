//! L4 recents: a bounded, newest-first scan of the save folder plus lazy
//! thumbnails for the filmstrip.
//!
//! Parity: `src/ui/app.ts:3864-3990` (the live filmstrip). The web build reads
//! its folder through Rust/tauri `list_folder_media`; here the scan is plain
//! `std::fs`, so it works on the desktop without a backend hop.
//!
//! Hostile input: the scan never recurses, caps the result at
//! [`RECENT_SCAN_MAX`] entries, ignores anything it cannot read and only ever
//! returns children of the folder it was given. Thumbnails decode through
//! [`crate::decode`], which enforces the byte and dimension caps.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use snagcraft_raster::Surface;

use crate::{ImageFormat, IoError};

/// How many recents the strip shows (live `RECENT_LIMIT`, `app.ts:672`).
pub const RECENT_LIMIT: usize = 12;

/// Hard ceiling on how many entries one scan may inspect and return, so a
/// folder with 200k files cannot stall the UI.
pub const RECENT_SCAN_MAX: usize = 64;

/// Extensions the strip considers an image (live `list_folder_media` set).
pub const RECENT_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Longest accepted thumbnail edge.
pub const MAX_THUMB_EDGE: u32 = 256;

/// One file-backed recent capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentEntry {
    pub path: PathBuf,
    /// File name only (what the strip and its tooltip show).
    pub name: String,
    pub mtime: SystemTime,
}

/// Newest-first image files in `dir`, at most `limit` (clamped to
/// [`RECENT_SCAN_MAX`]). `limit == 0` returns nothing. Unreadable entries are
/// skipped, never reported as errors.
pub fn scan_recent(dir: &Path, limit: usize) -> Vec<RecentEntry> {
    let limit = limit.min(RECENT_SCAN_MAX);
    if limit == 0 || !crate::files::is_usable_dir(dir) {
        return Vec::new();
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for entry in read.flatten().take(RECENT_SCAN_MAX * 8) {
        let path = entry.path();
        if !is_recent_image(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        entries.push(RecentEntry { path, name, mtime });
    }
    // Newest first; ties broken by name so the order is deterministic.
    entries.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| b.name.cmp(&a.name)));
    entries.truncate(limit);
    entries
}

/// True when `path` has one of the strip's image extensions (case-insensitive)
/// and names a file.
pub fn is_recent_image(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    path.is_file() && RECENT_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// Decode `path` and downscale it to fit `max_edge`, preserving aspect ratio.
/// `None` for anything unreadable (a broken thumb is never an error).
pub fn thumbnail(path: &Path, max_edge: u32) -> Option<Surface> {
    let max_edge = max_edge.clamp(1, MAX_THUMB_EDGE);
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    // W28: video containers the image decoder can't read get a first-frame
    // ffmpeg thumbnail (live `recording_thumb`). GIF stays on the image path —
    // the decoder reads its first frame directly. No image byte cap on video:
    // ffmpeg streams the file, it never loads it whole.
    if is_video_container(path) {
        return video_thumbnail(path, max_edge);
    }
    if meta.len() > crate::MAX_INPUT_BYTES as u64 {
        return None;
    }
    let img = crate::open(path).ok()?;
    Some(downscale(&img, max_edge))
}

/// Video containers needing ffmpeg for a thumbnail (live `MEDIA_VIDEO_EXTS`
/// minus gif, which the image decoder handles). Lowercase compare.
const VIDEO_THUMB_EXTS: [&str; 4] = ["mp4", "webm", "mov", "mkv"];

/// Whether a recent file is a video container that needs the off-thread ffmpeg
/// thumbnail path (the UI uses this to route around the synchronous image load).
pub fn is_video_container(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_THUMB_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// W28: first-frame thumbnail for a video recording — the live `recording_thumb`
/// recipe (`src-tauri/src/commands.rs:534`): ffmpeg pipes a scaled PNG of frame 1
/// (`-frames:v 1 -vf scale=W:-2`), decoded + downscaled here. Returns `None` when
/// ffmpeg is absent or can't read the file (the tile falls back to the film
/// placeholder). Slow (subprocess) — the caller runs it off the UI thread.
pub fn video_thumbnail(path: &Path, max_edge: u32) -> Option<Surface> {
    let max_edge = max_edge.clamp(1, MAX_THUMB_EDGE);
    if !path.is_file() {
        return None;
    }
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf"])
        .arg(format!("scale={max_edge}:-2"))
        .args(["-f", "image2pipe", "-vcodec", "png", "-"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let img = crate::decode(&out.stdout).ok()?;
    Some(downscale(&img, max_edge))
}

/// Nearest-neighbour downscale to fit `max_edge`; smaller images are returned
/// unchanged. Never upscales.
pub fn downscale(img: &Surface, max_edge: u32) -> Surface {
    let max_edge = max_edge.clamp(1, MAX_THUMB_EDGE);
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return img.clone();
    }
    if w <= max_edge && h <= max_edge {
        return img.clone();
    }
    let scale = (w.max(h)) as f64 / f64::from(max_edge);
    let nw = ((f64::from(w) / scale).round() as u32).clamp(1, max_edge);
    let nh = ((f64::from(h) / scale).round() as u32).clamp(1, max_edge);
    Surface::from_fn(nw, nh, |x, y| {
        let sx = (x as f64 * f64::from(w) / f64::from(nw)) as u32;
        let sy = (y as f64 * f64::from(h) / f64::from(nh)) as u32;
        *img.get_pixel(sx.min(w - 1), sy.min(h - 1))
    })
}

/// Extension implied by a recent file's path, when it is one of ours.
pub fn entry_format(entry: &RecentEntry) -> Option<ImageFormat> {
    match entry
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some(ImageFormat::Png),
        Some("jpg" | "jpeg") => Some(ImageFormat::Jpeg),
        Some("webp") => Some(ImageFormat::WebP),
        _ => None,
    }
}

/// Write `img` into `dir` as `name` (both under caller control). Kept here so
/// the engine's default-path save and the strip share one boundary.
///
/// An existing file is never overwritten: the name gains `-2`, `-3`, … (bounded)
/// so two captures in the same second both survive.
pub fn save_in_dir(
    img: &Surface,
    dir: &Path,
    name: &str,
    format: ImageFormat,
    quality: u8,
) -> Result<PathBuf, IoError> {
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(IoError::Unsupported);
    }
    std::fs::create_dir_all(dir).map_err(|e| IoError::Io(format!("{}: {e}", dir.display())))?;
    let path = unique_path(dir, name);
    crate::save(img, &path, format, quality)?;
    Ok(path)
}

/// `<dir>/<name>`, or the first free `<stem>-N.<ext>` (N = 2..=999).
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("capture");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    for n in 2..=999u32 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // A folder that crowded is beyond what a capture loop can produce; reuse
    // the stamped name rather than failing the save.
    dir.join(format!("{stem}-{}.{ext}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_dir(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("snagcraft-recent-{}-{n}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn write_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img = Surface::from_pixel(w, h, image::Rgba([20, 40, 60, 255]));
        let path = dir.join(name);
        crate::save(&img, &path, ImageFormat::Png, 100).expect("write png");
        path
    }

    fn age(path: &Path, secs: u64) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for mtime");
        let when = SystemTime::now() - std::time::Duration::from_secs(secs);
        file.set_modified(when).expect("set mtime");
    }

    #[test]
    fn scan_is_newest_first_and_capped() {
        let dir = tmp_dir("order");
        let older = write_png(&dir, "older.png", 4, 4);
        let newer = write_png(&dir, "newer.png", 4, 4);
        age(&older, 120);
        age(&newer, 5);

        let got = scan_recent(&dir, 12);
        assert_eq!(
            got.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["newer.png", "older.png"]
        );
        assert_eq!(got[0].path, newer);
        assert_eq!(scan_recent(&dir, 1).len(), 1);
        assert_eq!(scan_recent(&dir, 0).len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_ignores_non_images_and_directories() {
        let dir = tmp_dir("filter");
        write_png(&dir, "shot.png", 2, 2);
        std::fs::write(dir.join("notes.txt"), b"hi").expect("write txt");
        std::fs::write(dir.join("movie.mp4"), b"not really").expect("write mp4");
        std::fs::create_dir_all(dir.join("photo.jpg")).expect("dir named like an image");

        let got = scan_recent(&dir, 12);
        assert_eq!(got.len(), 1, "only the png is a recent image");
        assert_eq!(got[0].name, "shot.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_of_missing_or_root_folder_is_empty() {
        assert!(scan_recent(Path::new("/definitely/not/here"), 12).is_empty());
        // A filesystem root is refused outright rather than walked.
        assert!(scan_recent(Path::new("/"), 12).is_empty());
        assert!(!is_recent_image(Path::new("/")));
    }

    #[test]
    fn scan_caps_the_result_length() {
        let dir = tmp_dir("cap");
        for i in 0..8 {
            write_png(&dir, &format!("s{i}.png"), 2, 2);
        }
        assert_eq!(scan_recent(&dir, 3).len(), 3);
        // A hostile limit cannot exceed the scan ceiling.
        assert!(scan_recent(&dir, usize::MAX).len() <= RECENT_SCAN_MAX);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downscale_fits_the_edge_and_keeps_aspect() {
        let img = Surface::from_pixel(400, 200, image::Rgba([1, 2, 3, 255]));
        let small = downscale(&img, 64);
        assert_eq!(small.dimensions(), (64, 32));
        // Never upscales.
        let tiny = Surface::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
        assert_eq!(downscale(&tiny, 64).dimensions(), (8, 8));
        // Degenerate caps are clamped, not obeyed.
        assert_eq!(downscale(&img, 0).width(), 1);
    }

    #[test]
    fn thumbnail_decodes_and_shrinks() {
        let dir = tmp_dir("thumb");
        let path = write_png(&dir, "big.png", 300, 150);
        let thumb = thumbnail(&path, 64).expect("thumb");
        assert_eq!(thumb.dimensions(), (64, 32));
        assert!(thumbnail(&dir.join("gone.png"), 64).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_in_dir_rejects_a_name_with_separators() {
        let dir = tmp_dir("save");
        let img = Surface::from_pixel(2, 2, image::Rgba([9, 9, 9, 255]));
        assert!(save_in_dir(&img, &dir, "../escape.png", ImageFormat::Png, 90).is_err());
        assert!(save_in_dir(&img, &dir, "nested/escape.png", ImageFormat::Png, 90).is_err());
        assert!(save_in_dir(&img, &dir, "", ImageFormat::Png, 90).is_err());
        let path = save_in_dir(&img, &dir, "ok.png", ImageFormat::Png, 90).expect("save");
        assert!(path.is_file());
        assert_eq!(
            entry_format(&RecentEntry {
                path,
                name: "ok.png".into(),
                mtime: SystemTime::now(),
            }),
            Some(ImageFormat::Png)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W28: video containers route to the ffmpeg path; images (incl. gif, whose
    /// first frame the image decoder reads) stay on the synchronous path.
    #[test]
    fn is_video_container_routes_by_extension() {
        for ext in ["mp4", "webm", "mov", "mkv", "MP4", "WebM"] {
            assert!(
                is_video_container(Path::new(&format!("clip.{ext}"))),
                "{ext} is a video container"
            );
        }
        for ext in ["png", "jpg", "jpeg", "webp", "gif", "bmp", "txt", ""] {
            assert!(
                !is_video_container(Path::new(&format!("shot.{ext}"))),
                "{ext} is not a video container"
            );
        }
    }

    fn ffmpeg_present() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// W28: a real first-frame thumbnail from a synthetic clip (the live
    /// `recording_thumb` recipe). Self-skips where ffmpeg is absent (e.g. the
    /// Windows CI runner) — the extraction is only meaningful with the binary.
    #[test]
    fn video_thumbnail_extracts_a_first_frame() {
        if !ffmpeg_present() {
            return;
        }
        let dir = tmp_dir("vidthumb");
        let clip = dir.join("clip.mp4");
        // A 1-second solid-colour test clip, 64x48, 10 fps.
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("testsrc=size=64x48:rate=10:duration=1")
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .stdin(std::process::Stdio::null())
            .status()
            .expect("spawn ffmpeg fixture");
        assert!(made.success(), "fixture clip must encode");

        let thumb = video_thumbnail(&clip, 64).expect("a first frame");
        assert!(thumb.width() > 0 && thumb.height() > 0);
        assert!(thumb.width() <= 64, "scaled to the thumb edge");
        // The public dispatcher routes the mp4 through the same path.
        let via_dispatch = thumbnail(&clip, 64).expect("thumbnail dispatches video");
        assert!(via_dispatch.width() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W28: an unreadable / absent video yields None (the tile keeps its film
    /// placeholder), never a panic.
    #[test]
    fn video_thumbnail_of_a_missing_file_is_none() {
        assert!(video_thumbnail(Path::new("/no/such/clip.mp4"), 64).is_none());
    }
}
