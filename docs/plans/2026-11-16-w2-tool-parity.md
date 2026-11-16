# W2 — Editor Tool Parity (13 tools, text engine, blur parity, crop)

> **For Hermes:** subagent-driven-development + delegation-tuning. W2a = crate tracks
> (this dispatch); W2b = engine commands + full UI (parent/child after W2a lands).
> Parity source of truth: `docs/plans/rust-rebuild-parity.md` (149 evidenced rows).

**Goal:** all 13 live tools modeled + rendered + drivable; blur/redaction parity
constants; real text engine with the 10 bundled OFL families; crop + bucket ops;
history depth 80 (live parity).

**Verified inputs (wake #4):** live tool ids/shortcuts/options from parity doc §1–2;
font assets decompressed from live woff2 → `rust/assets/fonts/*.ttf` (14 faces,
fontTools 4.61.1, OFL files alongside). Italic: live bundles NO italic faces —
browser synthesizes oblique; rust does the same via shear at render (ITALIC_SHEAR).
Fonts in-repo follows the live app's precedent (snagcraft is not a storytold repo;
craft-fonts rule is suite-specific) — recorded in ATTRIBUTION.md.

## W2a LOCKED CONTRACTS

Layer map unchanged. New doc fields/variants MUST extend format's no-rest guard
(same child owns doc+ops+format). serde(default) on new fields keeps v1 files loadable.

```rust
// doc (L1) — extends W1
pub enum ShapeKind { Rect, Ellipse, Line, Triangle, Star } // +Triangle (apex top-center,
// base bottom corners), +Star (normalized 96×91 5-point path from live tools.ts:619,
// scaled into rect — read the live path, it is rostad's own code)
#[derive(Clone,Copy,Debug,PartialEq,Serialize,Deserialize)]
pub struct Shadow { pub enabled: bool, pub color: Rgba, pub blur: u32 /*0..=60*/,
  pub offset_x: f32 /*-50..=50*/, pub offset_y: f32 }
impl Default for Shadow { disabled, color black, blur 4, offsets (0,4) }
// ADD `shadow: Shadow` (#[serde(default)]) to Arrow, Shape, Text, Step, Callout.
// ADD variants:
Magnify { id: ObjId, rect: Rect, zoom: f32 /*clamp 1.5..=10 at render*/ },
BrushStroke { id: ObjId, points: Vec<Point>, color: Rgba, width: f32 /*2..=44*/ },
impl Document {
  pub fn crop(&self, rect: Rect) -> Option<Document>;
  // clamp rect to base bounds (None if empty); base = imageops::crop_imm;
  // objects shifted by (-rect.x, -rect.y); objects whose bounds fall fully
  // outside dropped; next_id preserved; size = cropped dims.
}
// bounds()/shift()/hit_test()/set_id() cover the 2 new variants + shadow is inert
// for bounds (shadow does not extend hit bounds — live parity).

// ops (L2): MAX_UNDO 100 → 80 (live MAX_HISTORY, parity doc §4). Rename const
// MAX_HISTORY and keep behavior; update tests.

// format (L3): extend manifest_json/object_json for shadow fields + 2 new variants
// + 2 new ShapeKind values — NO `..` rest patterns anywhere (compile guard).
// load: serde(default) fills shadow on old files. Round-trip tests for all 10 kinds.

// algo (L2) — parity constants + new ops
pub const BLUR_RADIUS_MIN: u32 = 2; pub const BLUR_RADIUS_MAX: u32 = 60;
pub const BLUR_RADIUS_DEFAULT: u32 = 12; pub const SECURE_BLUR_MIN: u32 = 10;
pub const BLACKOUT_INK: [u8; 4] = [0x11, 0x18, 0x27, 255];
// gaussian_blur_region: clamp radius 2..=60 THEN raise <10 to 10 (redaction
// honesty, live blur.ts:24-25). UPDATE W1 tests that assumed radius 4/5 semantics.
// blackout_region: fill BLACKOUT_INK (not pure black).
pub fn flood_fill(img: &Surface, seed: Point, color: Rgba, tolerance: u8) -> Surface;
// scanline seed fill; match = per-channel max abs diff <= tolerance; bounded by
// image dims; out-of-bounds seed or zero dims → clone unchanged; cap visited pixels.
pub fn resize_region(img: &Surface, src: Rect, dst_w: u32, dst_h: u32) -> Surface;
// bilinear sample of src rect into dst dims (magnify lens); clamps; dst capped 4096².

// compose (L3) — parity rendering
// Spotlight: overlay rgba(8,12,18) at alpha 0.68 OUTSIDE rect (src_over — replaces
//   W1's multiply-by-(1-dim); keep `dim` field as intensity scale: alpha = 0.68*dim).
// Highlighter: MULTIPLY blend at alpha 0.36: out_c = dst_c*(1-0.36 + 0.36*ink_c/255).
// Blur: via algo (new constants flow through).
// Magnify: source rect = rect shrunk about its center by 1/zoom; resize_region to
//   rect dims; draw; rounded-rect border 2px #1c1c1e; zoom label "{zoom:.1}×"
//   bottom-right inside (font, white on dark chip).
// BrushStroke: round-cap polyline, dab spacing <= width/4 (no gaps at width 44).
// Triangle/Star outlines per doc contract.
// Arrow head length = strokeWidth*2 (live tools.ts:1014-1018 — read exact geometry).
// Shadow (Arrow/Shape/Text/Step/Callout with shadow.enabled): render object
//   silhouette (fills AND outlines — affectStroke parity) into a temp alpha mask,
//   box-blur the mask by shadow.blur (<=60), tint shadow.color, draw at
//   (offset_x, offset_y) BEHIND the object. Bounded allocations.
// Text: bold/italic still rendered by compose via the font it is GIVEN (font
//   selection lives in the text crate + engine/UI, W2b). Italic shear applied in
//   draw_text when a shear param is passed — extend composite signature:
pub fn composite<F: ab_glyph::Font>(doc: &Document, font: Option<&F>) -> Surface; // unchanged
pub fn composite_styled<F: ab_glyph::Font>(doc: &Document, fonts: &dyn Fn(&str, bool) -> Option<&F>, italic_shear: f32) -> Surface;
// W2b wires per-object family/bold via the callback; W2a keeps composite() working
// (single font, regular) and adds the styled path + shear support in draw_text.

// text (L2, was stub) — font engine
pub struct FontDb; // include_bytes! the 14 TTFs from rust/assets/fonts/
impl FontDb {
  pub fn bundled() -> FontDb;
  pub fn families() -> &'static [&'static str]; // Inter, Lato, Source Serif 4,
  // Playfair Display, Oswald, Archivo Black, JetBrains Mono, Fira Code, Caveat, Pacifico
  pub fn resolve(&self, family: &str, bold: bool) -> Option<ab_glyph::FontVec>;
  // unknown family → Inter → first available; bold without 700 face → regular.
  pub fn face_count() -> usize; // 14
}
pub const ITALIC_SHEAR: f32 = 0.2;
// Tests: all 10 families resolve regular; bold resolves for Inter/Lato/Oswald/
// Playfair; fallbacks; glyph coverage sanity (glyph_id('A') != .notdef).
```

