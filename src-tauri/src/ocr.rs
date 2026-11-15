//! OCR ("grab text") — shells out to the system `tesseract` binary.
//!
//! Why a subprocess: tesseract ships as a normal distro package with the
//! trained-data languages users actually want (eng, swe, …), needs no C++
//! toolchain in *our* build, and a 19 MB ghostscript-free binary is exactly the
//! dependency an app like this is allowed to have. Linking `tesseract-sys`
//! instead would drag leptonica headers into every build and still need the
//! same language files on disk.
//!
//! Flow of `ocr_image`:
//!
//! ```text
//! data:image/png;base64,…  → decode (capture::decode_png_data_url)
//!     → temp file in env::temp_dir() (unique per call)
//!     → spawn_blocking(tesseract <tmp> stdout -l <lang>)
//!     → stdout text → clean → remove temp file → return
//! ```
//!
//! The subprocess runs on a blocking thread so a 2 s OCR pass never parks the
//! async runtime. The temp file is removed on every path — success, non-zero
//! exit and spawn failure — so a failed grab leaves nothing behind.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Language used when the frontend does not name one.
pub const DEFAULT_LANG: &str = "eng";

/// Languages Capicorn offers in its menu. `sanitize_lang` still accepts any
/// `[a-z+]` combination (so `deu`, `fra+eng` work if the user asks for them);
/// this list only drives the UI defaults and the docs.
pub const SUPPORTED_LANGS: &[&str] = &["eng", "swe", "eng+swe"];

/// Monotonic part of the temp file name, so two OCR calls in the same
/// microsecond still get distinct files.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Reduce arbitrary input to something safe to hand tesseract's `-l` flag.
///
/// Tesseract language names are lowercase letters plus `+` for combinations
/// (`eng+swe`), sometimes with a suffix like `eng_best`. We keep `[a-z+_]` —
/// deliberately nothing else: no spaces, no dashes, no path separators, so a
/// hostile `lang` can never turn into an argument or a path.
///
/// Empty segments are dropped (`eng++swe` → `eng+swe`, `+` → `"eng"`), and an
/// entirely empty result falls back to [`DEFAULT_LANG`].
pub fn sanitize_lang(lang: &str) -> String {
    let lowered = lang.trim().to_ascii_lowercase();
    let kept: String = lowered
        .chars()
        .filter(|c| c.is_ascii_lowercase() || *c == '+' || *c == '_')
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
/// lines), drop leading/trailing blank lines, normalise CRLF. Interior blank
/// lines survive — they are paragraph breaks in the original layout.
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

/// A unique, per-call PNG path inside the OS temp directory.
fn temp_png_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "capicorn-ocr-{}-{}-{}.png",
        std::process::id(),
        stamp,
        n
    ))
}

/// True when a `tesseract` binary is on `PATH` (used to fail fast with a
/// message that tells the user what to install).
fn tesseract_version_line() -> Option<String> {
    let output = Command::new("tesseract")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string(),
    )
}

