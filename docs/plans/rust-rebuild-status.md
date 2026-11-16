# SnagCraft Rust rebuild — status log

Spec: `docs/specs/2026-11-16-snagcraft-rust-rebuild.md`
Baseline: Tauri v1.5.2 (commit d25adb1). Rebuild workspace: `rust/` (not yet created).

## Wave status

| Wave | Scope | Status |
|------|-------|--------|
| W0 | egui editor spike + Wayland viewport probe | built + verified on pn53; KDE manual pass pending rostad |
| W1 | workspace scaffold, doc model, engine, capture port | core landed + verified (99 tests); desktop capture→editor built; KDE pass pending |
| W2 | editor tool parity | W2a landed (doc/ops/format/algo/text verified); engine W2b commands landed; full UI track in flight |
| W3 | shell parity (tray/hotkeys/widget/settings) | pending |
| W4 | recording, scrolling, OCR, timelapse | pending |
| W5 | CLI/JSON/MCP automation + WASM web | pending |
| W6 | packaging, docs, suite dossier | pending |

## Parity checklist

(to be generated from live v1.5.x source at W1 — tick with evidence)

## Wake log

### 2026-11-16 — spec written
- Full-Rust rebuild spec approved for goal/loop use; no code started yet.
- Inventory: ~9.2k lines Rust backend portable; ~12.2k lines TS editor/UI to rebuild.
- Open gates: relicense (GPL → MIT/Apache-2.0), external posts, cutover — all need rostad.

### 2026-11-16 — wake #1: W0 probe gate built and verified on pn53
- Architecture ground truth fetched to `tmp/research/photocraft/` + `tmp/research/suite/`
  (AGENTS.md, architecture.md, control-protocol.md, ui-design.md, house rules). egui API
  references in `tmp/research/egui/`.
- W0 plan: `docs/plans/2026-11-16-w0-probe-gate.md`. New workspace `rust/` (resolver 3,
  edition 2024, deny-lints + unsafe forbid, clippy.toml test exemptions).
- `rust/probes/editor-spike`: pure scene core (doc/history/3-pass blur/composite, 12 unit
  tests green) + thin egui shell (one-row classic toolbar, accent #2266b3, arrow/text/rect/
  blur tools, select+move, undo/redo, open/save PNG, drag-drop, inline text editing).
- `rust/probes/viewport-probe`: 292×64 transparent frameless pill, red Rec button,
  StartDrag anywhere, session info on-screen, Esc quits.
- Gates run: `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -D warnings`
  clean; 12/12 unit tests; headless kittest visual snapshot green AND vision-verified
  (`rust/probes/editor-spike/tests/snapshots/editor_spike.png` — toolbar, canvas, arrow,
  rect, text, blur all render correctly).
- Live launch on pn53 Wayland (Radeon 660M, RADV Vulkan): both binaries AND both AppImages
  stay alive (timeout exit 124). Only log noise: benign sctk_adwaita color-scheme portal
  timeout; arboard falls back to X11 clipboard (GNOME lacks ext-data-control — W3 note).
- Artifacts: `dist/w0-spike/SnagCraft_W0_Editor_Spike-x86_64.AppImage` (9.5 MB),
  `SnagCraft_W0_Viewport_Probe-x86_64.AppImage` (9.1 MB) — vs ~80–90 MB Tauri AppImage.
- W0 built by parent directly (two small probes; API verified in-context). Parallel
  subagent tracks start at W1.
- Remaining for W0 sign-off: rostad's KDE pass — desktop visible through pill corners,
  drag feel, stacking; editor flow capture→annotate→save on his box. `--selftest
  --shot-out` works only in an unlocked session (see appendix).

### 2026-11-16 — wake #2: W1 launched
- Fresh check: W0 committed b39da91 + pushed; no KDE feedback yet; W1 next.
- W1 plan with locked contracts: docs/plans/2026-11-16-w1-scaffold-engine-capture.md.
- Parent landed: full workspace scaffold (15 crates L0–L6 + 3 apps + xtask, 21 members),
  xtask layers (layer map + cycle detection + UI ban below L6) + ci, .cargo alias,
  packaging/ (desktop + icon + appimage.sh), .github/workflows/rust.yml (gates + AppImage
  artifact). ashpd 0.14.0 + xcap 0.9.10 + tokio/clap/cargo_metadata verified compiling.
