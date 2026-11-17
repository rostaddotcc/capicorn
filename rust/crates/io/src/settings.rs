//! L4 settings persistence: the user-facing preference block, a corrupt-safe
//! JSON loader and an atomic saver.
//!
//! Parity: the live frontend keeps these in `localStorage` (`src/ui/settings.ts`,
//! defaults at `:237`); the native build owns a real file so the app can read
//! preferences before any window exists.
//!
//! Hostile input: the file read is capped at [`MAX_SETTINGS_BYTES`] and every
//! failure path (missing file, unreadable, corrupt JSON, wrong field types) falls
//! back to [`Settings::default`] — [`load`] never panics and never returns `Err`.
//! Writes validate the two fields the encoder cares about before touching disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::IoError;

/// Hostile-input cap on the settings file we are willing to read (1 MB).
pub const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// Container formats the export path understands (`png`, `jpeg`, `webp`).
pub const EXPORT_FORMATS: [&str; 3] = ["png", "jpeg", "webp"];

/// UI theme. Persisted verbatim as `Light` / `Dark`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Light,
    Dark,
}

/// Everything the app remembers between launches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub minimize_to_tray: bool,
    pub start_at_login: bool,
    pub show_widget: bool,
    pub autosave_captures: bool,
    /// Delay between the hotkey firing and the capture (0 = immediate).
    pub capture_delay_secs: u32,
    pub theme: Theme,
    /// `png` | `jpeg` | `webp`.
    pub default_format: String,
    /// 1..=100 (JPEG/WebP only; PNG ignores it).
    pub default_quality: u8,
    /// `None` → the app resolves Pictures/SnagCraft.
    pub save_dir: Option<String>,
    pub filename_pattern: String,
    pub hotkey_full: String,
    pub hotkey_region: String,
    /// Export watermark text (empty = no watermark). W4d.
    pub watermark_text: String,
    /// Watermark opacity percent, 2..=100 (live default 45). W4d.
    pub watermark_opacity: u8,
    /// `bottom-right` | `bottom-left` | `top-right` | `top-left`. W4d.
    pub watermark_corner: String,
    /// Upload page URL (http/https only; empty = feature disabled). W4d.
    pub upload_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            minimize_to_tray: false,
            start_at_login: false,
            show_widget: true,
            autosave_captures: true,
            capture_delay_secs: 0,
            theme: Theme::Light,
            default_format: "png".to_string(),
            default_quality: 90,
            save_dir: None,
            filename_pattern: "SnagCraft-{date}-{time}".to_string(),
            hotkey_full: "Print".to_string(),
            hotkey_region: "SHIFT+Print".to_string(),
            watermark_text: String::new(),
            watermark_opacity: 45,
            watermark_corner: "bottom-right".to_string(),
            upload_url: String::new(),
        }
    }
}

impl Settings {
    /// Reject values the export pipeline cannot honour. Callers validate before
    /// persisting and before applying a settings update.
    pub fn validate(&self) -> Result<(), IoError> {
        if !(1..=100).contains(&self.default_quality) {
            return Err(IoError::Unsupported);
        }
        if !is_supported_format(&self.default_format) {
            return Err(IoError::Unsupported);
        }
        if !(2..=100).contains(&self.watermark_opacity) {
            return Err(IoError::Unsupported);
        }
        if !matches!(
            self.watermark_corner.as_str(),
            "bottom-right" | "bottom-left" | "top-right" | "top-left"
        ) {
            return Err(IoError::Unsupported);
        }
        if !self.upload_url.is_empty()
            && !(self.upload_url.starts_with("http://") || self.upload_url.starts_with("https://"))
        {
            return Err(IoError::Unsupported);
        }
        Ok(())
    }
}

/// True for the format names [`Settings::default_format`] accepts.
pub fn is_supported_format(format: &str) -> bool {
    EXPORT_FORMATS.contains(&format)
}

/// `$XDG_CONFIG_HOME/snagcraft/settings.json`, else `$HOME/.config/snagcraft/settings.json`.
pub fn path() -> PathBuf {
    path_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// Pure resolver behind [`path`] (kept testable — `env::set_var` is `unsafe`
/// under edition 2024 and this workspace forbids unsafe code outright).
fn path_from(xdg_config_home: Option<&std::ffi::OsStr>, home: Option<&std::ffi::OsStr>) -> PathBuf {
    let base = xdg_config_home
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("snagcraft").join("settings.json")
}

/// Read the persisted settings, falling back to [`Settings::default`] on any
/// problem (missing file, oversized file, corrupt JSON, wrong field types).
pub fn load() -> Settings {
    load_from(&path())
}

/// [`load`] against an explicit path.
pub fn load_from(file: &Path) -> Settings {
    let Ok(meta) = std::fs::metadata(file) else {
        return Settings::default();
    };
    if !meta.is_file() || meta.len() > MAX_SETTINGS_BYTES {
        return Settings::default();
    }
    let Ok(bytes) = std::fs::read(file) else {
        return Settings::default();
    };
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Settings::default();
    }
    serde_json::from_slice::<Settings>(&bytes).unwrap_or_default()
}