/// Decode → temp file → tesseract → text. Blocking; call it from
/// `spawn_blocking`.
fn run_tesseract(png: &[u8], lang: &str) -> Result<String, String> {
    let path = temp_png_path();
    std::fs::write(&path, png)
        .map_err(|e| format!("could not write the OCR temp file {}: {e}", path.display()))?;

    // Everything below must remove the temp file, including early returns.
    #[allow(clippy::redundant_closure_call)] // the closure gives us one cleanup path
    let result = (|| {
        let output = Command::new("tesseract")
            .arg(&path)
            .arg("stdout")
            .arg("-l")
            .arg(lang)
            .stdin(Stdio::null())
            .output();

        match output {
            Ok(output) if output.status.success() => {
                Ok(clean_output(&String::from_utf8_lossy(&output.stdout)))
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = stderr.trim();
                Err(if detail.is_empty() {
                    format!("tesseract exited with {}", output.status)
                } else {
                    format!("tesseract failed: {detail}")
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Err("OCR needs the tesseract binary, which is not installed. \
                 Install it with `sudo apt install tesseract-ocr` (add \
                 `tesseract-ocr-swe` for Swedish)."
                    .to_string())
            }
            Err(err) => Err(format!("could not start tesseract: {err}")),
        }
    })();

    if let Err(err) = std::fs::remove_file(&path) {
        // Not fatal: a leaked temp file is noise, a failed OCR is the story.
        eprintln!(
            "capicorn: could not remove OCR temp file {}: {err}",
            path.display()
        );
    }

    result
}

/// Extract text from a PNG data URL.
///
/// * `data_url` — a full `data:image/png;base64,…` URL (same contract as every
///   other command).
/// * `lang` — optional tesseract language, defaulting to `eng`; the value is
///   sanitised to `[a-z+_]` before use, so `swe` and `eng+swe` both work.
///
/// Returns the recognised text (trimmed, possibly empty when the image holds
/// no text). Errors carry a user-readable message: unreadable payload, missing
/// binary, or tesseract's own diagnostic (e.g. a language pack that is not
/// installed).
#[tauri::command]
pub async fn ocr_image(data_url: String, lang: Option<String>) -> Result<String, String> {
    let png = crate::capture::decode_png_data_url(&data_url).map_err(|e| e.to_string())?;
    // Cheap sanity check before paying for a subprocess: a payload that is not
    // a decodable PNG fails here with a clear message.
    image::load_from_memory(&png).map_err(|e| format!("invalid PNG payload: {e}"))?;

    let lang = sanitize_lang(lang.as_deref().unwrap_or(DEFAULT_LANG));

    tauri::async_runtime::spawn_blocking(move || run_tesseract(&png, &lang))
        .await
        .map_err(|err| format!("OCR worker failed: {err}"))?
}

/// Probe used by the docs/UI: does this machine have tesseract at all?
#[tauri::command]
pub fn ocr_available() -> bool {
    tesseract_version_line().is_some()
}

/// The languages the "grab text" menu offers (all of them pass `sanitize_lang`).
#[tauri::command]
pub fn ocr_languages() -> Vec<String> {
    SUPPORTED_LANGS
        .iter()
        .map(|lang| lang.to_string())
        .collect()
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
    }

    #[test]
    fn lang_is_sanitised_against_injection() {
        // Shell/path/argument injection attempts collapse to letters.
        assert_eq!(sanitize_lang("eng; rm -rf /"), "engrmrf");
        assert_eq!(sanitize_lang("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_lang("eng --psm 3"), "engpsm");
        assert_eq!(sanitize_lang("eng+swe+"), "eng+swe");
        assert_eq!(sanitize_lang("eng++swe"), "eng+swe");
    }

    #[test]
    fn empty_lang_falls_back_to_default() {
        assert_eq!(sanitize_lang(""), "eng");
        assert_eq!(sanitize_lang("   "), "eng");
        assert_eq!(sanitize_lang("+++"), "eng");
        assert_eq!(sanitize_lang("ÅÄÖ"), "eng");
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
        assert_eq!(ocr_languages().len(), SUPPORTED_LANGS.len());
    }

    #[test]
    fn temp_paths_are_unique() {
        let a = temp_png_path();
        let b = temp_png_path();
        assert_ne!(a, b);
        assert_eq!(a.parent(), Some(std::env::temp_dir().as_path()));
    }

    /// End-to-end plumbing check: a real PNG through a real tesseract. Skipped
    /// (with a note) on machines without the binary.
    #[test]
    fn tesseract_runs_on_a_real_png() {
        if tesseract_version_line().is_none() {
            eprintln!("skipping: tesseract not installed");
            return;
        }

        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            32,
            image::Rgba([255, 255, 255, 255]),
        ));
        let mut png = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png);
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode test png");

        let text = run_tesseract(&png, "eng").expect("tesseract should succeed");
        assert_eq!(text, "", "a blank image has no text");
    }
}
