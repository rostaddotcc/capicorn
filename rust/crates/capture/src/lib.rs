#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L4 — screen capture.
//!
//! Linux goes through the xdg-desktop-portal *Screenshot* interface (Wayland
//! native: works on GNOME/KDE without XWayland shims); macOS/Windows go through
//! `xcap`. Both paths return a [`Surface`], never an encoded blob — the data-URL
//! glue belongs to the app layer, not to the engine.
//!
//! Every portal interaction is bounded: a locked or unresponsive session must
//! surface as [`CaptureError::Timeout`], never as a hung UI thread.

pub mod scrolling;
pub mod timelapse;

/// The scrolling-capture entry points, re-exported for the app layer.
pub use scrolling::{ScrollConfig, ScrollEvent, ScrollHandle, spawn_scrolling};

use std::path::PathBuf;
use std::time::Duration;

use snagcraft_raster::{Surface, dims_ok};
use thiserror::Error;

/// Hard cap on how long the screenshot portal may take before we give up.
pub const PORTAL_TIMEOUT: Duration = Duration::from_secs(15);

/// Anything that can go wrong trying to obtain a screen image.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// The portal (or the platform backend) refused or failed the request.
    #[error("portal: {0}")]
    Portal(String),
    /// The portal did not answer inside [`PORTAL_TIMEOUT`].
    #[error("portal timed out")]
    Timeout,
    /// Local I/O or decoding of the delivered file failed.
    #[error("io: {0}")]
    Io(String),
    /// This platform/build cannot capture at all.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Capture the full primary screen as an RGBA surface.
#[cfg(target_os = "linux")]
pub async fn capture_full() -> Result<Surface, CaptureError> {
    use ashpd::desktop::screenshot::Screenshot;

    let request = Screenshot::request().interactive(false).send();
    let response = tokio::time::timeout(PORTAL_TIMEOUT, request)
        .await
        .map_err(|_| CaptureError::Timeout)?
        .map_err(|err| CaptureError::Portal(err.to_string()))?;

    let shot = response
        .response()
        .map_err(|err| CaptureError::Portal(err.to_string()))?;

    // ashpd 0.14 models the reply URI as an opaque string: no `to_file_path`.
    let uri = shot.uri().as_str();
    let path = file_uri_to_path(uri)?;
    tracing::debug!(path = %path.display(), "portal delivered a screenshot file");

    let bytes = std::fs::read(&path)
        .map_err(|err| CaptureError::Io(format!("reading {}: {err}", path.display())))?;
    decode_screenshot(&bytes)
}

/// Capture the primary screen on macOS/Windows through `xcap`.
#[cfg(not(target_os = "linux"))]
pub async fn capture_full() -> Result<Surface, CaptureError> {
    capture_full_native()
}

#[cfg(not(target_os = "linux"))]
fn capture_full_native() -> Result<Surface, CaptureError> {
    let monitor = xcap::Monitor::from_point(0, 0)
        .map_err(|err| CaptureError::Portal(format!("no monitor at 0,0: {err}")))?;
    let surface = monitor
        .capture_image()
        .map_err(|err| CaptureError::Io(format!("monitor capture failed: {err}")))?;
    let (w, h) = surface.dimensions();
    if !dims_ok(w, h) {
        return Err(CaptureError::Io(format!(
            "captured frame is {w}x{h}, outside the supported range"
        )));
    }
    tracing::debug!(width = w, height = h, "xcap delivered a screen image");
    Ok(surface)
}

/// Blocking wrapper for callers without an async runtime (CLI, egui event loop).
#[cfg(target_os = "linux")]
pub fn capture_full_blocking() -> Result<Surface, CaptureError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| CaptureError::Io(format!("cannot start a tokio runtime: {err}")))?;
    runtime.block_on(capture_full())
}

/// Blocking wrapper; the non-Linux backend is synchronous already, so no
/// runtime is spun up.
#[cfg(not(target_os = "linux"))]
pub fn capture_full_blocking() -> Result<Surface, CaptureError> {
    capture_full_native()
}

