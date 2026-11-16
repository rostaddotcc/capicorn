# SnagCraft MVP Implementation Plan

> **For Hermes:** execute with subagent-driven-development + delegation-tuning.
> Parallel wave 1 = tracks R / F / U with disjoint file ownership and locked contracts.

**Goal:** Ship a runnable SnagCraft .AppImage: full-screen capture → annotation editor
(crop, arrow, text, rect, blur, step, undo/redo) → copy to clipboard / save PNG, on
rostad's GNOME/Wayland workstation.

**Architecture:** Tauri 2 (Rust backend + Vite/TS frontend, Fabric.js editor).
Linux capture via ashpd portal; macOS/Win via xcap. See
`docs/specs/2026-11-14-snagcraft-design.md` for the full spec and locked contracts.

**Tech stack:** Rust stable, Tauri 2, ashpd, arboard, image, base64; Vite 6, TS 5, Fabric 6.

---

## Phase 0 — Environment & scaffold (parent, sequential)

1. Install Rust (rustup) + Tauri Linux deps (`libwebkit2gtk-4.1-dev`, …). [background]
2. Create public repo `rostaddotcc/snagcraft`, clone to `~/projects/snagcraft`. ✅
3. Write scaffold: package.json, vite/tsconfig, index.html, src-tauri (Cargo.toml,
   tauri.conf.json, capabilities, main.rs, lib.rs, stub modules), frontend stubs. ✅
4. `pnpm install`; generate icon set (`pnpm tauri icon`).
5. **PROBE GATE:** tiny Rust probe confirming ashpd Screenshot returns a PNG on this
   GNOME/Wayland session. Record result in the appendix; adjust capture brief if needed.
6. Commit scaffold; push.

## Phase 1 — Parallel implementation wave (3 subagents, disjoint files)

All briefs carry: ownership declaration, "do NOT run git/deploy", locked contract text
(spec §4), first-write-within-8-calls, explicit call budget + "STOP at budget, report
undone", and a resume changelog at `tmp/snagcraft/<track>-changelog.md`.

### Track R — Rust backend
**Files:** `src-tauri/src/capture.rs`, `clipboard.rs`, `commands.rs` (replace stubs).
- `capture.rs`: Linux `capture_full()` via ashpd Screenshot portal → read PNG URI →
  base64 data URL + dimensions (image crate). `#[cfg(not(linux))]` xcap path.
- `clipboard.rs`: decode PNG data URL → RGBA (image) → arboard `set_image`.
- `commands.rs`: implement `save_png` (tauri-plugin-dialog save + write bytes),
  wire `capture_screenshot`/`copy_png`/`get_version` per contract §4.1.
- Gate: `cargo build` (and `cargo test` for a data-url round-trip unit test).

### Track F — Annotation editor
**Files:** `src/editor/editor.ts`, `src/editor/tools.ts` (replace stub).
- Fabric.js canvas implementing the locked Editor API (§4.2): load/fit, crop,
  arrow, text, rect, blur/pixelate, step-number, undo/redo, flatten to PNG data URL.
- Gate: `tsc` typecheck clean; editor imports resolve.

### Track U — UI shell
**Files:** `src/ui/app.ts`, `src/main.ts`, `src/style.css`, `index.html` (replace stubs).
- Clean light/professional shell (rostad taste: minimalist, not boring; no terminal/DOS look).
- Capture button + PrintScreen hotkey → invoke capture → Editor.load.
- Toolbar wired to Editor.setTool; Copy/Save buttons → flatten → invoke commands.
- Status line for errors; uses ONLY the locked contracts (§4), not sibling internals.
- Gate: `tsc` + `vite build` clean.

## Phase 2 — Integration & build (parent)

1. Run full gates: `cargo build`, `tsc`, `vite build`. Fix cross-track wiring.
2. `pnpm tauri build` → AppImage (+ deb). Handle linuxdeploy quirks
   (`APPIMAGE_EXTRACT_AND_RUN=1`, `NO_STRIP=1`); fallback to manual AppImage if needed.
3. Launch the AppImage; verify capture → annotate → copy → save end-to-end.
4. Commit + push; tag `v0.1.0`; GitHub release with the AppImage attached.
5. Deliver the AppImage to rostad via MEDIA for feedback.

## Phase 3 — Iterate (subsequent turns)

Wave 2: recording (MP4/GIF) + pre-capture region overlay.
Wave 3: scrolling capture + OCR + local-AI auto-redaction.
Each wave: spec delta → plan → parallel subagents → build → AppImage → feedback.

---

## Appendix — measured reality

(Append one row per contradicted assumption: claim → measured → change.)

| Claim | Measured | Change |
|-------|----------|--------|
| ashpd `Uri::to_file_path()` exists | ashpd 0.14 `Uri` is a string wrapper (`as_str()` only) | R wrote `file_uri_to_path` (strip `file://` + percent-decode); probe example fixed |
| `FileDialogBuilder::blocking_save()` | method is `blocking_save_file()` | R corrected |
| Portal Screenshot returns a PNG when probed from the agent shell | **Screen is LOCKED** (`org.gnome.ScreenSaver.GetActive` = true). Portal method hangs; direct `org.gnome.Shell.Screenshot` = AccessDenied (GNOME 50 restricts that API to the portal). | Capture cannot be validated while locked. The portal is the sanctioned path; validate end-to-end once rostad unlocks and runs the AppImage. |