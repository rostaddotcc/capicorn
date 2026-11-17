# W12 — Parity long-tail: shortcuts overlay + About + editor residuals

> **For Hermes:** post-W11 parity long-tail (final implementable cluster).
> HARD BUDGET 75 calls — stop+report at budget. Write early, no vision-loop,
> gates ONCE. Trim is rostad-gated (NOT in live v1.5.3) — leave pending.

**Goal:** close parity rows — shortcuts help overlay (? key, lists Ctrl+scroll
zoom / Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S / Delete / ?), About modal
(File→About, logo+name+version+description+repo, Esc/OK close), editor
residuals (text caption commit on settle, crop shading excluded from
history/export, dirty-state from history depth gates Save), ffmpeg-absent
actionable message.

## CONTRACTS (read live src/ui/app.ts + src/editor/editor.ts first)
### Shortcuts help overlay (ui-egui)
- `?` key (and a toolbar/menu entry) opens a Shortcuts modal listing the
  bindings: Ctrl+scroll zoom, Ctrl+N new, Ctrl+O open, Ctrl+S save,
  Ctrl+Shift+S save as, Delete/Backspace delete, Ctrl+, settings, Ctrl+. stop
  timelapse, Ctrl+D duplicate, Ctrl+C/V copy/paste, ? help, Esc close. Verify
  which are ALREADY wired (many are from W2–W11) — wire any missing (Ctrl+scroll
  zoom, Ctrl+O if absent), and ensure the sheet matches reality. Esc/OK closes.
  kittest: ? opens overlay, lists bindings, Esc closes. Snapshot w12_shortcuts.png.
### About modal (ui-egui + engine)
- engine `app.version` (or session.info already has version?) → About modal:
  logo mark (snagcraft.png), name "SnagCraft", version (env!("CARGO_PKG_VERSION")
  or engine), one-line description, repo URL. File→About… (menu shows version
  as hint). Esc/OK close. kittest: open About, shows name+version, Esc closes.
  Snapshot w12_about.png.
### Editor residuals (engine/ui-egui)
- Text caption commit on settle (not per keystroke): text edits commit to
  history on blur/Enter, not every keystroke (read editor.ts:1178). Verify/fix.
- Crop shading (editor-only decoration) excluded from history + export: the
  crop overlay dim must not bake into export or count as a history step. Verify.
- Dirty-state from history depth gates Save: doc_dirty derived from history
  (unsaved steps), Save enabled only when dirty. Verify/fix.
### ffmpeg actionable message (engine/recording)
- When ffmpeg absent, record.start/timelapse.assemble return an actionable
  error ("Install ffmpeg: sudo apt install ffmpeg") not a raw spawn error.
  Verify the existing Io error surfaces this; improve if generic.

## Gates
cargo test --workspace --no-fail-fast (green except automation drift-guard if
registry changes — REPORT, parent owns), clippy --workspace --all-targets
-D warnings, fmt --all (RUN IT), xtask layers, web check. Re-bless shifted
snapshots UPDATE_SNAPSHOTS=1 --no-fail-fast. For off-screen settings/widgets in
kittest use click_accesskit (coordinate .click misses scrolled widgets).
Tick parity rows; update gaps. NO git by child.

## HARD PROCESS RULES
- FIRST write within 6 calls. Budget 75 MAX — at 75 STOP, journal, report.
- No vision-loop (ONE snapshot check max). Gates ONCE. Journal tmp/w12-journal.md.

## Appendix — measured reality