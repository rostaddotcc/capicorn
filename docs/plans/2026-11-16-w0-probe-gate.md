# W0 — Probe Gate Implementation Plan

> **For Hermes:** goal/loop mission spec `docs/specs/2026-11-16-snagcraft-rust-rebuild.md` §5 W0.
> Nothing else starts until this gate passes. Probe code stays in `rust/probes/`.

**Goal:** prove (a) an egui editor spike (PNG load, arrow/text/rect, undo/redo, multi-pass
blur, classic toolbar) and (b) a transparent frameless egui viewport, both built + clippy-clean
on pn53, delivered as a runnable artifact for rostad's KDE Plasma Wayland box.

**Architecture:** fresh cargo workspace `rust/` (W1 extends it with crates/apps/xtask).
Two probe binaries, disjoint. Editor core is pure data + functions (sans-I/O, unit-tested);
egui shell is thin. Same scene code renders in-app and in headless kittest snapshots.

**Tech stack (versions verified on crates.io 2026-11-16):** egui/eframe 0.37.0 (default
features already include wgpu + wayland + x11), image 0.25.10, rfd 0.18.0,
egui_kittest 0.37.0 (dev, features wgpu+snapshot), tracing + tracing-subscriber.
All MIT/Apache-2.0 — allowlist-clean. rustc 1.100.0 on pn53, edition 2024, resolver 3.

**API facts verified from emilk/egui@main source (tmp/research/egui/), not guessed:**
- Transparency: `ViewportBuilder::with_transparent(true).with_decorations(false)` +
  override `App::clear_color(&self, _: &Visuals) -> [f32;4]` → `[0,0,0,0]` +
  transparent panel frames. wgpu Painter receives the transparent flag
  (wgpu_integration.rs:240); clear color comes from `app.clear_color` (:837).
- Drag: `ctx.send_viewport_cmd(ViewportCommand::StartDrag)` (Wayland-native via winit).
- Self-screenshot: `ctx.request_screenshot(|img: Arc<ColorImage>| …)` — works on wgpu
  (viewport.rs:1202, context.rs:4577). `EFRAME_SCREENSHOT_TO` is NOT implemented for
  wgpu (wgpu_integration.rs:143 asserts) — do not use it.
- kittest 0.37: `Harness::new_ui_state(|ui, state| …, state)`, `.run()`,
  `.snapshot(name)` (needs wgpu+snapshot features; saves tests/snapshots/<name>.png;
  bless with UPDATE_SNAPSHOTS=true). `new_egui` does not exist in 0.37.
- House rules: deny clippy unwrap/expect/panic/unimplemented/todo/unreachable in prod
  roots, `unsafe_code = forbid`, no println/eprintln outside tests (use tracing).

**Execution decision:** W0 built by the parent directly (two ~400-line probes, API
knowledge already verified in-context; briefing children costs more than it saves).
Parallel subagent tracks start at W1 per spec.

---

### Task 1: Workspace scaffold
**Files:** `rust/Cargo.toml`, `rust/.gitignore` (`/target`), `rust/probes/editor-spike/Cargo.toml`,
`rust/probes/viewport-probe/Cargo.toml`.
Workspace lints: `unsafe_code = "forbid"` + the clippy deny set; `[profile.dev.package."*"] opt-level = 2`.
**Verify:** `cargo metadata --manifest-path rust/Cargo.toml` parses.

### Task 2: Editor core (pure, TDD)
**Files:** `rust/probes/editor-spike/src/scene.rs` (+ unit tests inline).
- `Annotation::{Arrow, Rect, Text, BlurRegion}` serde-friendly plain data.
- `Doc { base: Arc<RgbaImage>, anns: Vec<Annotation> }`; `History` = undo/redo stacks of
  `(Arc<RgbaImage>, Vec<Annotation>)` snapshots (Arc-shared base; blur produces a new image).
- `gaussian_blur_region(img, rect, radius)`: 3-pass separable box blur (≈ gaussian),
  REPLACES region pixels, clamped to bounds, radius ≥ 1, no panic on hostile rects.
