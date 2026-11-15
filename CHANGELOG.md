# Changelog

All notable changes to Capicorn, newest first. Full release notes for every tag live on
the [releases page](https://github.com/rostaddotcc/capicorn/releases).

## v0.27.1
- **Quality gate in CI.** A new `quality` job runs `cargo fmt --check`, `cargo clippy -D warnings`, the Rust unit tests and `tsc --noEmit`; both platform builds now depend on it.
- The whole Rust tree is **rustfmt-formatted** and **clippy-clean** (zero warnings).

## v0.27.0
- **Watermark.** Burn a text mark into exports — Settings → Export sets the text, opacity and corner. It never touches the editor or the live capture, and the size scales with the image so a 4K shot stays readable.
- **Upload.** File → Upload… POSTs the flattened PNG to *your* endpoint (Settings → Export) as `multipart/form-data` and copies the link the server answers with. No provider hardcoded, no credentials shipped — point it at your own handler.

## v0.26.0
- **Open images from disk.** File → Open… (`Ctrl+O`) with an image filter; everything is normalised to PNG, lands in the editor and appears in Recent as "Open". Drag-and-drop and `Ctrl+V` keep working.
- **Shadow controls you can feel.** The X/Y offset steppers are gone. **Direction** (0–360°), **distance**, **blur** and **opacity** sliders drive the shadow, with the sample tile showing how it falls. Offsets are derived from angle × distance, so existing presets keep their shadows.

## v0.25.0
- **Audio in recordings.** Pick a microphone and/or capture system audio (Linux: PulseAudio devices via `pactl`; Windows: `dshow`); the sources are mixed with `amix` into AAC. GIF stays silent.
- **Interval capture.** Capture → Interval takes a screenshot every 5 s / 10 s / 30 s / 60 s / 5 min in a small always-on-top card, saves every shot with a sequence number and can turn the whole set into an **MP4 timelapse** in one click.
- **Pause / resume** a recording from the pill; the elapsed clock freezes with it.
- **Record a window** — a third Record row restricts the portal picker to windows.
- **Magnify is back, and real.** Drag a lens and the area under it is enlarged live by 1.5–10× (the size slider becomes the zoom); an `N×` badge shows the factor. Exports match the canvas.

## v0.24.0
- **The unicorn head is now the File menu** — it replaces the word "File" and the topbar brand text (which only took space). Clicking it opens save/settings/widget/About.
- **Settings icon removed** from the topbar; Settings… stays in the File menu (Ctrl+,).
- **About dialog** added (File → About) with the version, license and repository; the version also shows beside the row.
- **Magnify removed** — its live preview was only a tinted box (no enlargement until export), so the half-working tool is gone from the toolbar, editor and shortcut list.

## v0.23.0
- **The timer is now a small always-on-top countdown**, not a full-window overlay. The main window shrinks to a clean timer card (decorations off), so the desktop stays usable — open menus, hover tooltips, arrange the shot — while it counts down.
- **The floating widget comes back after a capture**, and closing the editor now leaves the widget on top instead of quitting the app. Closing the widget brings the editor back, so the app can never become unreachable.
- **Removed Pin** — it never worked reliably and its purpose was unclear; gone from the UI and the backend (with its tests).
- `set_hud_mode` generalises the window-shrink used by both the scrolling card and the timer.

## v0.22.4
- **Scrolling capture's Done button now works.** The card used to sit inside a full-size **click-through** window, so a click on Done passed straight through to the app below, and after scrolling another window Enter went there too. A new `set_scroll_mode` shrinks the main window to the card (min-size lifted while active), turns click-through off and keeps the card on top — Done is clickable and Enter is delivered. The app chrome is hidden while the card owns the window; the old Alt "mouse control" step is gone.

## v0.22.3
- **Window / Choose-screen capture now negotiates its size flexibly.** The stream asked for an exact `VideoSize` (the portal's hint), which the producer cannot always match for an individual window — the format negotiation failed, no frame arrived, and the grab timed out as "cancelled". It now offers a **range** (1×1 … 16384×16384) with the hint as the default, so the producer delivers its real size. The recorder got the same fix.
- Window/monitor capture now **logs** to `~/.cache/capicorn-portal.log` (picker answer, stream states, negotiated format, first frame), so "Copy recording diagnostics" covers it too.

## v0.22.2
- Fixed **Window / Choose-screen** capture (and scrolling capture) on KDE. The one-frame ScreenCast consumer carried the same three PipeWire traps the recorder had: a `target.object` prop resolving before the registry saw the portal node, no registry wait before connecting, and an oversized explicit buffer pool KWin rejects. All three now use the proven pattern.

## v0.22.1
- Rectangle boxes are **untinted again** (outline only) — the faint fill added for shadows read as an unwanted "highlight". The drop shadow still falls naturally around the outline.

## v0.22.0
- Design pass toward a **more classic, less "AI" look**: neutral grey chrome (dropped the purple tint from surfaces/borders), tighter border radii, softer shadows; the purple accent is now reserved for primary actions and the brand.
- Neutral dot grid and scrollbars.

## v0.21.0
- Removed the standalone **Record** button — recording lives in the Capture menu; the running take's pill gains a **Stop** button.
- Blur/pixelate/blackout now preview **transparently** while dragging and apply the **real** effect the moment the box is released (no more odd colour, no click-elsewhere needed).
- New **unicorn** logo + app icon (ArtCraft-style clean geometric mark).
- Windows builds now produce an **.msi** as well as the NSIS .exe.

## v0.20.0
- New **File** menu (top-left): Save As…, Save, Copy image, Settings…, and a **Floating widget** toggle — the widget is easy to find again.
- **Record modes live in the Capture menu** (Record full screen / Record region); the separate Record dropdown is gone, so there is one mode menu.
- Window / screen captures **no longer leave the floating widget on screen** (it is hidden with the main window during a capture, so it can't "pop up").

## v0.19.0
- Recording indicator is red and pulses (the status dot follows the recording state).
- More shortcuts: bare-letter tool keys (`V R A T B P H S M E K`) and **Ctrl+scroll zoom**.
- Blur tool shows the **real** blur / pixelate / blackout live on the canvas, not a placeholder.
- Rectangle shadows now cast (the box gets a faint tint instead of a pure outline).
- Sleeker arrow: rounded tail, tapered shaft, head scaled with the stroke width.
- Text tool: resizable **text box** (drag to size), wrapping text, and a thin grey frame.

## v0.18.2
- License changed to **GPL-3.0-or-later** with a commercial dual-license; added a
  CONTRIBUTING note (contributions are licensed under both).
- README: the Windows `.exe` is now marked untested, with Linux as the focus.

## v0.18.1
- Recordings now appear in the Recent filmstrip (thumbnail via ffmpeg, film-icon fallback); clicking one opens the file with the OS default player.

## v0.18.0
- Settings moved to the top-left of the title bar.
- Thin AppImage (~3.9 MB) that uses the system's WebKitGTK instead of bundling the whole
  stack; the full self-contained AppImage (82 MB) ships alongside it.

## v0.17.2
- Screen recording: `error alloc buffers: Invalid argument` — drop the explicit
  `SPA_PARAM_Buffers` pool (KWin rejected it); `MAP_BUFFERS` producer defaults are correct.

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
