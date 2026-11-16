#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L2 text engine — bundled font faces and family/weight resolution.
//!
//! The 14 OFL TTFs shipped in `rust/assets/fonts/` are embedded in the binary with
//! [`include_bytes!`]; there is no runtime filesystem access (sans-I/O rule). Parsed
//! faces live in [`FontDb`], which hands out renderable faces to `snagcraft-compose`.
//!
//! Only 10 families are exposed live ([`FontDb::families`]), mirroring the editor's font
//! menu. Italic is synthetic: the live bundles ship no italic faces (the browser obliques
//! by shear), so the compose layer applies [`ITALIC_SHEAR`] to the outline at draw time.
//!
//! Weight resolution: [`FontDb::resolve`] keeps the locked `-> Option<FontVec>` contract and
//! builds an owned `FontVec` from the embedded bytes (ab_glyph 0.2's `FontVec` is *not*
//! `Clone`, so a "cheap clone" is not available). Hot render loops should use
//! [`FontDb::resolve_shared`], which clones an `Arc`-backed [`FontArc`] instead of
//! re-parsing.

use std::sync::atomic::{AtomicUsize, Ordering};

use ab_glyph::{FontArc, FontVec};

/// Synthetic oblique shear applied at render time (the live bundles ship no italic faces).
pub const ITALIC_SHEAR: f32 = 0.2;

/// Live font menu order — must match the editor's family list exactly.
const FAMILY_ORDER: [&str; 10] = [
    "Inter",
    "Lato",
    "Source Serif 4",
    "Playfair Display",
    "Oswald",
    "Archivo Black",
    "JetBrains Mono",
    "Fira Code",
    "Caveat",
    "Pacifico",
];

/// Family used when a requested family is unknown and when Inter itself fails to parse.
const DEFAULT_FAMILY: &str = "Inter";

/// One embedded family: its regular face plus an optional bold (700) face.
struct FaceSpec {
    family: &'static str,
    regular: &'static [u8],
    bold: Option<&'static [u8]>,
}

/// Embedded faces, in [`FAMILY_ORDER`] order. 10 regular + 4 bold = 14 faces.
const BUNDLED: [FaceSpec; 10] = [
    FaceSpec {
        family: "Inter",
        regular: include_bytes!("../../../assets/fonts/inter-400.ttf"),
        bold: Some(include_bytes!("../../../assets/fonts/inter-700.ttf")),
    },
    FaceSpec {
        family: "Lato",
        regular: include_bytes!("../../../assets/fonts/lato-400.ttf"),
        bold: Some(include_bytes!("../../../assets/fonts/lato-700.ttf")),
    },
    FaceSpec {
        family: "Source Serif 4",
        regular: include_bytes!("../../../assets/fonts/source-serif-4-400.ttf"),
        bold: None,
    },
    FaceSpec {
        family: "Playfair Display",
        regular: include_bytes!("../../../assets/fonts/playfair-400.ttf"),
        bold: Some(include_bytes!("../../../assets/fonts/playfair-700.ttf")),
    },
    FaceSpec {
        family: "Oswald",
        regular: include_bytes!("../../../assets/fonts/oswald-400.ttf"),
        bold: Some(include_bytes!("../../../assets/fonts/oswald-700.ttf")),
    },
    FaceSpec {
        family: "Archivo Black",
        regular: include_bytes!("../../../assets/fonts/archivoblack-400.ttf"),
        bold: None,
    },
    FaceSpec {
        family: "JetBrains Mono",
        regular: include_bytes!("../../../assets/fonts/jetbrains-mono-400.ttf"),
        bold: None,
    },
    FaceSpec {
        family: "Fira Code",
        regular: include_bytes!("../../../assets/fonts/fira-400.ttf"),
        bold: None,
    },
    FaceSpec {
        family: "Caveat",
        regular: include_bytes!("../../../assets/fonts/caveat-400.ttf"),
        bold: None,
    },
    FaceSpec {
        family: "Pacifico",
        regular: include_bytes!("../../../assets/fonts/pacifico-400.ttf"),
        bold: None,
    },
];

/// Number of embedded face slots (10 regular + 4 bold) — the healthy total.
const EMBEDDED_FACE_COUNT: usize = 14;

/// Faces successfully parsed by the most recent [`FontDb::bundled`] call.
static LOADED_FACE_COUNT: AtomicUsize = AtomicUsize::new(EMBEDDED_FACE_COUNT);

/// A parsed face: the embedded bytes (source of truth) plus a shared parsed handle.
struct FaceEntry {
    bytes: &'static [u8],
    font: FontArc,
}