- `composite(doc) -> RgbaImage`: base + arrows (line + filled head) + rects + text
  (egui font raster is UI-side; CPU composite draws text via simple built-in bitmap font
  for export/selftest — spike-grade, real type engine lands W2).
- Tests first: undo/redo round-trip, blur variance reduction + uniform-region invariance +
  out-of-bounds rect safety, arrowhead geometry, composite pixel spot-checks.
**Verify:** `cargo test -p editor-spike` green.

### Task 3: Editor egui shell
**Files:** `rust/probes/editor-spike/src/main.rs`.
- CLI: `[path.png]`, `--selftest --shot-out <png>` (synthetic scene → frames →
  request_screenshot → save → exit), default synthetic image when no path.
- One-row classic toolbar: Select/Arrow/Text/Rect/Blur + Open/Save/Undo/Redo;
  accent #2266b3, radii 4–8, light theme, neutral grey chrome, tooltips with shortcuts.
- Canvas: fit-to-window image, painter overlays in image coords; drag-create arrow/rect/blur;
  click-place text with inline singleline TextEdit (no placeholder, Enter/blur commits);
  select tool drag-moves annotations; Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y; drag-drop PNG load;
  Save composites via scene::composite → rfd save dialog → PNG.
**Verify:** clippy -D warnings; launch test on pn53 session (timeout 12 → exit 124 = alive).

### Task 4: Headless visual gate (kittest)
**Files:** `rust/probes/editor-spike/tests/visual.rs`.
`Harness::new_ui_state` over the same `editor_ui` fn with a seeded annotated doc;
`.run()`; `.snapshot("editor_spike")`. Bless once (UPDATE_SNAPSHOTS=true), then vision-inspect
the PNG: toolbar one row, image + arrow + rect + text + blurred region visible.
**Verify:** `cargo test -p editor-spike --test visual` green; PNG vision-checked.

### Task 5: Viewport probe
**Files:** `rust/probes/viewport-probe/src/main.rs`.
292×64 frameless transparent pill, red Rec button, drag-anywhere via StartDrag,
on-screen report (session type, renderer, transparency requested), Esc quits,
`--selftest --shot-out` path like the editor. No always_on_top on Wayland (suite rule);
stacking behaviour is for rostad to observe on KWin.
**Verify:** clippy; launch test; selftest screenshot if the session allows.

### Task 6: Gate run + artifact + status
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` in `rust/`.
- Release build; package AppImage reusing `~/.cache/tauri/linuxdeploy-*` + AppRun
  (AppDir: binary + .desktop + icon from app-icon.png); fallback = tar.gz of binaries.
- Launch-test the packaged artifact (APPIMAGE_EXTRACT_AND_RUN=1, timeout 12 → 124).
- Append wake entry + measured reality to `docs/plans/rust-rebuild-status.md`; commit
  explicit paths (spec + status + plan + rust/); deliver artifact via MEDIA.
- KDE-only checks (real transparency against desktop, drag feel, stacking) are rostad's
  manual pass — state them explicitly in the delivery note.

## Appendix — measured reality

| Claim | Measured | What changed |
|---|---|---|
| egui main-branch source (fetched for API verification) matches released 0.37 | Released 0.37 differs: unified `egui::Panel` with `.show(ui, …)`, `eframe::App::ui` (not `update`), `scope_id` (not `id`), `DroppedFile` trait, `FontData.font: Blob`, `TextEdit::frame(Frame)`, ab_glyph `PxScale` + no-arg `ascent_unscaled()` | Code corrected against `~/.cargo/registry/…/egui-0.37.0` source; W1+ briefs cite the released crate, never main |
| `EFRAME_SCREENSHOT_TO` works with wgpu | Compile-time assert: wgpu backend not implemented | `ctx.request_screenshot` + egui_kittest used instead |
| Selftest screenshots run on pn53 | Locked GNOME = no frame callbacks = selftest stalls (exit 124, no PNG) | kittest snapshot is the pn53 visual gate; KDE manual pass covers live selftest |
| linuxdeploy `--plugin appimage --output appimage` | appimage is an OUTPUT plugin: `--plugin appimage` errors | Use `--output appimage` only |