- Parent landed L0 geom/color/raster + L1 doc (8 object kinds, serde, hit-test/shift) +
  L2 ops (bounded history): 20 unit tests green, clippy -D warnings clean, layers legal.
- Dispatched 4 parallel tracks (deleg_6ba30a31): C1 algo+compose, C2 format+io,
  D capture (ashpd Screenshot port; ScreenCast/PipeWire deferred to W4), P parity
  checklist from live v1.5.3 source → docs/plans/rust-rebuild-parity.md.
- Next: integrate children → engine (Session + command registry) → ui-egui shell
  (capture→editor) → apps/snagcraft + cli → gates → AppImage → deliver.

### 2026-11-16 — wake #3: W1 integrated end-to-end
- Children C1/C2/D/P all finished; parent verified artifacts: 39 crate tests green
  before integration, clippy clean, format compile-guard intact (no `..` rest).
- Parent landed: engine Session + 16-command registry (7 tests incl. adversarial
  params), ui-egui engine-driven shell (Capture button → threaded portal capture →
  editor; Select/Arrow/Rect/Text/Blur; Save native / Export PNG; undo/redo; drag-drop),
  apps/snagcraft (eframe, --capture/--selftest) + snagcraft-cli (capture/convert/
  commands/run — every registry command drivable headless).
- Two real bugs found + fixed by parent verification (not child self-reports):
  (1) compose text used ab_glyph raw font-unit metrics (ascent 932×scale → baseline
  off-canvas, silent text loss) → fixed with as_scaled()/ScaleFont pixel metrics;
  (2) CLI had no fonts because egui's own default features are EMPTY (default_fonts
  comes via eframe) → workspace egui now enables default_fonts explicitly.
  Regression probes: ui-egui render_probe tests + CLI text-export pixel check (417 px).
- Seeded demo scene now uses a detailed synthetic base (white-on-white blur was
  invisible in the first snapshot).
- Final gates: fmt ✓ clippy -D warnings ✓ 99 tests / 0 failed / 1 ignored (live
  portal, locked host) ✓ xtask layers 21 crates ✓. Headless w1_shell snapshot
  vision-verified: Capture button, 5 tools, arrow+rect+text+blur all render.
- Desktop release binary + AppImage (10.3 MB) launch-tested on pn53 Wayland (exit 124).
  CLI smoke: commands lists 16; convert PNG→JPEG→PNG round-trip; run edit.add +
  export with text verified pixel-wise.
- ATTRIBUTION.md added (icon original; no committed fonts; dep allowlist check).
- Parity checklist: docs/plans/rust-rebuild-parity.md — 149 evidence-backed rows,
  14 sections, baseline v1.5.3 @ 3cec2ee. W1 ticks: capture→editor flow, 8 object
  kinds, undo/redo, PNG/JPEG/WebP export, native format, CLI automation.
- Remaining W1 sign-off: rostad's KDE pass — live portal capture (unlocked),
  annotate → save/export under ten seconds. W2 next: full 13-tool parity.
- Post-batch follow-up (child reports arrived after integration; all matched the
  verified artifacts): confirmed C1's spotlight semantics (dim = darkening amount,
  factor 1−dim — matches contract intent); dropped capture's unused geom dep;
  hardened format::load to reject manifest-size/base-image mismatch (+tamper test,
  format now 11 tests). C2's WebPEncoder::new_lossless deviation accepted (image
  0.25 API). W2 inputs from parity doc: 5 shape variants (triangle/star missing in
  rust ShapeKind), live MAX_HISTORY=80, SECURE_BLUR_MIN=10, blackout #111827,
  spotlight rgba(8,12,18,.68), highlighter α .36, shortcuts V C A T R B E G L H K S M.

