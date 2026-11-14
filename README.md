# Capicorn ✦

**Open-source Snagit-style screen capture & annotation.** Screenshot, mark up, record — without the subscription.

Capicorn is a clean-room, cross-platform reimplementation of the Snagit workflow:
**capture → annotate → share in under ten seconds.** Built with Rust + Tauri, in the
spirit of the ArtCraft project (open-source clones of proprietary creative tools).

> Status: **early alpha.** Linux (Wayland/X11) capture lands first; macOS and Windows
> backends are wired into the same cross-platform shell and follow next.

## Why

Snagit is excellent but proprietary, paid, and has no Linux build. The open-source
landscape is fragmented — Flameshot does screenshots, Kooha/Cap do recording, ShareX
does everything but only on Windows. Nobody ships the whole *capture → annotate →
share* flow in one lightweight app. Capicorn fills that gap.

## Features (MVP)

- 📸 Full-screen capture via the desktop portal (Wayland-native, no hacks)
- ✂️ In-editor region crop
- 🖊️ Annotation tools: arrow, text, rectangle, blur/pixelate, step numbers
- ↩️ Undo / redo
- 📋 Copy to clipboard & 💾 save to PNG
- 🎬 Screen recording (MP4/GIF) — *wave 2*
- 📜 Scrolling capture + OCR + local-AI auto-redaction — *wave 3*

## Tech stack

| Layer | Choice |
|-------|--------|
| Shell | [Tauri 2](https://tauri.app) (Rust backend, web frontend) |
| Capture (Linux) | `ashpd` → xdg-desktop-portal Screenshot |
| Capture (macOS/Win) | `xcap` |
| Clipboard | `arboard` |
| Editor | [Fabric.js](http://fabricjs.com) canvas |
| Frontend | Vite + TypeScript |

## Build from source

Prerequisites: Rust (stable), Node 20+, pnpm, and the
[Tauri Linux deps](https://tauri.app/start/prerequisites/) (`libwebkit2gtk-4.1-dev`, …).

```bash
pnpm install
pnpm tauri dev      # run in development
pnpm tauri build    # produce AppImage / deb / dmg / exe
```

## Project layout

```
src/            frontend (Vite + TS)
  editor/       Fabric.js annotation editor
  ui/           app shell & capture flow
src-tauri/      Rust backend
  src/capture.rs    portal / xcap capture
  src/clipboard.rs  clipboard image
  src/commands.rs   Tauri command surface
docs/specs/     design spec
docs/plans/     implementation plans
```

## License

MIT — see [LICENSE](LICENSE).

## Acknowledgements

Inspired by [ArtCraft](https://github.com/) (clean-room Adobe clones),
[Cap](https://cap.so) (open-source Loom), and
[Flameshot](https://flameshot.org). Not affiliated with TechSmith/Snakit.