## Tracks (W2a, parallel, disjoint)

- **T1 — doc + ops + format:** new variants/fields, crop, history 80, format guard
  extension + round-trips. Reads live tools.ts:619 (star path) for the normalized points.
- **T2 — algo + compose:** parity constants, flood_fill, resize_region, all new
  rendering + shadows + exact spotlight/highlighter/blackout behavior. Reads live
  blur.ts + tools.ts:1014 for head geometry. Updates W1 tests to parity semantics.
- **T3 — text crate:** FontDb over the prepared TTF assets (already on disk).
- **Parent (W2b, after landing):** engine commands (edit.crop, edit.bucket, shadow/
  magnify/brush params, blur default 12), ui-egui full 13-tool one-row toolbar +
  contextual properties rail (8-swatch palette, stroke 1–12, shape picker, blur mode
  selector, font family/size/bold/italic, shadow angle/distance/opacity, brush size
  2–44, magnify zoom 1.5–10), quick font/size bar while text editing, crop Enter/Esc
  UX, brush freehand, step auto-numbering, callout width→font-size; gates; AppImage;
  tick parity rows with evidence.

## Gates (W2 overall)

1. fmt/clippy -D warnings/tests/layers green; format guard still compile-breaking.
2. Headless snapshot: all 13 tools' objects render (vision-verified contact sheet).
3. Blur parity: radius<10 raised to 10 (test), blackout #111827 (pixel test),
   spotlight rgba(8,12,18,.68) (pixel test), highlighter multiply .36 (pixel test).
4. Crop/bucket command-level tests incl. hostile rects/seeds.
5. FontDb: 10 families, 14 faces, bold/italic paths.
6. UI: one-row toolbar at 1100px AND narrow widths; every tool creatable by pointer;
   shortcuts V C A T R B E G L H K S M route correctly (kittest or live probe).
7. AppImage launch + rostad KDE pass.

## Appendix — measured reality

(append rows as assumptions break)