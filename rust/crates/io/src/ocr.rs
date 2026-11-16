//! OCR ("grab text") — shells out to the system `tesseract` binary.
//!
//! Ported from the live Tauri app (`src-tauri/src/ocr.rs`, 291 lines). Why a
//! subprocess rather than `tesseract-sys`: the distro package already ships the
//! trained-data languages users actually want (eng, swe, …), needs no C++
//! toolchain in *our* build, and keeps every build reproducible on a machine
//! that only has `tesseract-ocr` installed.
//!
//! Flow of [`run_ocr`]:
//!
//! ```text
//! Surface → encode PNG → unique temp file in env::temp_dir()
//!     → spawn `tesseract <tmp> stdout -l <sanitised lang>`
//!     → drain stdout (capped) + stderr → wait with a 60 s deadline
//!     → clean_output → remove temp file → return
//! ```
//!
//! Hostile input is bounded at every step: the language is reduced to
//! `[a-z+_]`, the URL-free temp name is process-unique, stdout is capped at
//! [`MAX_OUTPUT_BYTES`], and a tesseract that hangs is killed after
//! [`OCR_TIMEOUT`]. The temp file is removed on **every** path — success, spawn
//! failure, non-zero exit and timeout — through a drop guard.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use snagcraft_raster::Surface;

use crate::{ImageFormat, IoError, encode};

/// Language used when the caller does not name one.
pub const DEFAULT_LANG: &str = "eng";

/// Languages the app's "grab text" menu offers. [`sanitize_lang`] accepts any
/// `[a-z+_]` combination, so `deu`, `fra` or `eng+swe` all work; this list only
/// drives UI defaults and docs.
pub const SUPPORTED_LANGS: &[&str] = &["eng", "swe", "eng+swe"];

/// Hard cap on recognised text handed back to the caller (1 MB). A hostile or
/// enormous image must not be able to balloon the engine's memory.
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Hard cap on how long one OCR pass may run before the child is killed.
pub const OCR_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest accepted language string, after sanitising.
const MAX_LANG_LEN: usize = 64;

/// Longest accepted `--list-langs` line (a trained-data name).
const MAX_LANG_NAME_LEN: usize = 32;

/// Most language names reported; keeps a hostile/misconfigured tessdata
/// directory from producing an unbounded list.
const MAX_LANGUAGES: usize = 512;

/// Monotonic part of the temp name, so two calls in the same microsecond still
/// get distinct files.
static COUNTER: AtomicU64 = AtomicU64::new(0);

const MISSING_BINARY: &str = "OCR needs the tesseract binary, which is not installed. \
     Install it with `sudo apt-get install tesseract-ocr` (add \
     `tesseract-ocr-swe` for Swedish).";

/// Reduce arbitrary input to something safe to hand tesseract's `-l` flag.
///
/// Tesseract language names are lowercase letters plus `+` for combinations
/// (`eng+swe`), sometimes with a suffix like `eng_best`. We keep `[a-z+_]` —
/// deliberately nothing else: no spaces, no dashes, no digits, no path
/// separators, so a hostile `lang` can never become an extra argument or a path
/// (`eng --psm 3` → `engpsm`).
///
/// Empty segments are dropped (`eng++swe` → `eng+swe`), the result is capped at
/// [`MAX_LANG_LEN`] characters, and an entirely empty result falls back to
/// [`DEFAULT_LANG`].
pub fn sanitize_lang(lang: &str) -> String {
    let lowered = lang.trim().to_ascii_lowercase();
    let kept: String = lowered
        .chars()
        .filter(|c| c.is_ascii_lowercase() || *c == '+' || *c == '_')
        .take(MAX_LANG_LEN)
        .collect();

    let joined = kept
        .split('+')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("+");

    if joined.is_empty() {
        DEFAULT_LANG.to_string()
    } else {
        joined
    }
}