/// Persist `s` to [`path`], creating the parent directory first.
///
/// Rejects an invalid quality/format with [`IoError::Unsupported`] *before*
/// writing, and replaces the file atomically (`.tmp` + rename) so a crash can
/// never leave a half-written settings file behind.
pub fn save(s: &Settings) -> Result<(), IoError> {
    save_to(&path(), s)
}

/// [`save`] against an explicit path.
pub fn save_to(file: &Path, s: &Settings) -> Result<(), IoError> {
    s.validate()?;
    let parent = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| IoError::Io(format!("{}: no parent directory", file.display())))?;
    std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;

    let bytes =
        serde_json::to_vec_pretty(s).map_err(|e| IoError::Encode(format!("settings: {e}")))?;

    let mut tmp_name = file.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    // Clear a stale temp file from an interrupted earlier write.
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::File::create(&tmp).map_err(|e| io_err(&tmp, e))?;
    use std::io::Write;
    f.write_all(&bytes).map_err(|e| io_err(&tmp, e))?;
    f.sync_all().map_err(|e| io_err(&tmp, e))?;
    drop(f);
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_err(file, e)
    })
}

fn io_err(p: &Path, e: std::io::Error) -> IoError {
    IoError::Io(format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Fresh per-test directory (no global env mutation — tests run in parallel).
    fn tmp_dir(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "snagcraft-settings-{}-{n}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn custom() -> Settings {
        Settings {
            minimize_to_tray: true,
            start_at_login: true,
            show_widget: false,
            autosave_captures: false,
            capture_delay_secs: 3,
            theme: Theme::Dark,
            default_format: "webp".to_string(),
            default_quality: 42,
            save_dir: Some("/tmp/shots".to_string()),
            filename_pattern: "shot-{datetime}".to_string(),
            hotkey_full: "CTRL+Print".to_string(),
            hotkey_region: "CTRL+SHIFT+Print".to_string(),
            watermark_text: "\u{a9} Rostad".to_string(),
            watermark_opacity: 70,
            watermark_corner: "top-left".to_string(),
            upload_url: "https://example.com/upload".to_string(),
        }
    }

    #[test]
    fn watermark_upload_defaults_and_validation() {
        let d = Settings::default();
        assert_eq!(d.watermark_text, "");
        assert_eq!(d.watermark_opacity, 45);
        assert_eq!(d.watermark_corner, "bottom-right");
        assert_eq!(d.upload_url, "");
        assert!(d.validate().is_ok());
        let mut bad = d.clone();
        bad.watermark_opacity = 1;
        assert!(bad.validate().is_err());
        bad = d.clone();
        bad.watermark_corner = "middle".to_string();
        assert!(bad.validate().is_err());
        bad = d.clone();
        bad.upload_url = "file:///etc".to_string();
        assert!(bad.validate().is_err());
        bad = d.clone();
        bad.upload_url = "https://ok.example/u".to_string();
        assert!(bad.validate().is_ok());
        // round-trip carries the new fields
        let c = custom();
        let json = serde_json::to_string(&c).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        // old files without the new fields load with defaults
        let old_json = r#"{"default_format":"png","default_quality":90}"#;
        let old: Settings = serde_json::from_str(old_json).unwrap();
        assert_eq!(old.watermark_opacity, 45);
        assert_eq!(old.watermark_corner, "bottom-right");
    }

    #[test]
    fn defaults_match_parity() {
        let d = Settings::default();
        assert!(!d.minimize_to_tray);
        assert!(!d.start_at_login);
        assert!(d.show_widget);
        assert!(d.autosave_captures);
        assert_eq!(d.capture_delay_secs, 0);
        assert_eq!(d.theme, Theme::Light);
        assert_eq!(d.default_format, "png");
        assert_eq!(d.default_quality, 90);
        assert_eq!(d.save_dir, None);
        assert_eq!(d.filename_pattern, "SnagCraft-{date}-{time}");
        assert_eq!(d.hotkey_full, "Print");
        assert_eq!(d.hotkey_region, "SHIFT+Print");
        d.validate().unwrap();
    }

    #[test]
    fn serde_round_trip() {
        let s = custom();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        // Partial JSON fills missing fields from Default.
        let partial: Settings = serde_json::from_str(r#"{"default_quality": 75}"#).unwrap();
        assert_eq!(partial.default_quality, 75);
        assert_eq!(partial.default_format, "png");
        assert_eq!(partial.theme, Theme::Light);
        assert!(partial.show_widget);
    }

    #[test]
    fn path_respects_xdg_config_home_and_home_fallback() {
        let xdg = std::ffi::OsStr::new("/tmp/xdg-test");
        let home = std::ffi::OsStr::new("/home/tester");
        assert_eq!(
            path_from(Some(xdg), Some(home)),
            PathBuf::from("/tmp/xdg-test/snagcraft/settings.json")
        );
        assert_eq!(
            path_from(None, Some(home)),
            PathBuf::from("/home/tester/.config/snagcraft/settings.json")
        );
        // Empty XDG_CONFIG_HOME is not a directory: fall through to HOME.
        assert_eq!(
            path_from(Some(std::ffi::OsStr::new("")), Some(home)),
            PathBuf::from("/home/tester/.config/snagcraft/settings.json")
        );
        assert!(path().ends_with("snagcraft/settings.json"));
    }

    #[test]
    fn load_missing_file_gives_defaults() {
        let dir = tmp_dir("missing");
        let got = load_from(&dir.join("settings.json"));
        assert_eq!(got, Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_corrupt_json_gives_defaults() {
        let dir = tmp_dir("corrupt");
        let file = dir.join("settings.json");
        std::fs::write(&file, b"{ this is not json at all ").unwrap();
        assert_eq!(load_from(&file), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();

        // Wrong types also fall back instead of panicking.
        let dir = tmp_dir("types");
        let file = dir.join("settings.json");
        std::fs::write(&file, br#"{"default_quality": "high", "theme": 7}"#).unwrap();
        assert_eq!(load_from(&file), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_oversized_file_gives_defaults() {
        let dir = tmp_dir("huge");
        let file = dir.join("settings.json");
        std::fs::write(&file, vec![b' '; MAX_SETTINGS_BYTES as usize + 1]).unwrap();
        assert_eq!(load_from(&file), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_keys_are_ignored_on_load() {
        let dir = tmp_dir("unknown");
        let file = dir.join("settings.json");
        std::fs::write(
            &file,
            br#"{"default_quality": 61, "future_field": [1,2,3]}"#,
        )
        .unwrap();
        let got = load_from(&file);
        assert_eq!(got.default_quality, 61);
        assert_eq!(got.hotkey_region, "SHIFT+Print");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_is_atomic_and_reloads_equal() {
        let dir = tmp_dir("save");
        let file = dir.join("nested").join("settings.json");
        let s = custom();
        save_to(&file, &s).unwrap();
        // Parent dirs created, no temp file left behind.
        assert!(file.is_file());
        let mut tmp_name = file.as_os_str().to_os_string();
        tmp_name.push(".tmp");
        assert!(
            !Path::new(&tmp_name).exists(),
            "tmp file must be renamed away"
        );
        assert_eq!(load_from(&file), s);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_rejects_bad_quality_and_format_before_writing() {
        let dir = tmp_dir("reject");
        let file = dir.join("settings.json");

        for q in [0u8, 255] {
            let s = Settings {
                default_quality: q,
                ..Settings::default()
            };
            assert!(matches!(save_to(&file, &s), Err(IoError::Unsupported)));
            assert!(!file.exists(), "nothing written for quality {q}");
        }
        let s = Settings {
            default_format: "tiff".to_string(),
            ..Settings::default()
        };
        assert!(matches!(save_to(&file, &s), Err(IoError::Unsupported)));
        assert!(!file.exists());

        // A rejected save leaves an existing file untouched.
        save_to(&file, &Settings::default()).unwrap();
        let bad = Settings {
            default_quality: 0,
            ..Settings::default()
        };
        assert!(save_to(&file, &bad).is_err());
        assert_eq!(load_from(&file), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_accepts_boundary_values() {
        let dir = tmp_dir("bounds");
        let file = dir.join("settings.json");
        for (q, fmt) in [(1u8, "png"), (100u8, "jpeg"), (50u8, "webp")] {
            let s = Settings {
                default_quality: q,
                default_format: fmt.to_string(),
                ..Settings::default()
            };
            save_to(&file, &s).unwrap();
            assert_eq!(load_from(&file), s);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
