//! Row-signature vertical alignment for manual-scroll stitching.
//!
//! Ported from the live Tauri implementation (`src-tauri/src/scrolling/align.rs`,
//! itself verified in a probe that recovered a 2050 px synthetic scroll exactly,
//! with MAD separating good from bad matches by ~500×: 0.1 vs 50).
//!
//! A frame is reduced to a **row signature**: [`BANDS`] narrow column bands, each
//! a `1×H` luma vector. Matching two frames costs `O(bands × H)` instead of
//! `O(W × H)`, and a wrong offset is still unambiguous.
//!
//! Three guard-rails the stitcher trusts:
//!
//! * **band variance floor** — a flat band carries no signal and is dropped; if
//!   every band is dropped the frame cannot be matched at all (a blank region
//!   "matches" at any offset).
//! * **MAD confidence floor** — a mismatch scores ~50 grey levels, a true match
//!   ~0.1, so one fixed threshold works for all content.
//! * **shift range** — `[MIN_SHIFT, 0.85·H]`: below 4 px is noise (and would
//!   append 1–3 duplicated edge rows per frame), above 0.85·H too little overlap
//!   is left to believe the match.
//!
//! Sign convention: a positive offset means the page content moved **up** by that
//! many rows (`next[y] == prev[y + offset]`), i.e. the user scrolled down.

use snagcraft_raster::Surface;

/// Number of column bands in the signature.
pub const BANDS: usize = 8;

/// A band is `width / BAND_DIV` pixels wide: narrow enough to be cheap, wide
/// enough that anti-aliased text still yields a stable mean.
const BAND_DIV: u32 = 32;

/// Offsets under this are "no movement": appending 1–3 rows per frame would
/// duplicate the seam over and over.
pub const MIN_SHIFT: i32 = 4;

/// Largest believable offset, as a fraction of the viewport height.
pub const MAX_SHIFT_FRACTION: f32 = 0.85;

/// A candidate at or below this MAD is a match: stop scanning immediately.
pub const GOOD_MAD: f32 = 3.0;

/// Above this MAD a match is rejected (~50 grey levels for unrelated content).
pub const MAD_FLOOR: f32 = 12.0;

/// Minimum band variance (grey levels²) for a band to count as having signal.
const BAND_VAR_FLOOR: f32 = 4.0;

/// Coarse scan step, in rows (2 px steps, refined afterwards).
const COARSE_STEP: i32 = 2;

/// Refinement radius around the best coarse candidate, at 1 px.
const REFINE_RADIUS: i32 = 3;

/// Fraction of the worst-scoring rows discarded before averaging.
///
/// This is what makes a sticky header harmless: frozen rows are the worst
/// scorers under a correct offset, so trimming the top 20 % removes them without
/// weakening the rejection of a genuinely wrong offset (whose error is spread
/// over every row).
const TRIM_FRACTION: f32 = 0.2;

/// Rows of overlap below which an offset is not worth trusting.
const MIN_OVERLAP: usize = 16;

/// Mean absolute difference (grey levels) under which a row counts as frozen at
/// its absolute position (sticky header/footer, static sidebar).
const STATIC_TOL: f32 = 1.0;

/// Columns sampled by the full-pixel MAD verification pass.
const VERIFY_COLUMNS: u32 = 64;

/// Rows sampled by the full-pixel MAD verification pass (bounded cost).
const VERIFY_ROWS: usize = 128;

/// Full-pixel MAD above which a band match is not believed.
///
/// The band signature is a proxy; a repeated pattern can align bands while the
/// pixels disagree, so the winning offset is re-scored on real pixels before it
/// is handed to the stitcher.
pub const VERIFY_MAD_FLOOR: f32 = 16.0;

/// BT.601 luma of one pixel, integer: the eye weights green most.
fn luma(pixel: &[u8; 4]) -> u32 {
    (77 * pixel[0] as u32 + 150 * pixel[1] as u32 + 29 * pixel[2] as u32) >> 8
}

/// The `(x0, x1)` span of each band, centred inside its 1/8 slice of the frame.
fn band_spans(width: u32) -> [(u32, u32); BANDS] {
    let band_w = (width / BAND_DIV).max(1).min(width.max(1));
    let mut spans = [(0u32, 0u32); BANDS];
    for (band, slot) in spans.iter_mut().enumerate() {
        let centre = (width as usize * (2 * band + 1) / (2 * BANDS)) as u32;
        let x0 = centre
            .saturating_sub(band_w / 2)
            .min(width.saturating_sub(band_w));
        let x1 = (x0 + band_w).min(width);
        *slot = (x0, x1);
    }
    spans
}

