# SnagCraft — Design Spec (MVP)

**Date:** 2026-11-14
**Status:** approved direction (ArtCraft track: cross-platform product, open repo)
**Owner:** rostad · built by Härmes + parallel subagents

## 1. Purpose & success criteria

Reimplement the **Snagit workflow** as a free, open-source, cross-platform app:
**capture → annotate → share in under ten seconds.** Clean-room (no TechSmith code),
in the spirit of ArtCraft's Adobe clones.

Success for the MVP (wave 1):
- On rostad's GNOME/Wayland workstation, one click captures the screen.
- The captured image opens in an editor with crop + arrow + text + rectangle +
  blur + step-number tools, undo/redo.
- The result copies to the clipboard and saves to PNG.
- Ships as a runnable **.AppImage** rostad can test and give feedback on.

## 2. Scope

**In (wave 1):** full-screen capture, in-editor region crop, annotation tools,
undo/redo, copy-to-clipboard, save-to-PNG, Linux Wayland capture backend,
cross-platform Tauri shell, AppImage + deb bundling, public GitHub repo + README.

**Wave 2:** region/screen recording → MP4/GIF + trim; pre-capture region overlay.
**Wave 3:** scrolling capture (stitching), OCR, local-AI auto-redaction of secrets.

**Out (YAGNI for now):** accounts/cloud sharing, templates, video editor,
Snagit "Simplify" (AI object removal — research-grade), scheduled capture.

## 3. Architecture

Tauri 2 app: Rust backend (capture, clipboard, file save) + web frontend
(Vite + TypeScript, Fabric.js canvas editor). Cross-platform shell; capture
backend is pluggable per-OS.

```
Frontend (TS)                         Rust backend
─────────────                         ────────────
ui/app.ts  ── invoke('capture_screenshot') ──► commands.rs ─► capture.rs
   │                                                              │ ashpd portal (Linux)
   ▼                                                              │ xcap (macOS/Win)
editor/editor.ts (Fabric.js)  ◄── { dataUrl, width, height } ─────┘
   │ flatten() → dataUrl
   ├─ invoke('copy_png', {dataUrl}) ─► commands.rs ─► clipboard.rs (arboard)
   └─ invoke('save_png', {dataUrl, suggestedName}) ─► commands.rs ─► dialog + fs
```

### Capture on Wayland (the load-bearing decision)

A transparent pre-capture region overlay is unreliable on GNOME Wayland. Instead:
**portal captures the full screen → the editor offers a crop tool.** This is robust,
needs no special compositor support, and matches the "capture then refine" flow.
Linux uses `ashpd` → `org.freedesktop.portal.Screenshot`; the portal returns a PNG
file URI which we read and base64-encode. macOS/Windows use `xcap`.

> PROBE GATE: before locking, verify `ashpd` Screenshot actually returns an image on
> this GNOME/Wayland session (a tiny Rust probe). If the portal path fails, fall back
> to `xcap`'s Wayland support or `grim`-style tooling and record the result in the plan
> appendix.

## 4. Locked contracts

### 4.1 Tauri commands (Rust ↔ frontend)

| Command | Args (camelCase) | Returns |
|---------|------------------|---------|
| `capture_screenshot` | — | `{ dataUrl: string, width: number, height: number }` |
| `save_png` | `dataUrl: string, suggestedName: string` | `string` (saved path) |
| `copy_png` | `dataUrl: string` | `void` |
| `get_version` | — | `string` |

`dataUrl` is a full `data:image/png;base64,…` URL. Tauri maps camelCase args to
snake_case Rust params automatically. Errors reject the promise with a string message.

### 4.2 Editor public API (editor module ↔ shell)

```ts
type ToolId = "select" | "crop" | "arrow" | "text" | "rect" | "blur" | "step";
class Editor {
  constructor(container: HTMLElement);
  load(dataUrl: string): Promise<void>;  // fit image to view
  flatten(): string;                     // PNG dataUrl: image + annotations + crop
  setTool(tool: ToolId): void;
  undo(): void; redo(): void;
  destroy(): void;
}
```

## 5. File ownership (parallel waves)

| Track | Owner | Files (exclusive) |
|-------|-------|-------------------|
| Scaffold/integration/build | parent | Cargo.toml, lib.rs, main.rs, tauri.conf.json, capabilities, package.json, vite/tsconfig, icons, README, docs |
| **R** — Rust backend | subagent | `src-tauri/src/capture.rs`, `clipboard.rs`, `commands.rs` |
| **F** — annotation editor | subagent | `src/editor/editor.ts`, `src/editor/tools.ts` |
| **U** — UI shell | subagent | `src/ui/app.ts`, `src/main.ts`, `src/style.css`, `index.html` |

R, F, U run concurrently against the locked contracts (§4). They do NOT run git or
touch sibling files; the parent integrates, builds, and commits between waves.

## 6. Error handling

- Capture failure → frontend status line shows the message; no crash.
- Clipboard unavailable (no Wayland data-control) → fall back to save-only, surface a note.
- Portal permission denied → clear message, retry button.

## 7. Testing

- Rust: `cargo test` unit tests for PNG encode/decode + data-url round-trip; `cargo build` gate.
- Frontend: `tsc` typecheck + `vite build` gate.
- Integration: parent runs the AppImage, captures, annotates, copies, saves; verifies
  the saved PNG is valid and the clipboard holds an image.

## 8. Risks

- Wayland portal capture behavior varies → PROBE GATE (§3).
- Wayland clipboard image support → test; degrade to save-only.
- AppImage bundling on Ubuntu 26.04 (linuxdeploy) → fallback to manual AppImage or deb.
- Subagent throughput on Token Plan → small briefs (≤3 files), first-write-early, budgets.