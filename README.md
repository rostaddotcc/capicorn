# SnagCraft ✦

**Open-source Snagit-style screen capture & annotation.** Screenshot, mark up, record — without the subscription.

SnagCraft is a clean-room, cross-platform reimplementation of the Snagit workflow:
**capture → annotate → share in under ten seconds.** Built with Rust + Tauri, in the
spirit of the ArtCraft project (open-source clones of proprietary creative tools).

> Status: **alpha, Linux-first.** Linux (Wayland/X11) is the primary target and the only
> platform tested end-to-end. The **Windows `.exe` is untested** — CI builds it from the
> same cross-platform code (screenshots, editor, export, clipboard, tray, recording via
> ffmpeg `gdigrab`), but nobody has run it on real Windows hardware, so treat it as
> experimental. Scrolling capture and global hotkeys are Linux-only for now.

## Why

Snagit is excellent but proprietary, paid, and has no Linux build. The open-source
landscape is fragmented — Flameshot does screenshots, Kooha/Cap do recording, ShareX
does everything but only on Windows. Nobody ships the whole *capture → annotate →
share* flow in one lightweight app. SnagCraft fills that gap.

## Features

**Capture**
- Full screen, a selected region, a single window, or one monitor — via the desktop
  portal on Wayland (no hacks, no root)
- **Scrolling capture** — scroll the window by hand and SnagCraft stitches the frames
  into one tall image
- **Screen recording** to MP4 or GIF — full screen, a region or a picked window
  (Linux: ScreenCast portal · Windows: ffmpeg `gdigrab`), with an on-screen timer,
  **pause/resume** and optional **microphone + system audio** (mixed to AAC)
- **Interval capture** — a screenshot every 5 s / 10 s / 30 s / 60 s / 5 min, optionally
  stitched into an MP4 timelapse
- Capture timer (off / 3 / 5 / 10 s) and a `PrintScreen` shortcut

**Annotate** (Fabric.js, native resolution)
- 13 tools: select, crop, arrow, text, rectangle, **true Gaussian blur** / pixelate /
  blackout, spotlight, highlighter, callout, step numbers, **live magnify lens**, brush,
  fill bucket
- **Quick text bar** — font, size, bold, italic and colour float right on the caption
  you're editing; drag to size a text box, no placeholder to delete first
- Shadows with **direction / distance / blur / opacity** sliders and a live sample —
  applied to the selection as you tune them, on shapes, arrows and labels alike
- Stroke width, and per-tool sizes with a live brush-size preview
- Ten bundled OFL fonts (Inter, Lato, Source Serif 4, Playfair Display, Oswald,
  Archivo Black, JetBrains Mono, Fira Code, Caveat, Pacifico) with size / bold / italic,
  plus reusable style presets
- Undo / redo, in-editor crop, delete the selected annotation with `Delete`

**Share**
- Copy to clipboard · save as PNG / JPEG / WebP (with quality) · **Save As…**
- **Save = a new copy**: every Ctrl+S writes a fresh version next to the original
  (never overwrites) and lands in Recent — browse back through every edit
- **Watermark** burned into exports (text, opacity, corner — never the live capture)
- **Upload** to your own endpoint, with the returned link copied to the clipboard
- Auto-save on capture, custom filename patterns (`{date}`, `{time}`, `{datetime}`)
- **OCR** — extract text from a capture (English / Swedish) via the system `tesseract`
- Recent-captures filmstrip — **resizable**: drag its top edge for bigger thumbnails

**Open & record**
- **File → Open…** (`Ctrl+O`) to annotate any image on disk; drag-and-drop and `Ctrl+V` too
- **Audio in recordings** — microphone and/or system audio, mixed into AAC
- **Pause/resume** a take without starting a new file

**Desktop integration**
- System tray with **click-to-restore** (left click or double click opens the window,
  middle click toggles the widget, right click is the menu) and minimise-to-tray
- **Start at login** — Settings → General registers SnagCraft as a login item
  (XDG autostart on Linux, AppImage-aware; registry on Windows; LaunchAgent on macOS)