/// Mean luma of one band inside row `y`.
fn band_luma(img: &Surface, y: u32, span: (u32, u32)) -> u8 {
    let (x0, x1) = span;
    if x1 <= x0 {
        return 0;
    }
    let mut sum = 0u32;
    for x in x0..x1 {
        sum += luma(&img.get_pixel(x, y).0);
    }
    (sum / (x1 - x0)).min(255) as u8
}

/// The row signature of row `y`: one mean luma per band.
///
/// Pure and allocation-bounded (`BANDS` bytes). A row outside the image yields
/// zeros rather than panicking — callers pass hostile coordinates.
pub fn row_signature(img: &Surface, y: u32) -> Vec<u8> {
    let mut out = vec![0u8; BANDS];
    if y >= img.height() || img.width() == 0 {
        return out;
    }
    let spans = band_spans(img.width());
    for (band, span) in spans.iter().enumerate() {
        out[band] = band_luma(img, y, *span);
    }
    out
}

/// Population variance of one band's row vector.
fn variance(rows: &[u8]) -> f32 {
    if rows.len() < 2 {
        return 0.0;
    }
    let n = rows.len() as f32;
    let mean = rows.iter().map(|&v| v as f32).sum::<f32>() / n;
    rows.iter()
        .map(|&v| {
            let d = v as f32 - mean;
            d * d
        })
        .sum::<f32>()
        / n
}

/// The whole frame's signature: [`BANDS`] band vectors, band-major.
pub(crate) struct Signature {
    rows: usize,
    /// `bands × rows`, band-major.
    data: Vec<u8>,
    /// Indices of bands whose variance clears [`BAND_VAR_FLOOR`].
    active: Vec<usize>,
}

impl Signature {
    /// Build a frame's signature. `None` for an empty or absurdly wide frame, so
    /// a truncated/hostile buffer can never be aligned against.
    pub(crate) fn of(img: &Surface) -> Option<Self> {
        let rows = img.height() as usize;
        if rows == 0 || img.width() == 0 {
            return None;
        }
        let spans = band_spans(img.width());
        let mut data = vec![0u8; BANDS * rows];
        for y in 0..rows {
            for (band, span) in spans.iter().enumerate() {
                data[band * rows + y] = band_luma(img, y as u32, *span);
            }
        }
        let active: Vec<usize> = (0..BANDS)
            .filter(|&band| variance(&data[band * rows..(band + 1) * rows]) >= BAND_VAR_FLOOR)
            .collect();
        Some(Self { rows, data, active })
    }

    fn band(&self, index: usize) -> &[u8] {
        &self.data[index * self.rows..(index + 1) * self.rows]
    }

    /// How many bands survived the variance floor (0 = featureless frame).
    ///
    /// Kept as a read-only probe for tests and diagnostics.
    #[allow(dead_code)]
    pub(crate) fn active_bands(&self) -> usize {
        self.active.len()
    }
}

/// One resolved offset between two frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffsetMatch {
    /// Rows the content moved up (positive = scrolled down, new rows at the
    /// bottom). Negative means the user scrolled back up.
    pub offset: i32,
    /// Trimmed band MAD at that offset (lower is better).
    pub mad: f32,
}

/// Find the vertical offset that carries `prev` into `next`.
///
/// `max_shift` bounds the search (hostile input): the effective ceiling is the
/// smaller of `max_shift`, `0.85·H` and `H − MIN_OVERLAP`. Returns `None` when no
/// offset can be matched confidently — the stitcher must then keep what it has
/// and warn instead of appending garbage.
///
/// The winning candidate is re-scored on real pixels ([`VERIFY_MAD_FLOOR`]) so a
/// band-signature coincidence cannot smuggle a wrong offset through.
pub fn align_offset(prev: &Surface, next: &Surface, max_shift: u32) -> Option<i32> {
    let found = find_match(prev, next, max_shift)?;
    if verify_pixels(prev, next, found.offset) > VERIFY_MAD_FLOOR {
        return None;
    }
    Some(found.offset)
}

