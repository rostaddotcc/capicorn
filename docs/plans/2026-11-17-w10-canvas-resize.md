# W10 — Parity long-tail: New-canvas presets + Resize dialog

> **For Hermes:** post-W9 parity long-tail, SMALL scope (2 features). HARD
> BUDGET: stop and report at 70 tool calls even if incomplete. Write early,
> no vision-loop, run gates ONCE. Offline-verifiable, no rostad gate.

**Goal:** close parity rows 94 (New blank canvas Ctrl+N, 8 size presets) +
95 (Resize canvas dialog, Esc closes).

## CONTRACTS
### Feature 1 — New blank canvas presets (ui-egui)
Read ../src/ui/app.ts New-canvas dialog (grep "1280" / "New" / preset size
list — NOT SEED_PRESETS at :717 which are tool styles). Parity row 94 lists:
Default 1280×800, HD 1280×720, Full HD 1920×1080, QHD 2560×1440, 4K UHD
3840×2160, + square/custom (read live for the exact 8). 
- ui-egui: Ctrl+N + File→New… opens a New-canvas dialog: preset buttons
  (name + WxH) + custom width/height fields + Create. Create → engine
  doc.new {w,h} (exists). Dirty-guard if current doc has annotations (match
  live close-guard pattern). kittest: open dialog, click a preset → doc.new
  dispatched with that size; custom fields work. Snapshot w10_new_canvas.png.
### Feature 2 — Resize canvas dialog (engine + ui-egui)
Read ../src/ui/app.ts:1095-1101 (Resize… button → resize-pop "Resize image")
+ resizeImage(w,h) behaviour (does it scale annotations or just canvas? match).
- engine `doc.resize {w,h}` (native+web): resize the canvas; preserve/scale
  annotations per live behaviour (read live). Hostile-safe w/h clamp (1..=16384).
  Register (registry 44→45; REPORT automation drift-guard need, parent owns).
- ui-egui: toolbar Resize… button → dialog (width/height fields, current dims
  prefilled, Esc closes, Apply → doc.resize). kittest: open, change dims,
  Apply → doc.resize dispatched; Esc closes. Snapshot w10_resize.png.

## Gates
cargo test --workspace (green except automation drift-guard 44→45 — REPORT,
parent fixes), clippy --workspace --all-targets -D warnings, fmt, xtask layers,
web check. Tick parity 94/95; update gaps. NO git by child.

## HARD PROCESS RULES
- FIRST write within 6 calls. Budget 70 calls MAX — at 70 STOP, write journal,
  report done/undone immediately (do not exceed).
- No vision-loop: ONE snapshot check max. Verify via kittest assertions.
- Run cargo gates ONCE at the end. Journal: tmp/w10-journal.md.

## Appendix — measured reality