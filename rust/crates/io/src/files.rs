//! L4 save-location logic: file-name pattern expansion and the default save
//! folder.
//!
//! Parity: `src/ui/settings.ts:299-323` (`expandPattern` / `buildFilename`).
//! The live app expands `{date}`, `{time}` and `{datetime}` case-insensitively,
//! replaces every character that is illegal in a Windows path with `-`, squashes
//! whitespace runs, and appends the format's extension. An empty pattern falls
//! back to [`DEFAULT_PATTERN`].
//!
//! Hostile input: the pattern is capped at [`MAX_PATTERN_CHARS`] characters
//! before expansion and the resulting stem at [`MAX_STEM_CHARS`], so a corrupt
//! settings file can never hand the filesystem a 10 MB name. Expansion only ever
//! *removes* separators (`\` and `/` become `-`), so a token value cannot inject
//! a path.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

use crate::ImageFormat;

/// The pattern used when the setting is blank (live default).
pub const DEFAULT_PATTERN: &str = "SnagCraft-{date}-{time}";

/// Longest pattern we expand, in characters.
pub const MAX_PATTERN_CHARS: usize = 200;

/// Longest expanded stem we hand to the filesystem, in characters.
pub const MAX_STEM_CHARS: usize = 120;

/// Name of the folder under the OS pictures directory.
pub const SAVE_DIR_NAME: &str = "SnagCraft";

/// Expand `pattern` for `now` and append `format`'s extension.
///
/// Unknown `{tokens}` stay literal; an empty/whitespace-only pattern becomes
/// [`DEFAULT_PATTERN`]; a stem that sanitises down to nothing becomes
/// `SnagCraft-<date>-<clock>`.
pub fn build_filename(pattern: &str, format: ImageFormat, now: DateTime<Local>) -> String {
    let stem = expand(pattern, now);
    format!("{stem}.{}", format.extension())
}

/// [`build_filename`] with the local clock.
pub fn build_filename_now(pattern: &str, format: ImageFormat) -> String {
    build_filename(pattern, format, Local::now())
}

/// Expand the pattern without an extension (the live `expandPattern`).
pub fn expand(pattern: &str, now: DateTime<Local>) -> String {
    let trimmed = pattern.trim();
    let source: String = if trimmed.is_empty() {
        DEFAULT_PATTERN.to_string()
    } else {
        trimmed.chars().take(MAX_PATTERN_CHARS).collect()
    };

    let day = now.format("%Y-%m-%d").to_string();
    let clock = now.format("%H-%M-%S").to_string();
    let stamp_day = day.clone();
    let stamp_clock = clock.clone();

    let expanded = expand_tokens(&source, &day, &clock, &format!("{stamp_day}_{stamp_clock}"));
    let clean = sanitize(&expanded);
    let stem: String = clean.chars().take(MAX_STEM_CHARS).collect();
    let stem = stem.trim_end_matches(['.', ' ']).to_string();
    if stem.is_empty() {
        format!("SnagCraft-{day}-{clock}")
    } else {
        stem
    }
}

/// One pass over `src`, replacing `{date}` / `{time}` / `{datetime}`
/// (case-insensitive). Anything else inside braces is copied through verbatim.
fn expand_tokens(src: &str, date: &str, time: &str, datetime: &str) -> String {
    let mut out = String::with_capacity(src.len() + 32);
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Tokens are short; cap the look-ahead so a stray '{' cannot make
            // this quadratic over a hostile pattern.
            let window_end = (i + 32).min(bytes.len());
            if let Some(close) = src[i..window_end].find('}') {
                let token = &src[i + 1..i + close];
                let replacement = match token.to_ascii_lowercase().as_str() {
                    "date" => Some(date),
                    "time" => Some(time),
                    "datetime" => Some(datetime),
                    _ => None,
                };
                if let Some(value) = replacement {
                    out.push_str(value);
                    i += close + 1;
                    continue;
                }
            }
        }
        let Some(ch) = src[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Filesystem-safe stem: illegal characters become `-`, whitespace runs one
/// space, ends trimmed. Mirrors the live regex chain.
fn sanitize(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut last_was_space = false;
    for ch in src.chars() {
        let ch = if ch.is_control()
            || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            '-'
        } else {
            ch
        };
        if ch == ' ' || ch == '\t' {
            if last_was_space {
                continue;
            }
            out.push(' ');
            last_was_space = true;
            continue;
        }
        last_was_space = false;
        out.push(ch);
    }
    out.trim().to_string()
}

/// The OS default save folder: `$HOME/Pictures/SnagCraft`.
///
/// Deliberately no `dirs` crate (no new workspace dependency): the W9 contract
/// fixes the HOME/Pictures fallback. Without a HOME the relative `SnagCraft`
/// folder keeps the app usable inside a sandbox.
pub fn default_save_dir() -> PathBuf {
    default_save_dir_from(std::env::var_os("HOME").as_deref())
}

/// Pure resolver behind [`default_save_dir`] (testable without touching env).
pub fn default_save_dir_from(home: Option<&std::ffi::OsStr>) -> PathBuf {
    home.filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join("Pictures").join(SAVE_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(SAVE_DIR_NAME))
}

/// `configured` when it names a folder, else [`default_save_dir`]. An empty or
/// whitespace-only setting is treated as unset (live `getSaveDir` behaviour).
pub fn resolve_save_dir(configured: Option<&str>) -> PathBuf {
    match configured.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => default_save_dir(),
    }
}

