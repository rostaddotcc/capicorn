//! Clipboard image support via arboard (Wayland data-control on Linux).

use std::borrow::Cow;

/// Write a PNG data URL to the system clipboard as an image.
pub fn copy_png_data_url(data_url: &str) -> anyhow::Result<()> {
    let png = crate::capture::decode_png_data_url(data_url)?;
    let img = image::load_from_memory(&png)
        .map_err(|e| anyhow::anyhow!("clipboard payload is not a decodable image: {e}"))?
        .to_rgba8();
    let (width, height) = img.dimensions();

    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| anyhow::anyhow!("cannot open the system clipboard: {e}"))?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(img.into_raw()),
        })
        .map_err(|e| anyhow::anyhow!("cannot write image to the clipboard: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_rejects_bad_data_url_before_touching_clipboard() {
        // Must fail on decode, not on clipboard access (which needs a session).
        assert!(copy_png_data_url("data:image/png;base64,!!!").is_err());
        assert!(copy_png_data_url("nonsense").is_err());
    }
}