impl FaceEntry {
    /// Builds an owned `FontVec` for the locked `resolve` contract.
    fn to_font_vec(&self) -> Option<FontVec> {
        match FontVec::try_from_vec_and_index(self.bytes.to_vec(), 0) {
            Ok(font) => Some(font),
            Err(err) => {
                tracing::warn!(error = %err, "bundled face failed to re-parse");
                None
            }
        }
    }
}

/// A parsed family: regular face and, when the bundle ships one, a 700 face.
struct Face {
    family: &'static str,
    regular: Option<FaceEntry>,
    bold: Option<FaceEntry>,
}

impl Face {
    /// Regular face, falling back to the bold face if only that one parsed.
    fn regular_face(&self) -> Option<&FaceEntry> {
        self.regular.as_ref().or(self.bold.as_ref())
    }

    /// Bold face when the family has a real 700 face, else the regular face.
    fn bold_face(&self) -> Option<&FaceEntry> {
        self.bold.as_ref().or(self.regular.as_ref())
    }

    fn face(&self, bold: bool) -> Option<&FaceEntry> {
        if bold {
            self.bold_face()
        } else {
            self.regular_face()
        }
    }
}

/// Owns the parsed bundled faces. Cheap to build once per process.
pub struct FontDb {
    faces: Vec<Face>,
}

impl FontDb {
    /// Parses all 14 embedded faces. A corrupt face is skipped with a `tracing::warn`
    /// (never a panic), so a damaged asset degrades that one face, not the editor.
    pub fn bundled() -> FontDb {
        let mut faces = Vec::with_capacity(BUNDLED.len());
        let mut loaded = 0usize;

        for spec in &BUNDLED {
            let regular = parse_face(spec.regular, spec.family, 400);
            let bold = spec
                .bold
                .and_then(|bytes| parse_face(bytes, spec.family, 700));

            if regular.is_some() {
                loaded += 1;
            }
            if bold.is_some() {
                loaded += 1;
            }

            // Drop a family only when neither face survived; an empty entry would shadow
            // the Inter/first-available fallback with a `None`.
            if regular.is_none() && bold.is_none() {
                continue;
            }

            faces.push(Face {
                family: spec.family,
                regular,
                bold,
            });
        }

        LOADED_FACE_COUNT.store(loaded, Ordering::Relaxed);

        FontDb { faces }
    }

    /// Live font menu order (10 families) — what the editor's font picker shows.
    pub fn families() -> &'static [&'static str] {
        &FAMILY_ORDER
    }

    /// Faces embedded in the binary (14) — or fewer if the last `bundled()` had to skip a
    /// corrupt face. Read before any `bundled()` call this reports the healthy total.
    pub fn face_count() -> usize {
        LOADED_FACE_COUNT.load(Ordering::Relaxed)
    }

    /// Resolves `family` + weight to an owned renderable face.
    ///
    /// Matching is case-insensitive. Unknown family falls back to Inter, then to the first
    /// available face. `bold` returns the 700 face when the family has one and the regular
    /// face otherwise. `None` only if no face could be parsed at all.
    pub fn resolve(&self, family: &str, bold: bool) -> Option<FontVec> {
        self.find(family)?.face(bold)?.to_font_vec()
    }

    /// Same resolution as [`FontDb::resolve`] but returns an `Arc`-backed [`FontArc`].
    ///
    /// This is the cheap path for per-frame rendering: cloning a `FontArc` is an atomic
    /// increment, whereas `resolve` re-parses the face bytes every call.
    pub fn resolve_shared(&self, family: &str, bold: bool) -> Option<FontArc> {
        self.find(family)?
            .face(bold)
            .map(|entry| entry.font.clone())
    }

    /// Case-insensitive family lookup with the Inter → first-available fallback chain.
    fn find(&self, family: &str) -> Option<&Face> {
        self.faces
            .iter()
            .find(|face| face.family.eq_ignore_ascii_case(family))
            .or_else(|| {
                self.faces
                    .iter()
                    .find(|face| face.family.eq_ignore_ascii_case(DEFAULT_FAMILY))
            })
            .or_else(|| self.faces.first())
    }
}