/// W25: one connected output, as the in-app monitor picker lists it.
///
/// A plain serde value so no `xcap` type leaks past this crate — the engine
/// serializes it straight to the UI, and kittest seeds it for picker gates.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MonitorInfo {
    /// Stable index into `Monitor::all()` order (the capture selector).
    pub index: usize,
    /// Human label (xcap `friendly_name`, falling back to `name`).
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
    pub scale_factor: f32,
}

/// W25: enumerate connected outputs for the in-app monitor picker.
///
/// Windows/macOS: `xcap::Monitor::all()`. Linux returns an empty list — the
/// desktop portal provides its own screen picker there (a Wayland-native
/// in-app enumeration needs output-management protocols; that is a later
/// sub-wave). An empty list tells the UI "use the system picker".
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    #[cfg(target_os = "linux")]
    {
        Ok(Vec::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let monitors = xcap::Monitor::all()
            .map_err(|err| CaptureError::Io(format!("cannot enumerate monitors: {err}")))?;
        let mut out = Vec::with_capacity(monitors.len());
        for (index, m) in monitors.iter().enumerate() {
            let name = m
                .friendly_name()
                .or_else(|_| m.name())
                .unwrap_or_else(|_| format!("Monitor {}", index + 1));
            out.push(MonitorInfo {
                index,
                name,
                x: m.x().unwrap_or(0),
                y: m.y().unwrap_or(0),
                width: m.width().unwrap_or(0),
                height: m.height().unwrap_or(0),
                is_primary: m.is_primary().unwrap_or(false),
                scale_factor: m.scale_factor().unwrap_or(1.0),
            });
        }
        Ok(out)
    }
}

