//! XDG autostart (`~/.config/autostart/snagcraft.desktop`).
//!
//! `XDG_CONFIG_HOME` wins when set (that is what the spec and every desktop
//! honours); otherwise `~/.config`. `X-GNOME-Autostart-enabled=true` is what
//! GNOME reads to actually enable the entry.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::PlatformError;

const FILE_NAME: &str = "snagcraft.desktop";

fn desktop_entry() -> String {
    // Exec is the bare binary name: the packaging step puts it on PATH, and a
    // hard-coded absolute path would break for a per-user install.
    "[Desktop Entry]\n\
     Type=Application\n\
     Name=SnagCraft\n\
     Comment=Screenshot annotations\n\
     Exec=snagcraft\n\
     Terminal=false\n\
     Categories=Graphics;Utility;\n\
     X-GNOME-Autostart-enabled=true\n"
        .to_string()
}

/// `$XDG_CONFIG_HOME` (absolute, non-empty) or `$HOME/.config`.
///
/// Pure so it can be tested: `set_var` is `unsafe` in edition 2024 and this
/// workspace forbids `unsafe_code`, so the environment is read once here and
/// never mutated.
fn resolve_config_home(
    xdg: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, PlatformError> {
    if let Some(dir) = xdg {
        let path = PathBuf::from(dir);
        if !dir.is_empty() && path.is_absolute() {
            return Ok(path);
        }
    }
    match home {
        Some(home) if !home.is_empty() => Ok(PathBuf::from(home).join(".config")),
        _ => Err(PlatformError::Io(
            "neither XDG_CONFIG_HOME nor HOME is set".to_string(),
        )),
    }
}

fn config_home() -> Result<PathBuf, PlatformError> {
    resolve_config_home(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn autostart_path_in(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join(FILE_NAME)
}

fn autostart_enabled_in(config_home: &Path) -> bool {
    autostart_path_in(config_home).is_file()
}

fn set_autostart_in(config_home: &Path, enabled: bool) -> Result<(), PlatformError> {
    let path = autostart_path_in(config_home);
    if enabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, desktop_entry())?;
        tracing::info!(path = %path.display(), "autostart entry written");
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!(path = %path.display(), "autostart entry removed"),
            // Already absent: disabling twice is not an error.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(PlatformError::Io(err.to_string())),
        }
    }
    Ok(())
}

/// Enable or disable launch-at-login. Creates `~/.config/autostart` as needed;
/// disabling an entry that is not there succeeds.
pub fn set_autostart(enabled: bool) -> Result<(), PlatformError> {
    set_autostart_in(&config_home()?, enabled)
}

/// Whether a launch-at-login entry exists. A missing environment reads as `false`.
pub fn autostart_enabled() -> bool {
    match config_home() {
        Ok(dir) => autostart_enabled_in(&dir),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "snagcraft-platform-autostart-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn set_read_and_remove_round_trip() {
        let cfg = temp_config("round");
        assert!(!autostart_enabled_in(&cfg));
        set_autostart_in(&cfg, true).expect("enable");
        assert!(autostart_enabled_in(&cfg));
        let body = std::fs::read_to_string(autostart_path_in(&cfg)).expect("read entry");
        assert!(body.starts_with("[Desktop Entry]\n"));
        assert!(body.contains("Type=Application"));
        assert!(body.contains("Exec=snagcraft"));
        assert!(body.contains("X-GNOME-Autostart-enabled=true"));
        set_autostart_in(&cfg, false).expect("disable");
        assert!(!autostart_enabled_in(&cfg));
        // Disabling a second time is still Ok.
        set_autostart_in(&cfg, false).expect("disable twice");
        let _ = std::fs::remove_dir_all(&cfg);
    }

    #[test]
    fn enable_creates_the_autostart_directory() {
        let cfg = temp_config("mkdir");
        assert!(!cfg.join("autostart").exists());
        set_autostart_in(&cfg, true).expect("enable");
        assert!(cfg.join("autostart").is_dir());
        assert_eq!(
            autostart_path_in(&cfg),
            cfg.join("autostart").join("snagcraft.desktop")
        );
        let _ = std::fs::remove_dir_all(&cfg);
    }

    #[test]
    #[cfg(unix)]
    fn xdg_config_home_wins_over_home() {
        assert_eq!(
            resolve_config_home(Some(OsStr::new("/tmp/xdg")), Some(OsStr::new("/home/u")))
                .expect("absolute XDG_CONFIG_HOME"),
            PathBuf::from("/tmp/xdg")
        );
        assert_eq!(
            resolve_config_home(None, Some(OsStr::new("/home/u"))).expect("HOME fallback"),
            PathBuf::from("/home/u/.config")
        );
        // An empty or relative XDG_CONFIG_HOME is ignored, per the spec.
        assert_eq!(
            resolve_config_home(Some(OsStr::new("")), Some(OsStr::new("/home/u")))
                .expect("empty is ignored"),
            PathBuf::from("/home/u/.config")
        );
        assert_eq!(
            resolve_config_home(
                Some(OsStr::new("relative/path")),
                Some(OsStr::new("/home/u"))
            )
            .expect("relative is ignored"),
            PathBuf::from("/home/u/.config")
        );
        assert!(resolve_config_home(None, None).is_err());
    }

    #[test]
    fn the_live_paths_read_the_real_environment() {
        // Whatever the environment is, these must not panic and must agree.
        match config_home() {
            Ok(dir) => {
                assert_eq!(autostart_enabled(), autostart_enabled_in(&dir));
            }
            Err(_) => assert!(!autostart_enabled()),
        }
    }
}
