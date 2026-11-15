# Changelog

All notable changes to Capicorn, newest first. Full release notes for every tag live on
the [releases page](https://github.com/rostaddotcc/capicorn/releases).

## v0.17.1
- Screen recording fix: `no target node available` — drop the `target.object` stream
  property (it pre-resolved the target before the portal node was in the registry) and wait
  for the node to appear in the registry before connecting.

## v0.17.0
- Paste (`Ctrl+V`) or drag-and-drop an image to annotate it.

## v0.16.0
- In-app keyboard shortcuts overlay (`?` key or the top-bar help button).

## v0.15.0 — Windows recording
- Windows screen recording via `ffmpeg -f gdigrab` (full screen or region), graceful stop.

## v0.14.1
- Settings → Troubleshooting → "Copy recording diagnostics": bundles the portal log and the
  latest take log onto the clipboard.

## v0.14.0
- Windows NSIS installer built by GitHub Actions CI.
- Platform support matrix and recording troubleshooting in the README.

## v0.13.1
- Widget drag fixed at the root: the `widget` window is now covered by the Tauri capability
  (`allow-start-dragging`, `allow-set-size`), and window moves go through an app command.

## v0.13.0
- Recording: the portal offers monitors **and** windows; SelectSources is kept alive until
  the cast starts; the real portal error is surfaced (no more false "no screen was picked");
  every portal step is logged to `~/.cache/capicorn-portal.log`.
- Brush-size preview in the properties rail and as an on-canvas cursor.
- Shadows for arrows, rectangles, text, callouts and steps.
- The floating widget collapses to a small edge tab ("flärp").

## v0.12.2
- Per-take recording diagnostic log.

## v0.12.1
- Clear error when `ffmpeg` is missing.

## v0.12.0
- Recording dropdown with full-screen and region modes; region crops each frame.

## v0.11.0
- Save As… (native dialog, PNG/JPEG/WebP), reset-to-default save folder, auto-save toggle,
  transparent movable widget.

## v0.10.1
- Text tool: no stuck placeholder, undo works while typing, drag the text box while editing.

## v0.10.0
- Window and monitor capture; menu polish.

## v0.9.0
- Brush tool, per-tool size sliders, image resize.

## v0.8.0
- Scrolling capture: manual scroll + frame stitching.

## v0.7.0
- Screen recording (MP4/GIF).

## v0.6.0
- OCR, style presets, recent-captures filmstrip, pin, magnify.

## v0.5.0
- Global hotkeys (GlobalShortcuts portal), floating widget, contextual properties rail.

## v0.4.0
- True Gaussian blur (StackBlur), pixelate, blackout, spotlight, highlighter, callout.
- Export PNG/JPEG/WebP, system tray, minimise-to-tray, close guard, capture timer.

## v0.3.0
- Fonts (Inter, Source Serif 4, JetBrains Mono, Caveat), font picker, capture dropdown,
  dark mode, settings, fill bucket.

## v0.2.1
- True fullscreen region capture.

## v0.2.0
- Region capture and colour selection.

## v0.1.0
- First alpha: portal capture + Fabric.js editor.