/// W25: capture one specific output by its `list_monitors` index.
///
/// Windows/macOS: `xcap` per-monitor capture (this is the issue #1 fix — the
/// user picks the screen in-app where no OS picker exists). Linux: the portal
/// picker is the selector, so a direct index is unsupported here.
pub fn capture_monitor_index(index: usize) -> Result<Surface, CaptureError> {
    #[cfg(target_os = "linux")]
    {
        let _ = index;
        Err(CaptureError::Unsupported(
            "monitor selection on Linux uses the system portal picker".into(),
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let monitors = xcap::Monitor::all()
            .map_err(|err| CaptureError::Io(format!("cannot enumerate monitors: {err}")))?;
        let monitor = monitors
            .get(index)
            .ok_or_else(|| CaptureError::Io(format!("monitor index {index} is out of range")))?;
        let surface = monitor
            .capture_image()
            .map_err(|err| CaptureError::Io(format!("monitor capture failed: {err}")))?;
        let (w, h) = surface.dimensions();
        if !dims_ok(w, h) {
            return Err(CaptureError::Io(format!(
                "captured frame is {w}x{h}, outside the supported range"
            )));
        }
        Ok(surface)
    }
}

/// Decode PNG/JPEG/WebP bytes into a validated RGBA surface.
///
/// Split out from [`capture_full`] so the decode + cap rules are testable
/// without a portal, and reused for any file the portal hands back.
///
/// Linux-only in production: `xcap` hands back a decoded `RgbaImage` already,
/// so keeping this compiled there would only be dead code.
#[cfg(any(target_os = "linux", test))]
fn decode_screenshot(bytes: &[u8]) -> Result<Surface, CaptureError> {
    let image = image::load_from_memory(bytes)
        .map_err(|err| CaptureError::Io(format!("screenshot is not a decodable image: {err}")))?;
    let (width, height) = (image.width(), image.height());
    if !dims_ok(width, height) {
        return Err(CaptureError::Io(format!(
            "captured image is {width}x{height}, outside the supported range"
        )));
    }
    Ok(image.to_rgba8())
}

/// Convert the portal's `file://` URI into a local path.
///
/// ashpd 0.14 models URIs as opaque strings, so decoding happens here: scheme,
/// optional `localhost` authority, query/fragment, then percent-escapes into
/// raw OS bytes (screenshot paths may legitimately be non-UTF-8).
pub fn file_uri_to_path(uri: &str) -> Result<PathBuf, CaptureError> {
    let rest = uri.strip_prefix("file://").ok_or_else(|| {
        CaptureError::Portal(format!("screenshot portal returned a non-file uri: {uri}"))
    })?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    if !rest.starts_with('/') {
        return Err(CaptureError::Portal(format!(
            "file uri without an absolute path: {uri}"
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(percent_decode(
            rest,
        ))))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(rest.to_string()))
    }
}

/// Percent-decode into raw bytes. Invalid escapes (`%zz`, truncated `%4`) are
/// kept literally: a hostile URI must produce a harmless path, not a panic.
#[cfg(unix)]
fn percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png(width: u32, height: u32) -> Vec<u8> {
        let surface = Surface::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut out);
        image::DynamicImage::ImageRgba8(surface)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode test png");
        out
    }

    #[test]
    #[cfg(unix)]
    fn file_uri_becomes_a_path() {
        assert_eq!(
            file_uri_to_path("file:///tmp/Screenshot%20from%202026.png").expect("percent space"),
            PathBuf::from("/tmp/Screenshot from 2026.png")
        );
        assert_eq!(
            file_uri_to_path("file://localhost/tmp/a.png").expect("localhost authority"),
            PathBuf::from("/tmp/a.png")
        );
        assert!(file_uri_to_path("https://example.com/a.png").is_err());
        assert!(file_uri_to_path("file://relative.png").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn file_uri_ignores_query_and_fragment() {
        assert_eq!(
            file_uri_to_path("file:///tmp/shot.png?cache=1#top").expect("query and fragment"),
            PathBuf::from("/tmp/shot.png")
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalid_escapes_stay_literal() {
        assert_eq!(percent_decode("%41"), b"A");
        assert_eq!(percent_decode("%zz%2"), b"%zz%2");
        // A trailing lone '%' cannot form an escape and must survive verbatim.
        assert_eq!(percent_decode("/tmp/100%"), b"/tmp/100%");
    }

    #[cfg(unix)]
    #[test]
    fn percent_decode_handles_non_utf8_bytes() {
        // 0xFF alone is not valid UTF-8; the raw byte must still reach the path.
        assert_eq!(percent_decode("/tmp/%FF.png"), b"/tmp/\xff.png");
        let path = file_uri_to_path("file:///tmp/%FF.png").expect("raw byte path");
        assert_eq!(
            path.as_os_str().as_encoded_bytes(),
            b"/tmp/\xff.png".as_slice()
        );
    }

    #[test]
    fn decoded_screenshots_are_rgba_and_capped() {
        let surface = decode_screenshot(&tiny_png(5, 3)).expect("decode tiny png");
        assert_eq!(surface.dimensions(), (5, 3));
        assert_eq!(surface.get_pixel(0, 0).0, [10, 20, 30, 255]);

        assert!(decode_screenshot(b"definitely not an image").is_err());
    }

    #[test]
    fn capture_errors_display_cleanly() {
        assert_eq!(
            CaptureError::Portal("denied".into()).to_string(),
            "portal: denied"
        );
        assert_eq!(CaptureError::Timeout.to_string(), "portal timed out");
        assert_eq!(
            CaptureError::Io("no such file".into()).to_string(),
            "io: no such file"
        );
        assert_eq!(
            CaptureError::Unsupported("wayland only".into()).to_string(),
            "unsupported: wayland only"
        );
    }

    /// Live portal round trip. Ignored by default: it needs an unlocked,
    /// interactive desktop session (the CI/build host runs with a locked
    /// screen, where the portal prompt can never be answered).
    ///
    /// Run it from inside the session:
    /// `cargo test -p snagcraft-capture -- --ignored`
    /// (add `XDG_SESSION_TYPE=wayland` / a valid `DBUS_SESSION_BUS_ADDRESS`
    /// if the test process did not inherit the session bus).
    #[cfg(target_os = "linux")]
    #[ignore = "needs an unlocked desktop session with a live xdg-desktop-portal"]
    #[tokio::test]
    async fn live_portal_capture() {
        let surface = capture_full().await.expect("live portal capture");
        assert!(surface.width() > 0 && surface.height() > 0);
        assert!(dims_ok(surface.width(), surface.height()));
    }
}
