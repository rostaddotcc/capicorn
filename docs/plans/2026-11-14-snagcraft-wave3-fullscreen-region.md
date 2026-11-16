# SnagCraft Wave 3 — TRUE fullscreen region capture

> rostad's feedback: v0.2.0 region select only works INSIDE the app window. He wants to select a
> square on the ACTUAL desktop (Snagit-style). Fix with a fullscreen frozen-screen overlay.

**Approach (one window, no layer-shell hacks, cross-DE):**
Region mode = make the main window **fullscreen** → capture (window hidden during capture) →
show the frozen screenshot in a **fixed, viewport-covering overlay** → user drags a box anywhere
on the (fullscreen = whole screen) frozen desktop → exit fullscreen → crop → editor. Because the
fullscreen window equals the screen and the capture is native resolution, selection maps 1:1 to
screen pixels. Works on KDE/GNOME/Windows/macOS (just fullscreen + an overlay).

## Locked contract
- New Tauri command `set_fullscreen(full: bool) -> ()` (main window). R owns.
- `capture_screenshot` unchanged (hides window during capture).
- Editor API unchanged. `selectRegion`/`cropDataUrl` (src/ui/region.ts) reused, overlay now fixed
  fullscreen over document.body.

## Tracks
- **R**: `src-tauri/src/commands.rs` + `src-tauri/src/lib.rs` — add + register `set_fullscreen`.
- **U**: `src/ui/app.ts`, `src/ui/region.ts`, `src/style.css` — region flow:
  set_fullscreen(true) → capture_screenshot → selectRegion(fixed fullscreen overlay) →
  set_fullscreen(false) → cropDataUrl → editor.load. Cancel restores fullscreen=false.

## Ship (parent)
gates → `pnpm tauri build` v0.2.1 → launch-test → commit/push/release → MEDIA.

## Appendix — measured reality
(append claim → measured → change)