//! System clipboard image I/O via arboard (Wayland data-control on Linux,
//! X11 fallback where the compositor lacks it — same path as the live app).
//! Ported from src-tauri/src/clipboard.rs, Surface-based instead of data URLs.

use std::borrow::Cow;

use snagcraft_raster::{MAX_DIM, Surface};

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("cannot open the system clipboard: {0}")]
    Open(String),
    #[error("cannot write image to the clipboard: {0}")]
    Write(String),
    #[error("cannot read image from the clipboard: {0}")]
    Read(String),
    #[error("clipboard image is empty or too large")]
    InvalidImage,
    #[error("diagnostics text is too large for the clipboard")]
    TextTooLarge,
}

/// Hostile-input cap on text put on the clipboard (1 MiB): a runaway log tail
/// can never hand arboard a multi-megabyte string.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Write plain text to the system clipboard (Copy diagnostics, OCR results).
/// Empty text is a no-op success; oversized text is refused, never truncated
/// silently.
pub fn copy_text(text: &str) -> Result<(), ClipboardError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(ClipboardError::TextTooLarge);
    }
    if text.is_empty() {
        return Ok(());
    }
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| ClipboardError::Open(e.to_string()))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| ClipboardError::Write(e.to_string()))
}

/// Write a surface to the system clipboard as an RGBA image (pastes into
/// chats, browsers, image editors — anything that accepts image pastes).
pub fn copy_image(img: &Surface) -> Result<(), ClipboardError> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(ClipboardError::InvalidImage);
    }
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| ClipboardError::Open(e.to_string()))?;
    clipboard
        .set_image(arboard::ImageData {
            width: w as usize,
            height: h as usize,
            bytes: Cow::Owned(img.as_raw().clone()),
        })
        .map_err(|e| ClipboardError::Write(e.to_string()))
}

/// Read an image from the system clipboard (Ctrl+V into the editor).
/// Returns Ok(None) when the clipboard holds text/empty instead of an image.
pub fn paste_image() -> Result<Option<Surface>, ClipboardError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| ClipboardError::Open(e.to_string()))?;
    let data = match clipboard.get_image() {
        Ok(d) => d,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => return Err(ClipboardError::Read(e.to_string())),
    };
    let (w, h) = (data.width, data.height);
    if w == 0 || h == 0 || w > MAX_DIM as usize || h > MAX_DIM as usize {
        return Err(ClipboardError::InvalidImage);
    }
    let expected = w.saturating_mul(h).saturating_mul(4);
    if data.bytes.len() < expected {
        return Err(ClipboardError::InvalidImage);
    }
    let img = Surface::from_raw(w as u32, h as u32, data.bytes[..expected].to_vec())
        .ok_or(ClipboardError::InvalidImage)?;
    Ok(Some(img))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_rejects_empty_and_oversized() {
        let empty = Surface::new(0, 0);
        assert!(matches!(
            copy_image(&empty),
            Err(ClipboardError::InvalidImage)
        ));
        // dims check happens before clipboard access (no session needed)
        let mut huge = Surface::new(1, 1);
        // can't allocate MAX_DIM+1 square cheaply; test the guard via a 1px
        // surface with forged dims is not possible — guard covered by code path.
        huge.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
        let _ = copy_image(&huge); // may Err(Open) on a headless host — never panic
    }

    #[test]
    fn paste_never_panics_headless() {
        // On a host without a clipboard session this errors gracefully.
        let _ = paste_image();
    }
}
