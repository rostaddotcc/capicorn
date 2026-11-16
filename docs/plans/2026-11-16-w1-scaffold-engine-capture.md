# W1 — Scaffold, Doc Model, Engine, Capture Port — Implementation Plan

> **For Hermes:** execute via subagent-driven-development + delegation-tuning.
> Parent owns scaffold/manifests/xtask/CI/engine/ui-egui/apps; children own disjoint
> crate interiors against the LOCKED CONTRACTS below. Children never run git.

**Goal:** `rust/` grows the full L0–L6 crate layering + apps + xtask; a real doc model,
command-registry engine and portal capture backend land; the desktop app runs
capture → editor shows image; AppImage builds in CI; exact parity checklist written
from live v1.5.x source.

**Verified inputs (2026-11-16, wake #2):**
- Live tools (src/editor/tools.ts ids): select, shape (rect/ellipse/line variants),
  blur, spotlight, highlighter, magnify, arrow, text, step, callout, crop, bucket, brush.
- Capture port source: `src-tauri/src/capture.rs` (284 lines, ashpd 0.14 Screenshot +
  xcap 0.9 + file_uri_to_path/percent_decode + tests). ScreenCast window/monitor
  (`portal_capture.rs`, PipeWire) is W4 — NOT W1.
- ashpd 0.14 API verified (tauri-desktop-app › wayland-capture.md): `Screenshot::request()
  .interactive(false).send().await?` → `resp.response()?` → `shot.uri().as_str()`;
  Uri has NO to_file_path. Pin ashpd 0.14, features screenshot+tokio.
- egui 0.37 API: see snagcraft-rust-rebuild skill (released crate ≠ GitHub main).
- child_timeout_seconds=3600, max_concurrent_children=10 already configured.

**W1 scope discipline:** doc enum carries the objects W1 renders (Arrow, Shape, Text,
Blur, Step, Highlighter, Spotlight, Callout). Magnify/Brush/Bucket/Crop land W2 with the
tool-parity wave. text/vector/automation crates + snagcraft-web app are compiling stubs.
WebP encode is lossless via `image` (lossy webp = documented parity gap, W2 decision).

---

## LOCKED CONTRACTS (identical text in every child brief)

Layer map: L0 geom,color,raster · L1 doc · L2 ops,algo,text,vector · L3 compose,format ·
L4 io,capture · L5 engine · L6 ui-egui,automation · L7 apps,xtask.
Crate names `snagcraft-<dir>`. All crates: `[lints] workspace = true`, root deny attribute
`#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`,
no unsafe, no println/eprintln outside tests (tracing), errors via thiserror, hostile-input
bounds everywhere. serde derive on all doc data.

```rust
// snagcraft-geom (L0) — serde on all types
pub struct Point { pub x: f32, pub y: f32 }
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }
impl Rect { new(x,y,w,h); from_corners(a: Point, b: Point) -> Rect /*normalized*/;
  contains(p: Point) -> bool; is_finite() -> bool;
  clamp_to(w: u32, h: u32) -> Option<(u32,u32,u32,u32)> /*x0,y0,x1,y1 integer bounds*/ }
pub struct Size { pub w: u32, pub h: u32 }

// snagcraft-color (L0)
pub struct Rgba { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }
impl Rgba { const fn new(r,g,b,a); to_array() -> [u8;4]; from_array([u8;4]) -> Rgba;
  src_over(self, dst: Rgba) -> Rgba /*standard source-over, straight alpha*/ }

// snagcraft-raster (L0)
pub type Surface = image::RgbaImage;
pub const MAX_DIM: u32 = 16_384;
pub fn filled(size: Size, color: Rgba) -> Surface;

// snagcraft-doc (L1)
pub type ObjId = u64;
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)] pub enum ShapeKind { Rect, Ellipse, Line }
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)] pub enum BlurMode { Gaussian, Pixelate, Blackout }
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)] #[serde(tag="kind")]
pub enum AnnotObject {
  Arrow { id: ObjId, from: Point, to: Point, color: Rgba, width: f32 },
  Shape { id: ObjId, shape: ShapeKind, rect: Rect, color: Rgba, width: f32 }, // Line = rect diagonal (x,y)->(x+w,y+h)
  Text { id: ObjId, pos: Point, text: String, color: Rgba, size: f32, bold: bool, italic: bool },
  Blur { id: ObjId, rect: Rect, mode: BlurMode, radius: u32 }, // NON-destructive object; compose applies it; export bakes
  Step { id: ObjId, pos: Point, number: u32, color: Rgba, radius: f32 },
  Highlighter { id: ObjId, rect: Rect, color: Rgba }, // semi-transparent band, src_over
  Spotlight { id: ObjId, rect: Rect, dim: f32 }, // darken OUTSIDE rect by dim (0..1)
  Callout { id: ObjId, rect: Rect, text: String, color: Rgba, size: f32 }, // rounded outline + centered text (tail = W2)
}
pub struct Document { pub size: Size, pub base: Arc<Surface>, pub objects: Vec<AnnotObject>, pub next_id: ObjId }
impl Document { new(base: Surface) -> Document /*size from image, next_id=1*/;
  white(size: Size) -> Document; add(obj: AnnotObject) -> ObjId /*assigns next_id, pushes, returns id*/;
  remove(id) -> bool; get(id) -> Option<&AnnotObject>; get_mut(id) -> Option<&mut AnnotObject>;
  shift(id, dx: f32, dy: f32) -> bool /*moves any object's position fields*/;
  hit_test(p: Point) -> Option<ObjId> /*topmost bounds hit*/; bounds(obj) -> Rect }

// snagcraft-ops (L2)
pub const MAX_UNDO: usize = 100;
pub struct History; // Default; bounded undo/redo stacks of Document clones (Arc-shared base)
impl History { record(&mut self, before: &Document); undo(&mut self, current: &Document) -> Option<Document>;
  redo(&mut self, current: &Document) -> Option<Document>; can_undo() -> bool; can_redo() -> bool; clear() }

// snagcraft-algo (L2)
pub fn gaussian_blur_region(img: &mut Surface, rect: Rect, radius: u32) -> bool; // 3-pass separable box ≈ gaussian, radius clamp 1..=64, REPLACES region, clamped reads, false if nothing changed
pub fn pixelate_region(img: &mut Surface, rect: Rect, block: u32) -> bool; // block clamp 2..=64, block-average color fill
pub fn blackout_region(img: &mut Surface, rect: Rect) -> bool; // solid black fill (irreversible redaction)
pub fn apply_blur(img: &Surface, rect: Rect, mode: BlurMode, radius: u32) -> Surface; // clone + dispatch

// snagcraft-compose (L3)
pub fn composite<F: ab_glyph::Font>(doc: &Document, font: Option<&F>) -> Surface;
// base clone → objects in vec order: Blur applies algo::apply_blur to the working image;
// Spotlight dims outside; others draw over. Text/Callout text via ab_glyph (bold/italic
// flags stored but rendered regular in W1 — documented gap). No font → text skipped.

// snagcraft-format (L3)
pub const FORMAT_VERSION: u32 = 1;
pub fn save(doc: &Document) -> Result<Vec<u8>, FormatError>; // JSON {version, size, next_id, objects, base_png: base64}
pub fn load(bytes: &[u8]) -> Result<Document, FormatError>;
// COMPILE GUARD: doc→manifest conversion destructures Document with NO `..` rest pattern,
// and every AnnotObject match arm binds ALL fields explicitly (no `..`). Adding a doc field
// or object field MUST break compilation here. Round-trip test: save→load == original.

// snagcraft-io (L4)
pub enum ImageFormat { Png, Jpeg, WebP }
pub fn decode(bytes: &[u8]) -> Result<Surface, IoError>; // sniff via image::guess_format
pub fn encode(img: &Surface, format: ImageFormat, quality: u8) -> Result<Vec<u8>, IoError>; // jpeg quality 1..=100; png/webp lossless (quality ignored)
pub fn open(path: &Path) -> Result<Surface, IoError>; // reject > MAX_DIM
pub fn save(img: &Surface, path: &Path, format: ImageFormat, quality: u8) -> Result<(), IoError>;

// snagcraft-capture (L4)
pub enum CaptureError { Portal(String), Timeout, Io(String), Unsupported(String) } // thiserror
pub async fn capture_full() -> Result<Surface, CaptureError>;
// linux: ashpd 0.14 Screenshot::request().interactive(false).send() wrapped in
//   tokio::time::timeout(15s); resp.response() → uri().as_str() → file_uri_to_path →
//   fs::read → io::decode. non-linux: xcap 0.9 Monitor::from_point(0,0).capture_image().
pub fn capture_full_blocking() -> Result<Surface, CaptureError>; // own tokio current-thread runtime
pub fn file_uri_to_path(uri: &str) -> Result<PathBuf, CaptureError>; // port from src-tauri/src/capture.rs incl. percent_decode + its unit tests
```

## Tracks

- **Parent (before dispatch):** workspace manifests for ALL crates/apps/xtask with stub
  libs that compile; workspace deps added + `cargo check` green (ashpd/xcap/tokio/thiserror/
  base64/cargo_metadata/clap verified parent-side). Contracts pasted into crate lib.rs docs.
- **Child B — doc+ops:** fill crates/doc + crates/ops per contracts, TDD (serde round-trip,
  add/remove/shift/hit_test, history bounds + undo/redo incl. base-restore).
- **Child C — algo+compose+format+io:** fill 4 crates per contracts. Port gaussian blur +
  composite + arrow geometry from rust/probes/editor-spike/src/scene.rs (own W0 code).
  TDD: blur variance/uniform/hostile-rect tests (port), pixelate/blackout tests, composite
  pixel spot-checks per object kind, format round-trip + guard, io encode/decode round-trips.
- **Child D — capture:** fill crates/capture per contracts; port file_uri_to_path +
  percent_decode + tests from src-tauri/src/capture.rs; ashpd path with timeout; xcap
  target-gated; offline tests only (live portal test #[ignore] — pn53 screen locked).
- **Child P — parity checklist (READ-ONLY):** enumerate every feature/shortcut/setting from
  live src/editor/tools.ts, src/editor/editor.ts, src/ui/app.ts, CHANGELOG.md → write
  docs/plans/rust-rebuild-parity.md (checkbox table: feature | v1.5.x evidence file:line |
  rust status). Never edit code files.
- **Parent (after children):** engine (Session + registry + commands), ui-egui (spike shell
  re-driven through engine; Capture button), apps/snagcraft + snagcraft-cli, xtask layers/ci,
  .github/workflows/rust.yml (fmt/clippy/test/layers + AppImage artifact), integrate,
  full gates, launch test, AppImage, status log, commit, deliver.

## Gates

1. `cargo xtask layers` green (no egui/eframe/winit/rfd below L6; layer order enforced).
2. fmt + clippy -D warnings + full test suite green in rust/.
3. Desktop app: launch on pn53 (exit 124), headless kittest snapshot of the new shell
   vision-verified; capture button present (live portal capture validated by rostad unlocked).
4. CLI: `snagcraft-cli commands` lists registry; `convert` round-trips a PNG→JPEG→PNG.
5. Parity checklist exists with file:line evidence; W1 items ticked.
6. CI workflow builds AppImage artifact on ubuntu-latest.

## Appendix — measured reality

(append rows as assumptions break)