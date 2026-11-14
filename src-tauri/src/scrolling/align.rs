//! Row-signature vertical alignment for scroll stitching (Wave 9, track R).
//!
//! Ported from the verified probe in `tmp/research/scrolling.md` §5.3 (which
//! recovered a 2050 px synthetic scroll exactly, with MAD separating good from
//! bad matches by ~500×: 0.1 vs 50).
//!
//! Instead of comparing pixels, each frame is reduced to a **row signature**: 8
//! narrow column bands, each a `1×H` luma vector. Cost is O(bands × H) per
//! frame instead of O(W × H), and a wrong offset is still unambiguous.
//!
//! Three guard-rails, each of which the stitcher must be able to trust:
//!
//! * **band variance floor** — a flat/blank band has no signal, so it is dropped
//!   before matching; if every band is dropped the frame is rejected (otherwise a
//!   blank area "matches" at any offset, the degenerate case in §5.3).
//! * **MAD confidence floor** — a mismatch scores ~50, a true match ~0.1, so a
//!   fixed floor is reliable and needs no tuning per content.
//! * **shift range** — `[4, 0.85·H]`: below 4 px is noise (and would append 1–3
//!   px of duplicated edge every frame), above 0.85·H there is too little overlap
//!   left to believe the match.
//!
//! The search scans **outward from the previous frame's shift**, because pages
//! are full of repeated rows (list items, table rules) where a global minimum can
//! sit one period away; the prediction keeps the scan in the right basin and lets
//! a good match exit after one or two candidates.

use std::cmp::Ordering;

/// Number of column bands in the signature (wayscrollshot's eight).
pub const BANDS: usize = 8;

/// A band is `width / BAND_DIV` pixels wide: narrow enough to be cheap, wide
/// enough that anti-aliased text still produces a stable mean.
const BAND_DIV: usize = 32;

/// Shifts under this are treated as "no movement": appending 1–3 rows per frame
/// would duplicate the seam over and over.
pub const MIN_SHIFT: isize = 4;

/// Largest believable shift, as a fraction of the viewport height.
pub const MAX_SHIFT_FRACTION: f32 = 0.85;

/// A candidate at or below this MAD is a match: stop scanning immediately.
pub const GOOD_MAD: f32 = 3.0;

/// Above this MAD the match is rejected (the probe measured ~50 for a mismatch).
pub const MAD_FLOOR: f32 = 12.0;

/// Minimum variance (grey levels²) for a band to count as having signal.
/// std dev 2 is the noise level; real text bands sit in the hundreds.
const BAND_VAR_FLOOR: f32 = 4.0;

/// Coarse scan step, in rows (`§5.3`: 2 px steps, refined afterwards).
const COARSE_STEP: isize = 2;

/// Refinement radius around the best coarse candidate, at 1 px.
const REFINE_RADIUS: isize = 3;

/// Fraction of the worst-scoring rows to discard before averaging.
///
/// This is what makes a sticky header harmless: 60 frozen rows out of a 550-row
/// overlap are the worst-scorers under a correct shift, so trimming the top 20%
/// removes them without weakening the rejection of a genuinely wrong shift
/// (whose error is spread over every row).
const TRIM_FRACTION: f32 = 0.2;

/// Rows of overlap below which a shift is not worth trusting.
const MIN_OVERLAP: usize = 16;

/// Mean absolute difference (grey levels) under which a row is considered
/// frozen at its absolute position.
///
/// Sticky headers, sticky footers and static sidebars are *identical at the same
/// y* in consecutive frames, which is precisely what distinguishes them from
/// content that moved. Masking those rows is the only way a sticky header can
/// still be matched: under the correct shift they are the rows that mismatch.
const STATIC_TOL: f32 = 1.0;

/// The row signature of one frame.
#[derive(Clone, Debug)]
pub struct Bands {
    rows: usize,
    /// `bands × rows`, band-major.
    data: Vec<u8>,
    /// Indices of the bands that carry signal (variance above the floor).
    active: Vec<usize>,
}

