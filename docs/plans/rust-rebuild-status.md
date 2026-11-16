# SnagCraft Rust rebuild — status log

Spec: `docs/specs/2026-11-16-snagcraft-rust-rebuild.md`
Baseline: Tauri v1.5.2 (commit d25adb1). Rebuild workspace: `rust/` (not yet created).

## Wave status

| Wave | Scope | Status |
|------|-------|--------|
| W0 | egui editor spike + Wayland viewport probe | built + verified on pn53; KDE manual pass pending rostad |
| W1 | workspace scaffold, doc model, engine, capture port | pending |
| W2 | editor tool parity | pending |
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

## Appendix — measured reality

| Claim | Measured | What changed |
|---|---|---|
| egui main-branch docs match crates.io 0.37 | 0.37 renamed APIs: `Panel::top/bottom` (unified, `.show(ui,…)` not `.show(ctx,…)`), `eframe::App::ui` replaces `update`, `Ui::id`→`scope_id`, `Id::new` deprecated, `DroppedFile` is a trait (`.path()`), `FontData.font` is a `Blob`, `TextEdit::frame` takes `Frame` not bool, ab_glyph `Scale`→`PxScale`, `ascent_unscaled()` takes no args | All fixed against the vendored 0.37 registry source; briefs for W1+ must cite 0.37, not main |
| `EFRAME_SCREENSHOT_TO` usable for headless shots | Asserts "not yet implemented for wgpu backend" (wgpu_integration.rs:143) | Use `ctx.request_screenshot(cb)` (wgpu-supported) or egui_kittest offscreen |
| `--selftest` screenshots on pn53 | Locked GNOME session delivers no frame callbacks → frame counter never advances → no shot, no auto-exit (timeout 124) | kittest headless snapshot is the pn53 visual gate; selftest expected to work on unlocked KDE |
| kittest `Harness::new_egui` | Does not exist in 0.37; use `Harness::builder().with_size(…).build_ui_state(\|ui,state\|…, state)` | Test written against the real API |
