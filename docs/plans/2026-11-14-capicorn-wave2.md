# Capicorn Wave 2 — Region capture, color & stroke, self-capture fix

> Execute with subagent-driven-development + delegation-tuning. Parallel tracks R / E / U,
> disjoint files, locked contracts. Parent owns scaffold/Cargo.toml/lib.rs/build.

**Goal:** Fix the recursive self-capture (the "hall of mirrors" rostad saw), add Snagit-style
**region/zone capture**, and add **annotation color + stroke-width selection**. Ship v0.2.0 AppImage.

**Root cause of the recursion:** full-screen capture included the Capicorn window itself, so each
capture nested the previous one. Fix: hide the app window during capture, and offer region capture.

---

## Locked contracts

### Tauri commands (R ↔ frontend) — shape UNCHANGED
- `capture_screenshot` → `{ dataUrl, width, height }`. Now hides the main window during capture
  (so we never capture ourselves), restores it after. No frontend args. AppHandle auto-injected.
- `save_png`, `copy_png`, `get_version` unchanged.

### Editor additive API (E ↔ U) — existing API unchanged, ADD:
```ts
setColor(color: string): void;        // hex, e.g. "#ff3b30"; applies to NEW annotations
getColor(): string;
setStrokeWidth(width: number): void;  // clamped 1..12
getStrokeWidth(): number;
```
Default color = ACCENT "#ff3b30", default stroke = 3. Crop chrome stays ACCENT.

### Region helpers (U owns src/ui/region.ts)
```ts
selectRegion(dataUrl: string, host: HTMLElement): Promise<Box | null>; // Box in NATURAL image px; null = cancelled
cropDataUrl(dataUrl: string, box: Box): Promise<string>;               // canvas crop → PNG dataUrl
```

## Tracks (disjoint files)
- **R** Rust: `src-tauri/src/capture.rs`, `commands.rs`, `clipboard.rs`. Hide/show window around capture.
- **E** editor: `src/editor/editor.ts`, `src/editor/tools.ts`. Color + stroke via ToolHost.
- **U** shell: `src/ui/app.ts`, `src/ui/region.ts` (new), `src/style.css`, `src/main.ts`, `index.html`.
  Capture mode menu (Full / Region), region overlay, color palette, stroke control.

## Build & ship (parent)
cargo build + test, tsc + vite build, `pnpm tauri build` → v0.2.0 AppImage, launch-test, commit,
push, `gh release v0.2.0`, deliver via MEDIA.

## Appendix — measured reality
(append claim → measured → change)

| Claim | Measured | Change |
|-------|----------|--------|
| The build host is rostad's test machine | Build host = PN53 (GNOME, screen locked). rostad's screenshot shows a **KDE Plasma ultrawide 3440×1440** desktop (Dolphin, different taskbar) — a SEPARATE workstation where he runs the AppImage. | Capture is validated by rostad's own screenshot (3440×1440), not by the PN53 probe. PN53's portal probe hangs only because PN53's screen is locked — irrelevant to the user's box. The hide-window + region fix must work cross-DE (GNOME/KDE); Tauri hide/show + portal Screenshot are DE-agnostic. |
| Repeated full-screen captures "get smaller" | Droste effect: full-screen capture includes the Capicorn window, nesting the previous capture. Capture dimensions stay 3440×1440. | Fixed by hiding the app window during capture + offering region capture. |