//! Screen capture backends.
//! Linux: xdg-desktop-portal Screenshot via ashpd (Wayland-native, works on
//! GNOME/KDE Wayland without XWayland portal shims).
//! macOS/Windows: xcap.
//!
//! `capture_full` returns a PNG data URL plus the image dimensions; the same
//! data-URL helpers are reused by `clipboard` and `commands` so the whole
//! pipeline speaks exactly one format.

/// Prefix of every data URL this app produces/consumes.
pub const PNG_DATA_URL_PREFIX: &str = "data:image/png;base64,";

/// Capture the full primary screen. Returns (png_data_url, width, height).
#[cfg(target_os = "linux")]
pub async fn capture_full() -> anyhow::Result<(String, u32, u32)> {
    use ashpd::desktop::screenshot::Screenshot;

    let resp = Screenshot::request().interactive(false).send().await?;
    let shot = resp.response()?;
    // ashpd 0.14 exposes the portal's reply as a string-backed `Uri` (no
    // `to_file_path`), so the conversion lives here.
    let path = file_uri_to_path(shot.uri().as_str())?;
    let bytes = std::fs::read(&path)?;
    png_bytes_to_data_url(&bytes)
}

/// Capture the primary screen on macOS/Windows through xcap.
#[cfg(not(target_os = "linux"))]
pub async fn capture_full() -> anyhow::Result<(String, u32, u32)> {
    let monitor = xcap::Monitor::from_point(0, 0)?;
    let rgba = monitor.capture_image()?;
    let mut png = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png);
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| anyhow::anyhow!("failed to encode screenshot as PNG: {e}"))?;
    png_bytes_to_data_url(&png)
}

/// One user-picked window, grabbed as a single frame (Wave 10).
///
/// The *Screenshot* portal can only grab whole screens, so this goes through the
/// *ScreenCast* portal with `SourceType::WINDOW`: the OS picker lists the open
/// windows and the user chooses one. Returns (png_data_url, width, height).
#[cfg(target_os = "linux")]
pub async fn capture_window() -> anyhow::Result<(String, u32, u32)> {
    grab_one(crate::portal_capture::SourceKind::Window).await
}

/// One user-picked monitor (same portal path as [`capture_window`], with
/// `SourceType::MONITOR`) — multi-monitor setups pick the output in the OS
/// dialog.
#[cfg(target_os = "linux")]
pub async fn capture_monitor() -> anyhow::Result<(String, u32, u32)> {
    grab_one(crate::portal_capture::SourceKind::Monitor).await
}

/// Grab one frame through the ScreenCast portal and encode it as a PNG data URL.
#[cfg(target_os = "linux")]
async fn grab_one(kind: crate::portal_capture::SourceKind) -> anyhow::Result<(String, u32, u32)> {
    let frame = crate::portal_capture::grab(kind)
        .await
        .map_err(anyhow::Error::msg)?;
    rgb24_to_data_url(&frame.rgb, frame.width, frame.height)
}

/// Encode a tightly-packed `rgb24` buffer as a PNG data URL.
///
/// Validates the buffer length first: `RgbImage::from_raw` returns `None` on a
/// short buffer, and handing that to the encoder would be a silent blank image.
pub fn rgb24_to_data_url(rgb: &[u8], width: u32, height: u32) -> anyhow::Result<(String, u32, u32)> {
    let expected = width as usize * height as usize * 3;
    if width == 0 || height == 0 || rgb.len() < expected {
        anyhow::bail!(
            "captured frame is {} bytes, expected {expected} for {width}x{height}",
            rgb.len()
        );
    }

    let image = image::RgbImage::from_raw(width, height, rgb[..expected].to_vec())
        .ok_or_else(|| anyhow::anyhow!("cannot build a {width}x{height} RGB image"))?;

    let mut png = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png);
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| anyhow::anyhow!("failed to encode the captured frame as PNG: {e}"))?;

    png_bytes_to_data_url(&png)
}

/// Window/monitor capture is portal-based, so non-Linux builds report the same
/// honest "not here yet" error as the recording backend instead of pretending.
#[cfg(not(target_os = "linux"))]
pub async fn capture_window() -> anyhow::Result<(String, u32, u32)> {
    anyhow::bail!("window capture is not supported on this platform yet")
}

#[cfg(not(target_os = "linux"))]
pub async fn capture_monitor() -> anyhow::Result<(String, u32, u32)> {
    anyhow::bail!("monitor capture is not supported on this platform yet")
}

/// Convert the portal's `file://` URI into a local path.
///
/// ashpd 0.14 models URIs as an opaque string, so decoding happens here:
/// scheme, optional `localhost` authority, query/fragment, then percent-escapes
/// into raw OS bytes (paths may legitimately be non-UTF-8).
pub fn file_uri_to_path(uri: &str) -> anyhow::Result<std::path::PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| anyhow::anyhow!("screenshot portal returned a non-file uri: {uri}"))?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    if !rest.starts_with('/') {
        anyhow::bail!("file uri without an absolute path: {uri}");
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Ok(std::path::PathBuf::from(std::ffi::OsString::from_vec(
            percent_decode(rest),
        )))
    }
    #[cfg(not(unix))]
    {
        Ok(std::path::PathBuf::from(rest.to_string()))
    }
}

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