/// True for a folder we are willing to scan or write into: non-empty and not a
/// filesystem root (a scan of `/` is never what the user meant, and the recents
/// strip must stay bounded).
pub fn is_usable_dir(dir: &Path) -> bool {
    !dir.as_os_str().is_empty() && dir.parent().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn stamp() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 17, 14, 5, 9)
            .single()
            .expect("valid local timestamp")
    }

    #[test]
    fn expands_each_token() {
        let now = stamp();
        assert_eq!(
            build_filename("{date}", ImageFormat::Png, now),
            "2026-11-17.png"
        );
        assert_eq!(
            build_filename("{time}", ImageFormat::Png, now),
            "14-05-09.png"
        );
        assert_eq!(
            build_filename("{datetime}", ImageFormat::Png, now),
            "2026-11-17_14-05-09.png"
        );
        assert_eq!(
            build_filename("shot-{date}-{time}-{datetime}", ImageFormat::Png, now),
            "shot-2026-11-17-14-05-09-2026-11-17_14-05-09.png"
        );
    }

    #[test]
    fn tokens_are_case_insensitive() {
        let now = stamp();
        assert_eq!(
            build_filename("{DATE}_{Time}", ImageFormat::Png, now),
            "2026-11-17_14-05-09.png"
        );
    }

    #[test]
    fn extension_matches_each_format() {
        let now = stamp();
        assert!(build_filename("a", ImageFormat::Png, now).ends_with(".png"));
        assert!(build_filename("a", ImageFormat::Jpeg, now).ends_with(".jpg"));
        assert!(build_filename("a", ImageFormat::WebP, now).ends_with(".webp"));
    }

    #[test]
    fn unknown_tokens_stay_literal() {
        let now = stamp();
        assert_eq!(
            build_filename("shot-{nope}-{date}", ImageFormat::Png, now),
            "shot-{nope}-2026-11-17.png"
        );
        // An unclosed brace is not a token.
        assert_eq!(build_filename("a{b", ImageFormat::Png, now), "a{b.png");
    }

    #[test]
    fn empty_or_blank_pattern_uses_the_default() {
        let now = stamp();
        let want = "SnagCraft-2026-11-17-14-05-09.png";
        assert_eq!(build_filename("", ImageFormat::Png, now), want);
        assert_eq!(build_filename("   ", ImageFormat::Png, now), want);
        assert_eq!(build_filename(DEFAULT_PATTERN, ImageFormat::Png, now), want);
    }

    #[test]
    fn separators_and_control_chars_are_neutralised() {
        let now = stamp();
        // Path traversal, Windows-illegal characters and a NUL cannot survive.
        let name = build_filename("../../etc/passwd:{a}|b?c\"d<e>", ImageFormat::Png, now);
        assert!(!name.contains('/'), "{name}");
        assert!(!name.contains('\\'), "{name}");
        assert!(!name.contains(':'), "{name}");
        assert!(!name.contains('|'), "{name}");
        assert!(!name.contains('"'), "{name}");
        assert!(!name.contains('<') && !name.contains('>'), "{name}");
        assert!(!name.contains('\0'), "{name}");
        assert_eq!(name, "..-..-etc-passwd-{a}-b-c-d-e-.png");
        assert!(!name.contains("  "));
    }

    #[test]
    fn a_pattern_of_only_illegal_characters_still_yields_a_name() {
        let now = stamp();
        // Everything sanitises away or to dashes, so the caller never gets "".
        let name = build_filename("\u{1}\u{2}", ImageFormat::Png, now);
        assert!(name.ends_with(".png"));
        assert!(name.len() > 4);
    }

    #[test]
    fn hostile_lengths_are_capped() {
        let now = stamp();
        let long = "x".repeat(10_000);
        let name = build_filename(&long, ImageFormat::Png, now);
        assert!(
            name.chars().count() <= MAX_STEM_CHARS + 5,
            "stem not capped: {} chars",
            name.chars().count()
        );
        let tokens = "{datetime}".repeat(1_000);
        let name = build_filename(&tokens, ImageFormat::Png, now);
        assert!(name.chars().count() <= MAX_STEM_CHARS + 5);
    }

    #[test]
    fn default_dir_is_pictures_snagcraft() {
        let home = std::ffi::OsStr::new("/home/tester");
        assert_eq!(
            default_save_dir_from(Some(home)),
            PathBuf::from("/home/tester/Pictures/SnagCraft")
        );
        assert_eq!(
            default_save_dir_from(None),
            PathBuf::from("SnagCraft"),
            "no HOME must not panic or produce an absolute path"
        );
        assert_eq!(
            default_save_dir_from(Some(std::ffi::OsStr::new(""))),
            PathBuf::from("SnagCraft")
        );
    }

    #[test]
    fn resolve_save_dir_prefers_a_configured_folder() {
        assert_eq!(
            resolve_save_dir(Some("/tmp/shots")),
            PathBuf::from("/tmp/shots")
        );
        // Blank entries mean "unset", never a scan/write into the cwd.
        let fallback = resolve_save_dir(Some("   "));
        assert!(fallback.ends_with("SnagCraft"));
        assert_eq!(resolve_save_dir(None), default_save_dir());
        assert!(is_usable_dir(&fallback));
        assert!(!is_usable_dir(Path::new("/")));
    }
}