### 2026-11-16 — wake #4: W2 launched (tool parity)
- Fresh check: W1 stable at 942597f, no KDE feedback yet → next item W2.
- W2 plan: docs/plans/2026-11-16-w2-tool-parity.md (W2a crate tracks now, W2b
  engine+UI after). Parity inputs from checklist §1–2: 13 tools, 5 shape variants,
  blur radius 2–60 default 12 + SECURE_BLUR_MIN 10, blackout #111827, spotlight
  rgba(8,12,18,.68), highlighter multiply .36, shadow angle×distance→offsets,
  history 80, brush 2–44, magnify 1.5–10×, callout width→font-size.
- Font assets prepared parent-side: 14 OFL TTFs decompressed from the live app's
  woff2 (fontTools 4.61.1) → rust/assets/fonts/ + OFL texts; ATTRIBUTION updated.
  Italic = synthetic shear 0.2 (live bundles no italic faces either).
- Manifests pre-wired + compile-verified: doc +image (crop_imm), text +ab_glyph.
- Dispatched W2a (deleg_0688f850): T1 doc/ops/format (Shadow, Triangle/Star,
  Magnify/BrushStroke, crop, history 80, guard extension), T2 algo/compose
  (parity constants, flood_fill, resize_region, exact blends, shadows, styled
  composite + italic shear), T3 text FontDb (10 families, 14 faces).
- Next: verify T1–T3 artifacts → W2b engine commands + full 13-tool UI → gates →
  AppImage → tick parity rows.

### 2026-11-16 — wake #5: W2a verified, engine W2b landed, UI dispatched
- T1 (doc/ops/format) + T3 (text) finished; T2 (algo/compose) in final verification.
- Parent verified: doc 19 tests, format 14, ops 11, text 8 (live family order,
  14 faces, bold fallbacks, case-insensitive) — clippy -D warnings clean on all.
- Contract gap caught parent-side: Text/Callout lacked `family` (live TextStyle
  has 10 families). T1 finished before the steer landed → parent applied the
  family extension (serde default "Inter") to doc + format guard + 6 test literals.
  T2 consumed its steer: composite_styled now passes (obj.family, obj.bold) and
  applies italic shear per object.
- Engine W2b (parent): edit.crop + edit.bucket commands (18 total), blur default
  radius 12 clamp 2–60 (parity), render() switched to composite_styled with a
  Session-cached FontDb face table (resolve_shared/FontArc) — bundled OFL fonts
  now identical across UI/CLI/export; set_font removed (ui-egui + CLI updated,
  CLI egui dep dropped). 11 engine tests green incl. crop/bucket undo, hostile
  seeds, FontDb text pixel probe.
- Dispatched full 13-tool UI track (deleg_3ecf00d3): one-row toolbar + contextual
  properties rail + crop/brush/step/callout/magnify/bucket UX + quick font bar.
- Next: T2 + UI land → full gates → vision contact sheet → AppImage → tick parity.
- W2a batch closed: T2 final artifacts parent-verified (algo 22 + compose 34 tests,
  engine 11 re-run against final compose — all green, clippy clean, layers legal).
  Accepted deviations (cosmetic, logged for W2 polish/W6 gaps): spotlight hole +
  highlighter are hard rects (live rounds corners); shadow blur = one box pass
  (live canvas shadowBlur sigma). Arrow head uses the EXACT live formula
  (thickness=w*2, headLen=max(16,thickness*1.7), headHalf=max(9,thickness)).
  T3 contract conflict resolved well: ab_glyph FontVec is NOT Clone →
  resolve_shared/FontArc added as the cheap path (engine already uses it).
  T1 added sanitize() on add() + format::load (shadow clamps, brush point cap,
  non-finite point skip) — extra hostile-input hardening beyond contract.

### 2026-11-16 — wake #6: W2 polish while UI builds
- UI track (deleg_3ecf00d3) still running — lib.rs 714 → 1896 lines mid-build.
- Parent closed the two logged cosmetic parity gaps in compose (no sibling owns
  it now): spotlight hole radius min(12,min(w,h)/6) + highlighter band radius
  min(8,min(w,h)/4) with ≤0.5 hard-rect fallback (exact live blur.ts formulas,
  SDF coverage AA), shadow blur 3 box passes ≈ gaussian. 2 corner-coverage
  regression tests; 36 compose tests green, clippy clean. Committed c3a1a94.