/// The band-only half of [`align_offset`], exposed for the stitcher's tests.
pub(crate) fn find_match(prev: &Surface, next: &Surface, max_shift: u32) -> Option<OffsetMatch> {
    if prev.dimensions() != next.dimensions() {
        return None;
    }
    let prev = Signature::of(prev)?;
    let next = Signature::of(next)?;
    if prev.rows != next.rows || prev.rows == 0 {
        return None;
    }
    // Every band flat: a blank region matches at any offset, so refuse.
    if prev.active.is_empty() || next.active.is_empty() {
        return None;
    }

    let by_overlap = prev.rows as i32 - MIN_OVERLAP as i32;
    let by_fraction = (prev.rows as f32 * MAX_SHIFT_FRACTION) as i32;
    let max = by_fraction.min(by_overlap).min(max_shift as i32);
    if max < MIN_SHIFT {
        return None;
    }

    // Rows frozen at their absolute position (sticky header/footer) are excluded
    // from every comparison.
    let frozen = static_rows(&prev, &next);

    // *Every* row frozen at its own y: the content did not move at all (a static
    // screen, or a frame redrawn identically). That is a genuine offset of 0 —
    // reporting `None` here would make a still screen look like a failed match.
    if frozen.iter().all(|&row| row) {
        return Some(OffsetMatch {
            offset: 0,
            mad: 0.0,
        });
    }

    let mut best: Option<OffsetMatch> = None;
    // Scan outward from 0: -2, +2, -4, +4, … Small scrolls (the common case)
    // are found in a couple of comparisons, and a repeated row period cannot
    // steal the match before the true, nearer offset is scored.
    let mut step = 0;
    while step <= 2 * max + COARSE_STEP {
        let candidates: [Option<i32>; 2] = if step == 0 {
            [Some(0), None]
        } else {
            [Some(-step), Some(step)]
        };
        for candidate in candidates.into_iter().flatten() {
            if candidate.abs() > max || (candidate != 0 && candidate.abs() < MIN_SHIFT) {
                continue;
            }
            let Some(mad) = score(&prev, &next, candidate, &frozen) else {
                continue;
            };
            if best.map(|b| mad < b.mad).unwrap_or(true) {
                best = Some(OffsetMatch {
                    offset: candidate,
                    mad,
                });
            }
            if mad <= GOOD_MAD {
                return refine(&prev, &next, candidate, &frozen).or(best);
            }
        }
        step += COARSE_STEP;
    }

    let best = best?;
    let refined = refine(&prev, &next, best.offset, &frozen).unwrap_or(best);
    // Refinement can only lower the score, so the floor is applied afterwards.
    if refined.mad > MAD_FLOOR {
        return None;
    }
    Some(refined)
}

/// 1 px search within [`REFINE_RADIUS`] of a coarse candidate.
fn refine(prev: &Signature, next: &Signature, coarse: i32, frozen: &[bool]) -> Option<OffsetMatch> {
    let mut best: Option<OffsetMatch> = None;
    for delta in -REFINE_RADIUS..=REFINE_RADIUS {
        let offset = coarse + delta;
        if offset != 0 && offset.abs() < MIN_SHIFT {
            continue;
        }
        let Some(mad) = score(prev, next, offset, frozen) else {
            continue;
        };
        if best.map(|b| mad < b.mad).unwrap_or(true) {
            best = Some(OffsetMatch { offset, mad });
        }
    }
    best
}

/// Trimmed mean absolute difference between the overlapping rows of every active
/// band, for one candidate offset.
fn score(prev: &Signature, next: &Signature, offset: i32, frozen: &[bool]) -> Option<f32> {
    let rows = prev.rows as i32;
    let moved = offset.abs();
    if moved >= rows {
        return None;
    }
    let overlap = (rows - moved) as usize;
    if overlap < MIN_OVERLAP {
        return None;
    }

    let bands = prev.active.len().max(1) as f32;
    let mut diffs = Vec::with_capacity(overlap);

    for row in 0..overlap {
        // Positive offset: next[0..] lines up with prev[moved..].
        // Negative offset: next[moved..] lines up with prev[0..].
        let (prev_row, next_row) = if offset >= 0 {
            (row + moved as usize, row)
        } else {
            (row, row + moved as usize)
        };

        // A row that did not move is no evidence about the offset.
        if frozen.get(prev_row).copied().unwrap_or(false)
            || frozen.get(next_row).copied().unwrap_or(false)
        {
            continue;
        }

        let mut sum = 0f32;
        for &band in &prev.active {
            sum += (prev.band(band)[prev_row] as f32 - next.band(band)[next_row] as f32).abs();
        }
        diffs.push(sum / bands);
    }

    if diffs.len() < MIN_OVERLAP.min(overlap) {
        return None;
    }

    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let kept = diffs.len();
    let keep = ((kept as f32) * (1.0 - TRIM_FRACTION)).ceil() as usize;
    let keep = keep.clamp(1, kept);
    Some(diffs[..keep].iter().sum::<f32>() / keep as f32)
}