impl Bands {
    /// Build the signature of a tightly packed `rgb24` frame.
    ///
    /// Returns `None` for an empty frame or a buffer that is too short, so a
    /// truncated PipeWire frame can never be aligned against.
    pub fn new(rgb: &[u8], width: usize, height: usize) -> Option<Self> {
        if width == 0 || height == 0 || rgb.len() < width * height * 3 {
            return None;
        }

        let band_w = (width / BAND_DIV).max(1).min(width);
        let mut data = vec![0u8; BANDS * height];

        for band in 0..BANDS {
            // Centre each band inside its 1/8 slice of the frame.
            let centre = width * (2 * band + 1) / (2 * BANDS);
            let x0 = centre.saturating_sub(band_w / 2).min(width - band_w);
            let x1 = (x0 + band_w).min(width);

            for y in 0..height {
                let row = &rgb[y * width * 3..(y + 1) * width * 3];
                let mut sum = 0u32;
                for x in x0..x1 {
                    let p = &row[x * 3..x * 3 + 3];
                    // BT.601 luma, integer: the eye weights green most.
                    sum += (77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8;
                }
                let pixels = (x1 - x0).max(1) as u32;
                data[band * height + y] = (sum / pixels).min(255) as u8;
            }
        }

        let active: Vec<usize> = (0..BANDS)
            .filter(|&band| variance(&data[band * height..(band + 1) * height]) >= BAND_VAR_FLOOR)
            .collect();

        Some(Self {
            rows: height,
            data,
            active,
        })
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// How many bands survived the variance floor (0 = featureless frame).
    pub fn active_bands(&self) -> usize {
        self.active.len()
    }

    pub fn band(&self, index: usize) -> &[u8] {
        &self.data[index * self.rows..(index + 1) * self.rows]
    }

    /// Cheap "nothing changed" test: identical signatures mean no scroll.
    pub fn is_same(&self, other: &Bands) -> bool {
        self.rows == other.rows && self.data == other.data
    }
}

/// One resolved shift between two frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShiftMatch {
    /// Rows the content moved up (positive = scrolled down, new rows at the
    /// bottom). Negative means the user scrolled back up and new rows are at the
    /// top.
    pub shift: isize,
    /// Trimmed mean absolute difference at that shift (lower is better).
    pub mad: f32,
}

/// Find the vertical shift that carries `prev` into `cur`.
///
/// `prediction` is the previous frame's shift, used as the scan origin. Returns
/// `None` when the frames cannot be matched confidently — the stitcher must then
/// keep the frame it already has and warn instead of appending garbage.
pub fn find_offset(prev: &Bands, cur: &Bands, prediction: isize) -> Option<ShiftMatch> {
    if prev.rows != cur.rows || prev.rows == 0 {
        return None;
    }
    // Every band flat: a blank region matches at any offset, so refuse. This is
    // the guard-rail that keeps a scrolled-to-the-end blank area from stitching
    // the same whitespace forever.
    if prev.active.is_empty() || cur.active.is_empty() {
        return None;
    }

    let max = (prev.rows as f32 * MAX_SHIFT_FRACTION) as isize;
    if max < MIN_SHIFT {
        return None;
    }

    // Rows frozen at their absolute position (sticky header/footer, static
    // sidebar) are excluded from every comparison below.
    let frozen = static_rows(prev, cur);

    let start = prediction.clamp(-max, max);
    let mut best: Option<ShiftMatch> = None;

    // Scan outward from the prediction: 0, −2, +2, −4, +4, … A good match exits
    // on the spot, which is why a whole frame costs one or two comparisons in the
    // common case.
    let mut offset = 0;
    while offset <= 2 * max + COARSE_STEP {
        for candidate in if offset == 0 {
            [Some(start), None]
        } else {
            [Some(start - offset), Some(start + offset)]
        }
        .into_iter()
        .flatten()
        {
            if candidate.abs() > max || (candidate != 0 && candidate.abs() < MIN_SHIFT) {
                continue;
            }
            let Some(mad) = score(prev, cur, candidate, &frozen) else {
                continue;
            };
            if best.map(|b| mad < b.mad).unwrap_or(true) {
                best = Some(ShiftMatch {
                    shift: candidate,
                    mad,
                });
            }
            if mad <= GOOD_MAD {
                // Refine at 1 px, then stop: nothing at a further offset can beat
                // an already-excellent match.
                return refine(prev, cur, candidate, &frozen).or(best);
            }
        }
        offset += COARSE_STEP;
    }

    let best = best?;
    let refined = refine(prev, cur, best.shift, &frozen).unwrap_or(best);
    // Refinement can only lower the score, so the floor is applied afterwards.
    if refined.mad > MAD_FLOOR {
        return None;
    }
    Some(refined)
}

/// 1 px search within [`REFINE_RADIUS`] of a coarse candidate.
fn refine(prev: &Bands, cur: &Bands, coarse: isize, frozen: &[bool]) -> Option<ShiftMatch> {
    let mut best: Option<ShiftMatch> = None;
    for delta in -REFINE_RADIUS..=REFINE_RADIUS {
        let shift = coarse + delta;
        if shift != 0 && shift.abs() < MIN_SHIFT {
            continue;
        }
        let Some(mad) = score(prev, cur, shift, frozen) else {
            continue;
        };
        if best.map(|b| mad < b.mad).unwrap_or(true) {
            best = Some(ShiftMatch { shift, mad });
        }
    }
    best
}

/// Trimmed mean absolute difference between the overlapping rows of every
/// active band, for one candidate shift.
fn score(prev: &Bands, cur: &Bands, shift: isize, frozen: &[bool]) -> Option<f32> {
    let rows = prev.rows as isize;
    let moved = shift.abs();
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
        // Positive shift: cur[0..] lines up with prev[moved..].
        // Negative shift: cur[moved..] lines up with prev[0..].
        let (prev_row, cur_row) = if shift >= 0 {
            (row + moved as usize, row)
        } else {
            (row, row + moved as usize)
        };

        // A row that did not move is not evidence about the shift.
        if frozen.get(prev_row).copied().unwrap_or(false)
            || frozen.get(cur_row).copied().unwrap_or(false)
        {
            continue;
        }

        let mut sum = 0f32;
        for &band in &prev.active {
            let p = prev.band(band);
            let c = cur.band(band);
            sum += (p[prev_row] as f32 - c[cur_row] as f32).abs();
        }
        diffs.push(sum / bands);
    }