/// Tidy tesseract's stdout: strip trailing spaces per line (tesseract pads
/// lines out), normalise CRLF, drop leading/trailing blank lines. Interior blank
/// lines survive — they are paragraph breaks in the source layout.
pub fn clean_output(raw: &str) -> String {
    let normalised = raw.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalised.split('\n').map(|line| line.trim_end()).collect();

    let first = lines.iter().position(|line| !line.is_empty());
    let last = lines.iter().rposition(|line| !line.is_empty());

    match (first, last) {
        (Some(first), Some(last)) => lines[first..=last].join("\n").trim().to_string(),
        _ => String::new(),
    }
}

/// True when a usable `tesseract` binary is on `PATH` (`tesseract --version`).
pub fn ocr_available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Trained-data languages this machine actually has (`tesseract --list-langs`).
///
/// Graceful by contract: no binary, a non-zero exit or an empty tessdata
/// directory all yield an empty vector rather than an error — the caller shows
/// an empty language menu instead of failing the whole export.
pub fn ocr_languages() -> Vec<String> {
    let output = Command::new("tesseract")
        .arg("--list-langs")
        .stdin(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_languages(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `--list-langs` stdout: the first line is a human sentence ending in
/// `(3):`, the rest are one name per line. Anything that is not a plausible
/// trained-data name is dropped.
fn parse_languages(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let name = line.trim();
        if name.is_empty() || name.ends_with(':') || name.len() > MAX_LANG_NAME_LEN {
            continue;
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        if out.iter().any(|seen| seen == name) {
            continue;
        }
        out.push(name.to_string());
        if out.len() >= MAX_LANGUAGES {
            break;
        }
    }
    out
}

/// Extract text from `img` with tesseract language `lang` (sanitised).
///
/// Blocking: an OCR pass costs hundreds of milliseconds, so callers on an event
/// loop should hand this to a worker thread. `lang` is sanitised here even when
/// the caller already did, so there is exactly one chokepoint.
pub fn run_ocr(img: &Surface, lang: &str) -> Result<String, IoError> {
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return Err(IoError::Io("cannot OCR an empty image".to_string()));
    }

    let png = encode(img, ImageFormat::Png, 100)?;
    let temp = TempPng::new(temp_png_path());
    std::fs::write(temp.path(), &png).map_err(|err| {
        IoError::Io(format!(
            "could not write the OCR temp file {}: {err}",
            temp.path().display()
        ))
    })?;

    let lang = sanitize_lang(lang);
    let mut child = Command::new("tesseract")
        .arg(temp.path())
        .arg("stdout")
        .arg("-l")
        .arg(&lang)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => IoError::Io(MISSING_BINARY.to_string()),
            _ => IoError::Io(format!("could not start tesseract: {err}")),
        })?;

    // Drain both pipes on their own threads: a child that fills a 64 KiB pipe
    // buffer would otherwise block forever and look like a hang.
    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| std::thread::spawn(move || read_capped(pipe, MAX_OUTPUT_BYTES)));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| std::thread::spawn(move || read_capped(pipe, 8 * 1024)));

    let status = wait_with_timeout(&mut child);
    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    let status = status?;

    if !status.success() {
        let detail = clean_output(&String::from_utf8_lossy(&stderr));
        return Err(IoError::Io(if detail.is_empty() {
            format!("tesseract exited with {status}")
        } else {
            format!("tesseract failed: {detail}")
        }));
    }

    Ok(clean_output(&String::from_utf8_lossy(&stdout)))
}

/// Collect a reader thread's bytes; a panicked reader contributes nothing
/// rather than poisoning the OCR result.
fn join_reader(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match handle {
        Some(handle) => handle.join().unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Read `reader` to EOF, keeping at most `cap` bytes.
///
/// Bytes past the cap are drained, not kept: the child must never block on a
/// full pipe just because we stopped caring about its output.
fn read_capped<R: Read>(mut reader: R, cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let room = cap.saturating_sub(out.len());
                if room > 0 {
                    out.extend_from_slice(&buf[..n.min(room)]);
                }
            }
            Err(_) => break,
        }
    }
    out
}