- Parity checklist: 11 rows ticked W1/W2 (blur modes/constants/secure-min/
  blends, history 80 + reset-on-new).
- Next: UI lands → full gates + vision contact sheet + AppImage + W2 sign-off.

### 2026-11-16 — wake #7: UI pre-verification + step-number bug fix
- UI track still running (final verification); snapshots w2_shell + w2_shell_narrow
  blessed by child. Parent vision + pixel probes:
  * Shell structure PASS: exactly 13 icon tools on ONE row + Capture/Open/Save/
    Export/Undo/Redo; contextual rail (shape picker, 8-swatch palette, border,
    shadow angle/distance/opacity/blur); narrow width keeps one row with
    horizontal ScrollArea (tools 11–13 scroll-reachable, code-verified).
  * Vision flagged spotlight/blur/magnify as broken — pixel probes DISPROVED it
    (spotlight α=0.306 exact, blur smoothed, magnify shows zoomed content);
    vision misread the synthetic checker base as unflattened alpha.
  * REAL bug found: step-badge numbers invisible — compose asks fonts("", true)
    and the engine lookup was exact-match-only → None. Fixed: fallback chain
    exact → Inter → same-weight → first (mirrors FontDb::resolve). Regression
    test counts glyph pixels strictly inside the badge (first version
    false-passed on the white base — caught and fixed). 12 engine tests green.
- Steered UI child: re-bless both snapshots with the fixed engine; do NOT
  "fix" the proven-correct renderers. Committed f20cd2e + pushed.
- Next: UI lands → full gates + AppImage + W2 sign-off + delivery.

## Appendix — measured reality

| Claim | Measured | What changed |
|---|---|---|
| ab_glyph `ascent_unscaled() * scale` gives pixel ascent (W0 spike pattern) | Returns RAW font units (Ubuntu-Light ascent=932, upem 1000) → baseline 26 000 px off-canvas → silent text loss in CPU composite | compose uses `font.as_scaled(scale).ascent()` / `.h_advance(gid)` (ScaleFont trait); W0 spike export had the same latent bug (its on-screen text came from egui painter, which masked it) |
| egui = "0.37" bundles fonts by default | egui's own `default = []`; `default_fonts` is enabled via eframe — CLI (egui-only) got EMPTY FontDefinitions | workspace egui enables `default_fonts` explicitly; CLI text export verified pixel-wise |
| Child "tests green" claims imply visual correctness | compose passed 7 unit tests yet rendered no text (no font-present pixel test existed) | parent vision + pixel probes are mandatory on render paths; added render_probe regression tests |
| Blur visible in seeded snapshot | Gaussian blur on a pure-white base is invisible | seeded() loads a synthetic gradient base (load_surface) |
| photocraft layering forbids ALL same-layer deps | L0 foundation (raster→geom/color) needs same-layer edges | xtask layers allows same-layer edges but rejects cycles (DFS) + any upward edge; UI ban below L6 unchanged |
| egui main-branch docs match crates.io 0.37 | 0.37 renamed APIs: `Panel::top/bottom` (unified, `.show(ui,…)` not `.show(ctx,…)`), `eframe::App::ui` replaces `update`, `Ui::id`→`scope_id`, `Id::new` deprecated, `DroppedFile` is a trait (`.path()`), `FontData.font` is a `Blob`, `TextEdit::frame` takes `Frame` not bool, ab_glyph `Scale`→`PxScale`, `ascent_unscaled()` takes no args | All fixed against the vendored 0.37 registry source; briefs for W1+ must cite 0.37, not main |
| `EFRAME_SCREENSHOT_TO` usable for headless shots | Asserts "not yet implemented for wgpu backend" (wgpu_integration.rs:143) | Use `ctx.request_screenshot(cb)` (wgpu-supported) or egui_kittest offscreen |
| `--selftest` screenshots on pn53 | Locked GNOME session delivers no frame callbacks → frame counter never advances → no shot, no auto-exit (timeout 124) | kittest headless snapshot is the pn53 visual gate; selftest expected to work on unlocked KDE |
| kittest `Harness::new_egui` | Does not exist in 0.37; use `Harness::builder().with_size(…).build_ui_state(\|ui,state\|…, state)` | Test written against the real API |
