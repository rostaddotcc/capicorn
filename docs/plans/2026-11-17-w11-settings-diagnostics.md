# W11 — Parity long-tail: settings cluster + diagnostics + PrintScreen

> **For Hermes:** post-W10 parity long-tail. HARD BUDGET 75 calls — stop and
> report at budget. Write early, no vision-loop, gates ONCE. Many settings
> fields partly exist — COMPLETE them, don't duplicate. No rostad gate.

**Goal:** close parity rows 90/225/226/227/228/231/233/235/237/242 (settings)
+ 155 (copy diagnostics) + 133/262/263 (PrintScreen in-app fallback).

## CONTRACTS (read live src/ui/settings.ts + current io::settings first)
### Settings fields + UI (io::settings + ui-egui settings panel)
Complete these (add field if missing, serde default + validate + round-trip
test; wire the settings-panel row):
- interval capture period (default 30s) — timelapse period setting (row 225)
- microphone source (default None) — dropdown from audio.sources (row 226)
- system audio / output monitor (default off) — toggle (row 227)
- enable global hotkeys (default off) — toggle gating platform hotkey
  registration (row 228); "Register hotkeys" button + status line (row 231)
- recording format MP4/GIF (default MP4) — dropdown (row 235); used by
  record.start default
- separate recording folder splitFolders (default off) + its own Change/Reset
  (rows 90/233) — recording save dir override
- filename pattern + token hint ({date}/{time}/{datetime}) + LIVE PREVIEW
  (row 237) — preview uses io::build_filename (W9)
### Copy diagnostics (engine + ui-egui) [rows 155/242]
- engine `diagnostics.copy` (native): gather portal log (~/.cache/snagcraft-
  portal.log tail) + last per-take .rec.log tail + version/OS info → copy to
  clipboard (engine clipboard.copy_text) → {copied, bytes}. Read live
  commands.rs:457-501 for the content. ui-egui Settings→Troubleshooting
  "Copy diagnostics" button → diagnostics.copy → status.
### PrintScreen in-app fallback (ui-egui) [rows 133/262/263]
- Key handler: PrintScreen → capture full; Shift+PrintScreen → capture region
  (in-app fallback when global hotkeys off/unavailable). Match live
  app.ts:5128-5132. kittest: key dispatches capture.full / capture.region.

## Gates
cargo test --workspace --no-fail-fast (green except automation drift-guard if
registry changes — REPORT, parent owns), clippy --workspace --all-targets
-D warnings, fmt, xtask layers, web check. Re-bless shifted snapshots with
UPDATE_SNAPSHOTS=1 --no-fail-fast (kittest.toml max_failed_pixels=64 exists).
Tick parity rows; update gaps. NO git by child.

## HARD PROCESS RULES
- FIRST write within 6 calls. Budget 75 MAX — at 75 STOP, journal, report.
- No vision-loop (ONE snapshot check max). Verify via kittest assertions.
- Gates ONCE at end. Journal: tmp/w11-journal.md.

## Appendix — measured reality