/// Wrap raw PNG bytes into `data:image/png;base64,…`, validating the image so
/// callers always get real dimensions.
pub fn png_bytes_to_data_url(bytes: &[u8]) -> anyhow::Result<(String, u32, u32)> {
    use base64::Engine as _;

    let img = image::load_from_memory(bytes)
        .map_err(|e| anyhow::anyhow!("captured data is not a decodable image: {e}"))?;
    let (width, height) = (img.width(), img.height());
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok((format!("{PNG_DATA_URL_PREFIX}{b64}"), width, height))
}

/// Strip the PNG data-URL prefix and base64-decode the payload.
///
/// Accepts the canonical prefix case-insensitively enough for our own output;
/// anything without the prefix is rejected so a stray http/file URL never gets
/// silently written to disk.
pub fn decode_png_data_url(data_url: &str) -> anyhow::Result<Vec<u8>> {
    use base64::Engine as _;

    let payload = data_url
        .strip_prefix(PNG_DATA_URL_PREFIX)
        .or_else(|| {
            // Tolerate an uppercase variant and the ";charset" form some
            // canvases emit.
            if data_url.len() >= PNG_DATA_URL_PREFIX.len()
                && data_url[..PNG_DATA_URL_PREFIX.len()].eq_ignore_ascii_case(PNG_DATA_URL_PREFIX)
            {
                Some(&data_url[PNG_DATA_URL_PREFIX.len()..])
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("expected a data:image/png;base64, URL"))?;

    let payload = payload.trim();
    let payload = payload
        .split_once(';')
        .map(|(head, _)| head)
        .unwrap_or(payload);

    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| anyhow::anyhow!("invalid base64 in PNG data URL: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        // 3x2 opaque gradient, encoded through the same `image` crate the
        // production path uses.
        let mut buf = image::RgbaImage::new(3, 2);
        for (x, y, px) in buf.enumerate_pixels_mut() {
            *px = image::Rgba([
                (x * 80) as u8,
                (y * 120) as u8,
                200,
                255,
            ]);
        }
        let mut out = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut out);
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode test png");
        out
    }

    #[test]
    fn data_url_round_trip_preserves_bytes_and_dimensions() {
        let png = tiny_png();

        let (data_url, width, height) = png_bytes_to_data_url(&png).expect("encode data url");
        assert!(data_url.starts_with(PNG_DATA_URL_PREFIX));
        assert_eq!((width, height), (3, 2));

        let decoded = decode_png_data_url(&data_url).expect("decode data url");
        assert_eq!(decoded, png, "base64 round trip must be byte-exact");

        let img = image::load_from_memory(&decoded).expect("decode png");
        assert_eq!((img.width(), img.height()), (3, 2));
    }

    #[test]
    fn decode_rejects_non_png_data_url() {
        assert!(decode_png_data_url("data:image/jpeg;base64,AAAA").is_err());
        assert!(decode_png_data_url("https://example.com/a.png").is_err());
        assert!(decode_png_data_url("not a url").is_err());
    }

    #[test]
    fn file_uri_becomes_a_path() {
        assert_eq!(
            file_uri_to_path("file:///tmp/Screenshot%20from%202026.png").unwrap(),
            std::path::PathBuf::from("/tmp/Screenshot from 2026.png")
        );
        assert_eq!(
            file_uri_to_path("file://localhost/tmp/a.png").unwrap(),
            std::path::PathBuf::from("/tmp/a.png")
        );
        assert!(file_uri_to_path("https://example.com/a.png").is_err());
        assert!(file_uri_to_path("file://relative.png").is_err());
    }

    #[test]
    fn encode_rejects_garbage() {
        assert!(png_bytes_to_data_url(b"definitely not a png").is_err());
    }

    #[test]
    fn rgb24_frames_encode_to_a_png_data_url() {
        // 2x1 rgb24: one red pixel, one blue pixel.
        let rgb = [255u8, 0, 0, 0, 0, 255];
        let (data_url, width, height) = rgb24_to_data_url(&rgb, 2, 1).expect("encode rgb24");
        assert_eq!((width, height), (2, 1));

        let png = decode_png_data_url(&data_url).expect("decode back");
        let img = image::load_from_memory(&png).expect("decode png");
        assert_eq!((img.width(), img.height()), (2, 1));
        let rgba = img.to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [255, 0, 0, 255], "red stays red");
        assert_eq!(rgba.get_pixel(1, 0).0, [0, 0, 255, 255], "blue stays blue");
    }

    #[test]
    fn rgb24_short_buffers_are_rejected() {
        // 4x4 rgb24 needs 48 bytes; 47 must not become a blank image.
        assert!(rgb24_to_data_url(&[0u8; 47], 4, 4).is_err());
        assert!(rgb24_to_data_url(&[], 0, 0).is_err());
    }
}