- A collapsible floating capture widget (the "flärp") — a glass bookmark tab with the
  unicorn mark and a breathing accent spine; hover to open, drag to park anywhere
- Global hotkeys via the Wayland GlobalShortcuts portal (KDE)
- Light / dark themes, and a "save before closing?" guard

## Shortcuts

| Key | Action |
|-----|--------|
| `PrintScreen` | Capture full screen |
| `Shift` + `PrintScreen` | Capture a region |
| `Ctrl` / `Cmd` + `Z` | Undo |
| `Ctrl` / `Cmd` + `Shift` + `Z` · `Ctrl` + `Y` | Redo |
| `Ctrl` / `Cmd` + `C` | Copy to clipboard |
| `Ctrl` / `Cmd` + scroll | Zoom in / out |
| `V` `R` `A` `T` `B` `P` `H` `S` `M` `E` `K` | Tool shortcuts (Select, Box, Arrow, Text, Blur…) |
| `Ctrl` / `Cmd` + `S` | Save |
| `Ctrl` / `Cmd` + `Shift` + `S` | Save As… |
| `Ctrl` / `Cmd` + `,` | Settings |
| `Delete` / `Backspace` | Delete the selected annotation |
| `Esc` | Cancel / deselect / close |

## Tech stack

| Layer | Choice |
|-------|--------|
| Shell | [Tauri 2](https://tauri.app) (Rust backend, web frontend) |
| Capture (Linux) | `ashpd` → xdg-desktop-portal Screenshot |
| Capture (macOS/Win) | `xcap` |
| Clipboard | `arboard` |
| Editor | [Fabric.js](http://fabricjs.com) canvas |
| Frontend | Vite + TypeScript |

## Platforms

| Feature | Linux (Wayland/X11) — tested | Windows — untested |
|---------|------------------------------|--------------------|
| Screenshot (full / region / window) | ✅ portal + `xcap` | ⚠️ `xcap` |
| Editor (annotations, blur, shadows, fonts) | ✅ | ⚠️ |
| Copy / save (PNG / JPEG / WebP) | ✅ | ⚠️ |
| System tray + floating widget | ✅ | ⚠️ |
| Start at login | ✅ XDG autostart | ⚠️ registry Run key |
| Screen recording | ✅ ScreenCast portal (needs `ffmpeg`) | ⚠️ ffmpeg `gdigrab` (needs `ffmpeg`) |
| Scrolling capture | ✅ | 🔜 |
| Global hotkeys | ✅ GlobalShortcuts portal | 🔜 |

> **Windows is compile-verified only.** The `.exe` is built by CI from the same code but
> has not been run on real hardware — **Linux is the focus.** Windows test reports are
> welcome, but expect rough edges.

## Recording troubleshooting

Linux recording uses the xdg-desktop-portal ScreenCast API and needs `ffmpeg` on
`PATH`. If a take fails to start, SnagCraft logs every portal step to
`~/.cache/snagcraft-portal.log`, and each take writes a `*.rec.log` next to the video.
Those two files pinpoint exactly where the pipeline stopped.

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

SnagCraft is **GPL-3.0-or-later** — genuinely open source. You are free to use, study,
fork, modify and share it, **including commercially**, as long as derivatives stay open
under the GPL (see [LICENSE](LICENSE)).

**Commercial license:** if you want to embed SnagCraft in a proprietary, closed-source
product where the GPL's source-sharing obligations don't fit, a separate commercial
license is available from the author — that's the one case that needs permission. Open an
issue or reach out to arrange it.

> The project name and logo remain the author's; a fork should carry its own name.

## Acknowledgements

Inspired by [ArtCraft](https://github.com/) (clean-room Adobe clones),
[Cap](https://cap.so) (open-source Loom), and
[Flameshot](https://flameshot.org). Not affiliated with TechSmith/Snakit.

Bundled fonts (SIL OFL 1.1 — see `src/assets/fonts/OFL.txt`): Inter, Source Serif 4,
JetBrains Mono, and Caveat.