/// Which rows are identical at the same `y` in both frames.
fn static_rows(prev: &Signature, next: &Signature) -> Vec<bool> {
    let bands = prev.active.len().max(1) as f32;
    (0..prev.rows)
        .map(|y| {
            let mut sum = 0f32;
            for &band in &prev.active {
                sum += (prev.band(band)[y] as f32 - next.band(band)[y] as f32).abs();
            }
            sum / bands <= STATIC_TOL
        })
        .collect()
}

/// Mean absolute luma difference over the overlap, sampled on real pixels.
///
/// Columns and rows are strided so the pass is bounded (`VERIFY_COLUMNS ×
/// VERIFY_ROWS` samples) regardless of the frame size; `f32::MAX` signals "no
/// comparable overlap" so the caller rejects the offset.
fn verify_pixels(prev: &Surface, next: &Surface, offset: i32) -> f32 {
    let (width, height) = prev.dimensions();
    if next.dimensions() != (width, height) || width == 0 || height == 0 {
        return f32::MAX;
    }
    let moved = offset.unsigned_abs();
    if moved >= height {
        return f32::MAX;
    }
    let overlap = height - moved;
    if overlap < MIN_OVERLAP as u32 {
        return f32::MAX;
    }

    let step_x = (width / VERIFY_COLUMNS).max(1);
    let step_y = (overlap / VERIFY_ROWS as u32).max(1);
    let mut sum = 0f64;
    let mut count = 0u64;

    let mut row = 0;
    while row < overlap {
        let (prev_y, next_y) = if offset >= 0 {
            (row + moved, row)
        } else {
            (row, row + moved)
        };
        let mut x = 0;
        while x < width {
            let a = luma(&prev.get_pixel(x, prev_y).0);
            let b = luma(&next.get_pixel(x, next_y).0);
            sum += (a as f64 - b as f64).abs();
            count += 1;
            x += step_x;
        }
        row += step_y;
    }

    if count == 0 {
        return f32::MAX;
    }
    (sum / count as f64) as f32
}

#[cfg(test)]
pub(crate) mod synth {
    //! Synthetic scrolled "pages" shared by the align and stitch tests.
    //!
    //! Deterministic, per-row-unique content: a wrong vertical offset always
    //! mismatches, so a recovered offset is either exactly right or rejected —
    //! never accidentally plausible.

    use snagcraft_raster::Surface;

    /// Deterministic pseudo-random byte for `(x, y, seed)`.
    fn hash(x: usize, y: usize, seed: u32) -> u8 {
        let mut v = (x as u32)
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add((y as u32).wrapping_mul(0x85EB_CA6B))
            .wrapping_add(seed.wrapping_mul(0xC2B2_AE35));
        v ^= v >> 15;
        v = v.wrapping_mul(0x2545_F491);
        v ^= v >> 13;
        (v & 0xFF) as u8
    }