/// Parses one TTF face; `None` (with a warning) if the bytes are not a usable font.
fn parse_face(bytes: &'static [u8], family: &'static str, weight: u16) -> Option<FaceEntry> {
    match FontVec::try_from_vec_and_index(bytes.to_vec(), 0) {
        Ok(font) => Some(FaceEntry {
            bytes,
            font: FontArc::new(font),
        }),
        Err(err) => {
            tracing::warn!(
                family,
                weight,
                error = %err,
                "skipping unparseable bundled font face"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::{Font, PxScale, ScaleFont};

    /// Long enough to separate weights: advances differ somewhere in here for any pair
    /// of distinct faces (and identical faces always match).
    const PROBE: &str = "AaBbCcGgQqRrSs0123456789.,:;!?&@#%";

    /// Metrics signature of a face — per-glyph horizontal advance at 64px, in 1/64 px units.
    fn signature<F: Font>(font: &F) -> Vec<u32> {
        let scaled = font.as_scaled(PxScale::from(64.0));
        PROBE
            .chars()
            .map(|ch| {
                let id = font.glyph_id(ch);
                (scaled.h_advance(id) * 64.0).round().max(0.0) as u32
            })
            .collect()
    }

    fn db() -> FontDb {
        FontDb::bundled()
    }

    fn regular(db: &FontDb, family: &str) -> Vec<u32> {
        signature(&db.resolve(family, false).unwrap())
    }

    fn bold(db: &FontDb, family: &str) -> Vec<u32> {
        signature(&db.resolve(family, true).unwrap())
    }

    #[test]
    fn families_match_live_order() {
        assert_eq!(
            FontDb::families(),
            &[
                "Inter",
                "Lato",
                "Source Serif 4",
                "Playfair Display",
                "Oswald",
                "Archivo Black",
                "JetBrains Mono",
                "Fira Code",
                "Caveat",
                "Pacifico",
            ]
        );
    }

    #[test]
    fn face_count_is_fourteen() {
        let _ = db();
        assert_eq!(FontDb::face_count(), 14);
    }

    #[test]
    fn every_family_resolves_regular() {
        let db = db();
        for family in FontDb::families() {
            assert!(
                db.resolve(family, false).is_some(),
                "regular face missing for {family}"
            );
            assert!(
                db.resolve_shared(family, false).is_some(),
                "shared regular face missing for {family}"
            );
        }
    }

    #[test]
    fn every_face_has_glyph_a() {
        let db = db();
        for family in FontDb::families() {
            for bold in [false, true] {
                let font = db.resolve(family, bold);
                let font = font.as_ref().unwrap();
                assert_ne!(
                    font.glyph_id('A'),
                    ab_glyph::GlyphId(0),
                    "{family} (bold={bold}) has no 'A' glyph"
                );
            }
        }
    }

    #[test]
    fn bold_resolves_distinct_face() {
        let db = db();
        for family in ["Inter", "Lato", "Oswald", "Playfair Display"] {
            assert_ne!(
                regular(&db, family),
                bold(&db, family),
                "{family} bold must pick a distinct 700 face"
            );
        }
    }

    #[test]
    fn bold_falls_back_to_regular_for_single_weight_families() {
        let db = db();
        for family in [
            "Source Serif 4",
            "Archivo Black",
            "JetBrains Mono",
            "Fira Code",
            "Caveat",
            "Pacifico",
        ] {
            assert_eq!(
                regular(&db, family),
                bold(&db, family),
                "{family} ships no 700 face, so bold must fall back to regular"
            );
        }
    }

    #[test]
    fn unknown_family_falls_back_to_inter() {
        let db = db();
        let inter = regular(&db, "Inter");
        for unknown in ["", "Comic Sans MS", "not-a-family", "inter black"] {
            assert_eq!(
                regular(&db, unknown),
                inter,
                "fallback failed for {unknown:?}"
            );
        }
    }

    #[test]
    fn family_match_is_case_insensitive() {
        let db = db();
        let canonical = regular(&db, "Playfair Display");
        for variant in ["playfair display", "PLAYFAIR DISPLAY", "PlAyFaIr DiSpLaY"] {
            assert_eq!(
                regular(&db, variant),
                canonical,
                "match failed for {variant:?}"
            );
        }
    }

    #[test]
    fn shared_path_matches_owned_path() {
        let db = db();
        for family in FontDb::families() {
            for bold in [false, true] {
                let owned = db.resolve(family, bold);
                let shared = db.resolve_shared(family, bold);
                assert_eq!(
                    signature(&owned.unwrap()),
                    signature(&shared.unwrap()),
                    "{family} (bold={bold}): resolve and resolve_shared disagree"
                );
            }
        }
    }

    #[test]
    fn corrupt_face_bytes_are_skipped_not_panicked() {
        // Contract: a bad face is skipped with a warning, never a panic.
        assert!(parse_face(b"definitely not a TrueType file", "Broken", 400).is_none());
        assert!(parse_face(&[], "Empty", 400).is_none());
    }

    #[test]
    fn italic_shear_is_synthetic_oblique() {
        assert_eq!(ITALIC_SHEAR, 0.2);
    }
}
