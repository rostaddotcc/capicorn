//! Opening links in the desktop's default browser.
//!
//! Ported from the live app's `upload::open_url` (`src-tauri/src/upload.rs:88`),
//! with the scheme check hardened: **only** `http://` and `https://` ever reach
//! a launcher. `file://`, `javascript:`, `data:`, `mailto:`, `smb://` and every
//! other scheme are rejected before a process is spawned, so a link that arrived
//! from a server response (an upload endpoint's reply) cannot be turned into a
//! local file being opened or a script being run.
//!
//! The launcher is started detached and reaped on a background thread: the UI
//! must never block on a browser starting, and no zombie is left behind.

use std::process::{Command, Stdio};

use crate::IoError;

/// Longest URL we will hand to a launcher (hostile input cap).
pub const MAX_URL_LEN: usize = 2048;

/// Validate a URL for [`open_url`], returning the trimmed form to launch.
///
/// Pure: no process is spawned, which is what makes the security rule testable
/// on a headless build host.
pub fn checked_url(url: &str) -> Result<&str, IoError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(IoError::Io("no link given".to_string()));
    }
    if trimmed.len() > MAX_URL_LEN {
        return Err(IoError::Io(format!(
            "the link is longer than {MAX_URL_LEN} bytes"
        )));
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(IoError::Io(
            "the link contains whitespace or control characters".to_string(),
        ));
    }

    // Exact, lowercase scheme: a browser would happily read `FILE://` too, and
    // case-folding here would only widen what the allowlist lets through.
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(IoError::Io("only http(s) links can be opened".to_string()));
    }

    let Some((_, rest)) = trimmed.split_once("://") else {
        return Err(IoError::Io("only http(s) links can be opened".to_string()));
    };
    if rest.is_empty() || rest.starts_with('/') {
        return Err(IoError::Io(format!("the link has no host: {trimmed}")));
    }

    Ok(trimmed)
}

/// The platform opener for `url`: program plus its arguments.
///
/// Split out from [`open_url`] so the OS matrix is inspectable without ever
/// spawning anything.
fn opener_command(url: &str) -> (&'static str, Vec<&str>) {
    if cfg!(target_os = "windows") {
        // `start` is a cmd builtin; the empty argument is the window title, so
        // a URL containing `&` cannot be read as a second command.
        ("cmd", vec!["/C", "start", "", url])
    } else if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else {
        ("xdg-open", vec![url])
    }
}

/// Open `url` in the desktop's default handler.
///
/// Rejects every non-http(s) scheme, then spawns the platform opener detached
/// (stdio nulled) and returns immediately — the caller does not wait for the
/// browser.
pub fn open_url(url: &str) -> Result<(), IoError> {
    let url = checked_url(url)?;
    let (program, args) = opener_command(url);

    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| IoError::Io(format!("could not open the link with {program}: {err}")))?;

    // Reap on a helper thread: the browser (or the opener handing off to it)
    // outlives this call, and dropping a live `Child` would leave a zombie.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_and_https_are_accepted() {
        assert_eq!(
            checked_url("https://example.com/a.png").expect("https is allowed"),
            "https://example.com/a.png"
        );
        assert_eq!(
            checked_url("  http://localhost:8080/x?y=1#z  ").expect("http is allowed"),
            "http://localhost:8080/x?y=1#z"
        );
    }

    #[test]
    fn local_and_script_schemes_are_rejected() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "mailto:someone@example.com",
            "smb://server/share",
            "ftp://example.com/a.png",
            "chrome://settings",
            "//example.com/a.png",
            "example.com",
            "HTTP://example.com",
            "hTtPs://example.com",
        ] {
            assert!(checked_url(url).is_err(), "{url} must be rejected");
        }
    }

    #[test]
    fn empty_host_and_hostile_whitespace_are_rejected() {
        assert!(checked_url("").is_err());
        assert!(checked_url("   ").is_err());
        assert!(checked_url("https://").is_err());
        assert!(checked_url("http:///path").is_err());
        assert!(checked_url("https://example.com/\nx").is_err());
        // A leading dash or embedded argument must never reach the opener.
        assert!(checked_url("https://example.com --headless").is_err());
        assert!(checked_url("https://example.com\t-x").is_err());
    }

    #[test]
    fn urls_are_bounded() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(checked_url(&long).is_err());
        let just_under = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN - 20));
        assert!(checked_url(&just_under).is_ok());
    }

    #[test]
    fn errors_are_readable() {
        let err = checked_url("file:///tmp/a.png").expect_err("file scheme");
        assert_eq!(
            err.to_string(),
            "io error: only http(s) links can be opened"
        );
    }

    #[test]
    fn opener_matches_the_platform() {
        let (program, args) = opener_command("https://example.com");
        if cfg!(target_os = "windows") {
            assert_eq!(program, "cmd");
            assert_eq!(args, vec!["/C", "start", "", "https://example.com"]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(program, "open");
            assert_eq!(args, vec!["https://example.com"]);
        } else {
            assert_eq!(program, "xdg-open");
            assert_eq!(args, vec!["https://example.com"]);
        }
    }
}