/// Poll `child` until it exits or [`OCR_TIMEOUT`] elapses, killing it on expiry.
fn wait_with_timeout(child: &mut Child) -> Result<std::process::ExitStatus, IoError> {
    let deadline = Instant::now() + OCR_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(err) => return Err(IoError::Io(format!("waiting for tesseract: {err}"))),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(IoError::Io(format!(
                "tesseract did not finish within {} seconds",
                OCR_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A unique, per-call PNG path inside the OS temp directory.
fn temp_png_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "snagcraft-ocr-{}-{}-{}.png",
        std::process::id(),
        stamp,
        n
    ))
}

/// Removes its file when dropped, so no early return can leak a temp PNG.
struct TempPng {
    path: PathBuf,
}

impl TempPng {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempPng {
    fn drop(&mut self) {
        // Not fatal: a leaked temp file is noise, a failed OCR is the story.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_defaults_and_round_trips() {
        assert_eq!(sanitize_lang("eng"), "eng");
        assert_eq!(sanitize_lang("swe"), "swe");
        assert_eq!(sanitize_lang("eng+swe"), "eng+swe");
        assert_eq!(sanitize_lang("  ENG  "), "eng");
        assert_eq!(sanitize_lang("eng_best"), "eng_best");
    }

    #[test]
    fn lang_is_sanitised_against_injection() {
        // Shell / path / argument injection attempts collapse to letters.
        assert_eq!(sanitize_lang("eng; rm -rf /"), "engrmrf");
        assert_eq!(sanitize_lang("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_lang("eng --psm 3"), "engpsm");
        assert_eq!(sanitize_lang("eng+swe+"), "eng+swe");
        assert_eq!(sanitize_lang("eng++swe"), "eng+swe");
        assert_eq!(sanitize_lang("eng\u{0}"), "eng");
        assert_eq!(sanitize_lang("$(whoami)"), "whoami");
    }

    #[test]
    fn empty_lang_falls_back_to_default() {
        assert_eq!(sanitize_lang(""), "eng");
        assert_eq!(sanitize_lang("   "), "eng");
        assert_eq!(sanitize_lang("+++"), "eng");
        assert_eq!(sanitize_lang("ÅÄÖ"), "eng");
    }

    #[test]
    fn lang_is_bounded() {
        let hostile = "a".repeat(10_000);
        assert_eq!(sanitize_lang(&hostile).len(), MAX_LANG_LEN);
    }

    #[test]
    fn output_is_trimmed_without_losing_paragraphs() {
        assert_eq!(
            clean_output("  hello  \n\n  world \n\n"),
            "hello\n\n  world"
        );
        assert_eq!(clean_output("a\r\nb\r"), "a\nb");
        assert_eq!(clean_output("\n\n   \n"), "");
        assert_eq!(clean_output(""), "");
    }

    #[test]
    fn every_offered_language_survives_sanitising() {
        for lang in SUPPORTED_LANGS {
            assert_eq!(sanitize_lang(lang), *lang, "{lang} must pass unchanged");
        }
    }

    #[test]
    fn list_langs_output_is_parsed() {
        let raw = "List of available languages in \"/usr/share/tesseract-ocr/5/tessdata/\" (3):\neng\nosd\nswe\n";
        assert_eq!(parse_languages(raw), vec!["eng", "osd", "swe"]);
        // Garbage, duplicates and path-ish lines are dropped.
        assert_eq!(
            parse_languages("header (1):\neng\neng\n../evil\nEN\n\n"),
            vec!["eng"]
        );
        assert!(parse_languages("").is_empty());
    }

    #[test]
    fn temp_paths_are_unique_and_in_the_temp_dir() {
        let a = temp_png_path();
        let b = temp_png_path();
        assert_ne!(a, b);
        assert_eq!(a.parent(), Some(std::env::temp_dir().as_path()));
        assert!(a.to_string_lossy().contains("snagcraft-ocr-"));
    }

    #[test]
    fn temp_png_is_removed_on_drop() {
        let path = temp_png_path();
        std::fs::write(&path, b"x").expect("write probe file");
        {
            let guard = TempPng::new(path.clone());
            assert!(guard.path().exists());
        }
        assert!(!path.exists(), "the drop guard must remove the file");
    }

    /// The W4c build host ships tesseract 5.5.0; a missing binary is a broken
    /// environment, not an acceptable result.
    #[test]
    fn tesseract_is_available_on_this_host() {
        assert!(
            ocr_available(),
            "tesseract must be on PATH for the OCR tests to mean anything"
        );
        let langs = ocr_languages();
        assert!(
            langs.iter().any(|lang| lang == "eng"),
            "eng traineddata missing: {langs:?}"
        );
    }

    #[test]
    fn blank_image_has_no_text() {
        let img = Surface::from_pixel(64, 32, image::Rgba([255, 255, 255, 255]));
        let text = run_ocr(&img, "eng").expect("tesseract should succeed");
        assert_eq!(text, "", "a blank image has no text");
    }

    #[test]
    fn empty_image_is_rejected_before_spawning() {
        let img = Surface::new(0, 0);
        assert!(run_ocr(&img, "eng").is_err());
    }

    /// End-to-end: a synthetic "HELLO" image through the real binary.
    ///
    /// The io crate has no font stack (that is `snagcraft-text` / `snagcraft-compose`,
    /// which would invert the layer order), so the test glyphs are a hand-drawn
    /// 5×7 block font blown up to 20 px per cell — the same shape tesseract reads
    /// off any real screenshot.
    #[test]
    fn ocr_reads_a_synthetic_text_image() {
        let img = blocky_text("HELLO", 24, 64);
        let text = run_ocr(&img, "eng").expect("tesseract should succeed");
        let normalised = text.to_ascii_uppercase();
        assert!(normalised.contains("HELLO"), "expected HELLO, got {text:?}");
    }

    #[test]
    fn ocr_honours_a_language_combination() {
        let img = blocky_text("HELLO", 20, 40);
        let text = run_ocr(&img, "eng+swe").expect("tesseract should succeed");
        assert!(text.to_ascii_uppercase().contains("HELLO"), "got {text:?}");
    }

    /// 5×7 bitmap for the letters the OCR test needs, as row strings.
    fn glyph(ch: char) -> Option<[&'static str; 7]> {
        Some(match ch {
            'H' => [
                "10001", "10001", "10001", "11111", "10001", "10001", "10001",
            ],
            'E' => [
                "11111", "10000", "10000", "11110", "10000", "10000", "11111",
            ],
            'L' => [
                "10000", "10000", "10000", "10000", "10000", "10000", "11111",
            ],
            'O' => [
                "11111", "10001", "10001", "10001", "10001", "10001", "11111",
            ],
            _ => return None,
        })
    }

    /// White surface with `text` drawn as black blocks, `cell` px per bitmap
    /// pixel and `pad` px of white margin on every side.
    fn blocky_text(text: &str, cell: u32, pad: u32) -> Surface {
        const GLYPH_COLS: u32 = 5;
        const GLYPH_ROWS: u32 = 7;
        const GAP_COLS: u32 = 1;

        let chars: Vec<[&'static str; 7]> = text.chars().filter_map(glyph).collect();
        let count = u32::try_from(chars.len()).unwrap_or(1).max(1);
        let width = pad * 2 + count * (GLYPH_COLS + GAP_COLS) * cell;
        let height = pad * 2 + GLYPH_ROWS * cell;
        let mut img = Surface::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));

        for (index, rows) in chars.iter().enumerate() {
            let origin_x = pad + u32::try_from(index).unwrap_or(0) * (GLYPH_COLS + GAP_COLS) * cell;
            for (row, line) in rows.iter().enumerate() {
                for (col, bit) in line.chars().enumerate() {
                    if bit != '1' {
                        continue;
                    }
                    let x0 = origin_x + u32::try_from(col).unwrap_or(0) * cell;
                    let y0 = pad + u32::try_from(row).unwrap_or(0) * cell;
                    for y in y0..(y0 + cell).min(height) {
                        for x in x0..(x0 + cell).min(width) {
                            img.get_pixel_mut(x, y).0 = [0, 0, 0, 255];
                        }
                    }
                }
            }
        }
        img
    }
}