    /// A tall synthetic "page": text-line banding for realism, but every row is
    /// unique (per-row texture), so exactly one offset is correct.
    pub(crate) fn tall_page(width: u32, height: u32, seed: u32) -> Surface {
        let mut page = Surface::new(width, height);
        for y in 0..height {
            let baseline = (y / 18) % 2 == 0;
            for x in 0..width {
                // Blocky "glyphs" of ~9 px, only on text baselines.
                let ink = baseline && (hash(x as usize / 9, y as usize, seed + 5) & 3) != 0;
                let base = if ink { 40 } else { 235 };
                let v = base + hash(x as usize, y as usize, seed + 7) % 5;
                page.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        page
    }

    /// A viewport-sized window cut out of a tall page.
    pub(crate) fn window(page: &Surface, top: u32, height: u32) -> Surface {
        image::imageops::crop_imm(page, 0, top, page.width(), height).to_image()
    }

    /// A flat area: matches at any offset, so the aligner must refuse it.
    pub(crate) fn flat(width: u32, height: u32, grey: u8) -> Surface {
        Surface::from_pixel(width, height, image::Rgba([grey, grey, grey, 255]))
    }
}

#[cfg(test)]
mod tests {
    use super::synth::{flat, tall_page, window};
    use super::*;

    const W: u32 = 320;
    const H: u32 = 500;

    #[test]
    fn a_row_signature_has_one_value_per_band() {
        let page = tall_page(W, 40, 1);
        let signature = row_signature(&page, 3);
        assert_eq!(signature.len(), BANDS);
        // Text rows vary across the width, so the bands are not all identical.
        assert!(signature.iter().any(|&v| v != signature[0]));
    }

    #[test]
    fn a_row_outside_the_image_is_all_zeros() {
        let page = tall_page(W, 40, 1);
        assert_eq!(row_signature(&page, 40), vec![0u8; BANDS]);
        assert_eq!(row_signature(&page, u32::MAX), vec![0u8; BANDS]);
    }

    #[test]
    fn recovers_an_unknown_offset_exactly() {
        let page = tall_page(W, 2_000, 1);
        let prev = window(&page, 0, H);
        let next = window(&page, 250, H);

        let found = align_offset(&prev, &next, H).expect("a 250 px scroll must match");
        assert_eq!(found, 250, "exact offset, no off-by-one");
        let match_score = find_match(&prev, &next, H).expect("band match").mad;
        assert!(
            match_score <= GOOD_MAD,
            "a clean match scores near zero, got {match_score}"
        );
    }

    #[test]
    fn repeated_row_periods_cannot_steal_a_small_scroll() {
        // Every 40th row is identical (a repeating list item), so several offsets
        // score well; the outward scan must stop at the true, nearer one.
        let mut page = tall_page(W, 1_200, 2);
        let row = image::imageops::crop_imm(&page, 0, 0, W, 1).to_image();
        for y in (40..1_200).step_by(40) {
            image::imageops::replace(&mut page, &row, 0, i64::from(y));
        }

        let prev = window(&page, 0, 400);
        let next = window(&page, 80, 400);
        assert_eq!(align_offset(&prev, &next, 400), Some(80));
    }

    #[test]
    fn mad_rejects_unrelated_content() {
        let left = tall_page(W, H, 3);
        let right = tall_page(W, H, 99);
        assert_eq!(
            align_offset(&left, &right, H),
            None,
            "different text must not produce an offset"
        );
        // Whatever the band scorer thinks, it may never return a confident match.
        if let Some(found) = find_match(&left, &right, H) {
            assert!(
                found.mad <= MAD_FLOOR,
                "returned match must clear the floor"
            );
        }
    }

    #[test]
    fn a_flat_region_is_refused_rather_than_matched_anywhere() {
        let blank = flat(W, H, 250);
        let nudged = flat(W, H, 249);
        assert_eq!(align_offset(&blank, &blank, H), None, "blank has no signal");
        assert_eq!(
            align_offset(&blank, &nudged, H),
            None,
            "a featureless shift must not be guessed"
        );
    }

    #[test]
    fn a_scroll_past_the_overlap_limit_is_refused() {
        let page = tall_page(W, 2_000, 4);
        let prev = window(&page, 0, 400);
        // 0.85 * 400 = 340: a 360 px step leaves less overlap than we trust.
        let next = window(&page, 360, 400);
        assert_eq!(align_offset(&prev, &next, 400), None);
    }

    #[test]
    fn max_shift_bounds_the_search() {
        let page = tall_page(W, 2_000, 5);
        let prev = window(&page, 0, H);
        let next = window(&page, 250, H);
        // The true offset is 250; a hostile ceiling of 100 must refuse rather than
        // report something smaller.
        assert_eq!(align_offset(&prev, &next, 100), None);
        assert_eq!(align_offset(&prev, &next, 0), None, "no search at all");
    }

    #[test]
    fn reverse_scroll_is_a_negative_offset() {
        let page = tall_page(W, 2_000, 6);
        let prev = window(&page, 300, H);
        let next = window(&page, 150, H);
        assert_eq!(align_offset(&prev, &next, H), Some(-150));
    }

    #[test]
    fn frames_of_different_sizes_are_refused() {
        let page = tall_page(W, 1_000, 7);
        let prev = window(&page, 0, H);
        let next = window(&page, 100, H - 20);
        assert_eq!(align_offset(&prev, &next, H), None);
        assert_eq!(align_offset(&prev, &prev, 0), None);
    }

    #[test]
    fn identical_frames_match_at_zero() {
        let page = tall_page(W, 600, 8);
        let frame = window(&page, 50, H);
        assert_eq!(align_offset(&frame, &frame, H), Some(0));
    }
}