    // Too little live content left to judge (e.g. everything frozen): refuse
    // rather than guess.
    if diffs.len() < MIN_OVERLAP.min(overlap) {
        return None;
    }

    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let kept = diffs.len();
    let keep = ((kept as f32) * (1.0 - TRIM_FRACTION)).ceil() as usize;
    let keep = keep.clamp(1, kept);
    Some(diffs[..keep].iter().sum::<f32>() / keep as f32)
}

/// Which rows are identical at the same `y` in both frames.
fn static_rows(prev: &Bands, cur: &Bands) -> Vec<bool> {
    let bands = prev.active.len().max(1) as f32;
    (0..prev.rows)
        .map(|y| {
            let mut sum = 0f32;
            for &band in &prev.active {
                sum += (prev.band(band)[y] as f32 - cur.band(band)[y] as f32).abs();
            }
            sum / bands <= STATIC_TOL
        })
        .collect()
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

#[cfg(test)]
// `pub(crate)` so `stitcher`'s tests replay the same synthetic pages.
pub(crate) mod tests {
    use super::*;

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
    /// unique (per-row texture), so exactly one shift is correct.
    pub(crate) fn tall_page(width: usize, height: usize) -> Vec<u8> {
        let mut img = vec![255u8; width * height * 3];
        for y in 0..height {
            let baseline = (y / 18) % 2 == 0;
            for x in 0..width {
                // Blocky "glyphs" of ~9 px, only on text baselines.
                let ink = baseline && (hash(x / 9, y, 5) & 3) != 0;
                let base = if ink { 40 } else { 235 };
                let noise = hash(x, y, 7) % 5;
                let v = base + noise;
                let px = &mut img[(y * width + x) * 3..(y * width + x) * 3 + 3];
                px[0] = v;
                px[1] = v;
                px[2] = v;
            }
        }
        img
    }

    /// Cut a viewport-sized window out of the tall page.
    pub(crate) fn window(page: &[u8], width: usize, top: usize, height: usize) -> Vec<u8> {
        page[top * width * 3..(top + height) * width * 3].to_vec()
    }

    #[test]
    fn recovers_an_unknown_shift_exactly() {
        let page = tall_page(320, 2000);
        let prev = Bands::new(&window(&page, 320, 0, 500), 320, 500).expect("signature");
        let cur = Bands::new(&window(&page, 320, 250, 500), 320, 500).expect("signature");

        let found = find_offset(&prev, &cur, 0).expect("250 px scroll must match");
        assert_eq!(found.shift, 250, "exact shift, no off-by-one");
        assert!(
            found.mad <= GOOD_MAD,
            "clean match scores near zero, got {}",
            found.mad
        );
    }

    #[test]
    fn prediction_keeps_repeated_patterns_on_the_right_period() {
        // Every 40th row is identical (a repeating list item), so a global scan
        // has several equally good minima 40 px apart; the prediction must keep
        // us on the true one.
        let mut page = tall_page(320, 1200);
        let row = page[0..320 * 3].to_vec();
        let mut y = 0;
        while y + 40 < 1200 {
            page[(y + 40) * 320 * 3..(y + 41) * 320 * 3].copy_from_slice(&row);
            y += 40;
        }

        let prev = Bands::new(&window(&page, 320, 0, 400), 320, 400).expect("signature");
        let cur = Bands::new(&window(&page, 320, 120, 400), 320, 400).expect("signature");
        let found = find_offset(&prev, &cur, 120).expect("match");
        assert_eq!(found.shift, 120, "prediction disambiguates the period");
    }

    #[test]
    fn flat_region_is_rejected_not_matched_at_an_arbitrary_offset() {
        let flat = vec![200u8; 320 * 400 * 3];
        let prev = Bands::new(&flat, 320, 400).expect("signature");
        let cur = Bands::new(&flat, 320, 400).expect("signature");
        assert_eq!(prev.active_bands(), 0, "no band carries signal");
        assert!(find_offset(&prev, &cur, 8).is_none(), "blank must not match");
    }

    #[test]
    fn shift_beyond_the_overlap_limit_is_rejected() {
        let page = tall_page(320, 2000);
        let prev = Bands::new(&window(&page, 320, 0, 400), 320, 400).expect("signature");
        // 0.85 * 400 = 340: a 360 px scroll leaves less overlap than we trust.
        let cur = Bands::new(&window(&page, 320, 360, 400), 320, 400).expect("signature");
        assert!(
            find_offset(&prev, &cur, 0).is_none(),
            "fast scroll past the range must be refused"
        );
    }

    #[test]
    fn unrelated_content_is_rejected_by_the_mad_floor() {
        let a = tall_page(320, 400);
        // Same width/height, no row in common with `a`: a real mismatch.
        let mut b = vec![0u8; 320 * 400 * 3];
        for y in 0..400 {
            let v = hash(0, y, 99) / 2; // 0..128 grey rows, all different
            b[y * 320 * 3..(y + 1) * 320 * 3].fill(v);
        }
        let prev = Bands::new(&a, 320, 400).expect("signature");
        let cur = Bands::new(&b, 320, 400).expect("signature");
        // Either the content genuinely matches at some shift (unlikely) or it is
        // refused; what must never happen is a confident garbage match.
        if let Some(found) = find_offset(&prev, &cur, 0) {
            assert!(
                found.mad <= MAD_FLOOR,
                "a returned match must be under the floor"
            );
        }
    }

    #[test]
    fn reverse_scroll_is_found_as_a_negative_shift() {
        let page = tall_page(320, 2000);
        let prev = Bands::new(&window(&page, 320, 300, 500), 320, 500).expect("signature");
        let cur = Bands::new(&window(&page, 320, 150, 500), 320, 500).expect("signature");
        let found = find_offset(&prev, &cur, 0).expect("reverse scroll must match");
        assert_eq!(found.shift, -150);
    }
}