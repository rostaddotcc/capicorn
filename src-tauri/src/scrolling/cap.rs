//! Height / memory guard for the accumulated scrolling capture.
//!
//! Shottr's numbers are the reference: 3072×80 000 at 4 B/px ≈ 1 GB. We keep the
//! whole stitch as `rgb24` in memory (3 B/px) and then encode one PNG, so the cap
//! has to cover both the accumulation and the encoder's copy. 40 000 px is the
//! same public limit Shottr uses, and it is what the refusal message names so the
//! user has an actionable next step instead of a silent truncation.

/// Hard cap on the stitched height, in pixels.
pub const MAX_HEIGHT: u32 = 40_000;

/// Hard cap on the accumulated `rgb24` buffer (≈ 900 MB).
pub const MAX_BYTES: usize = 900 * 1024 * 1024;

/// Bytes per pixel of the frame format we stitch (`rgb24`).
pub const BYTES_PER_PIXEL: usize = 3;

/// Would `rows + add` fit? `Err` carries the message shown in the overlay.
pub fn check(width: u32, rows: u32, add: u32) -> Result<(), String> {
    let planned = rows.saturating_add(add);
    if planned > MAX_HEIGHT {
        return Err(format!(
            "this capture hit the {MAX_HEIGHT} px height limit — save what you have and \
             capture the rest in a second pass"
        ));
    }
    let bytes = (width as usize)
        .saturating_mul(planned as usize)
        .saturating_mul(BYTES_PER_PIXEL);
    if bytes > MAX_BYTES {
        return Err(format!(
            "this capture would need {} MB of memory — save what you have and capture the \
             rest in a second pass",
            bytes / (1024 * 1024)
        ));
    }
    Ok(())
}

/// Rows still allowed for the given width, so the stitcher can trim a big
/// accepted shift instead of dropping the frame.
pub fn room(width: u32, rows: u32) -> u32 {
    let by_height = MAX_HEIGHT.saturating_sub(rows);
    let by_bytes = if width == 0 {
        0
    } else {
        (MAX_BYTES / (width as usize * BYTES_PER_PIXEL)) as u32
    };
    by_height.min(by_bytes.saturating_sub(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_capture_is_allowed() {
        assert!(check(1200, 0, 800).is_ok());
        assert!(check(1200, 30_000, 9_000).is_ok());
    }

    #[test]
    fn the_height_cap_refuses_with_a_friendly_message() {
        let err = check(1200, 39_000, 2_000).expect_err("over the cap");
        assert!(err.contains("40000"), "{err}");
        assert!(err.contains("second pass"), "{err}");
    }

    #[test]
    fn a_huge_4k_capture_is_refused_on_memory_not_just_height() {
        // 3840 px * 40 000 rows * 3 B = 460 MB: still allowed. 8K width is not.
        assert!(check(3840, 0, 39_000).is_ok());
        let err = check(10_000, 0, 39_000).expect_err("over the byte cap");
        assert!(err.contains("MB"), "{err}");
    }

    #[test]
    fn room_shrinks_as_the_image_grows() {
        assert_eq!(room(1200, 0), MAX_HEIGHT);
        assert_eq!(room(1200, 39_000), 1_000);
        assert_eq!(room(0, 0), 0);
    }
}