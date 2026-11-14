# Capicorn Wave 4 (v0.3.0) — dropdown, dark mode, bucket, settings & auto-save

> rostad's explicit requirements (all mandatory):
> 1. Capture mode = a REAL dropdown: click the caret in the Capture button → menu appears
>    (Full screen / Select region); closes on select/outside/Esc. Not always-open.
> 2. Dark mode (toggle + persist + full theme).
> 3. Remove the "INK" label text (implied).
> 4. Add a bucket (flood-fill) tool.
> 5. Settings for where images are saved (folder picker).
> 6. Auto-save EVERY capture to the save dir.
> 7. Ctrl+S / Save saves an edited COPY when changes were made.
> Plus: persist settings; polished light+dark UI, custom scrollbars, micro-interactions.

## Locked contracts

### Tauri commands (R) — existing kept, ADD:
- `default_save_dir() -> String` (picture dir / Capicorn, cross-platform via tauri path API)
- `pick_save_dir() -> String` (native folder dialog; cancel → Err("cancelled"))
- `save_png_to_dir(dataUrl, dir, filename) -> String` (create dir, validate+write PNG, return path)
- existing: capture_screenshot, set_fullscreen, copy_png, get_version, save_png.

### Editor (E) — existing API kept, ADD:
- `ToolId` gains `"bucket"`.
- Bucket = non-destructive flood-fill overlay annotation (tolerance fill from click, ink color).
- `isDirty(): boolean` (true once annotations/crop changed since load; cursor > 0).

### Frontend (U) — owns app.ts, style.css, main.ts, index.html, settings.ts(new)
- Theme via `<html data-theme="light|dark">`; persist localStorage.
- localStorage keys: capicorn:theme, capicorn:capture-mode, capicorn:ink-color,
  capicorn:stroke-width, capicorn:save-dir.
- Save flow: capture → auto-save raw `Capicorn-<ts>.png`; Save/Ctrl+S → if isDirty(),
  save flattened `Capicorn-<ts>-edited.png`, else "No changes to save".

## Tracks (disjoint)
- R: src-tauri/src/commands.rs, lib.rs (+ capture.rs helper if needed).
- E: src/editor/editor.ts, src/editor/tools.ts.
- U: src/ui/app.ts, src/ui/settings.ts, src/style.css, src/main.ts, index.html.
- Parent: Cargo.toml/tauri.conf.json/package.json version, capabilities, build/ship.

## Ship
gates → pnpm tauri build v0.3.0 → launch-test → browser-verify → commit/push/release → MEDIA.

## Appendix — measured reality
(append claim → measured → change)