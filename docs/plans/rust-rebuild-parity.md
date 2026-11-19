# SnagCraft parity checklist — Rust rebuild (W1 reference)

**Baseline:** live v1.5.3, commit `3cec2ee`, snapshot date 2026-11-16. Sources: `src/editor/*.ts`, `src/ui/app.ts`, `src-tauri/src/*`, `CHANGELOG.md`, `README.md`.
**Rule:** every row is a *feature* with the file:line where the TS app implements it. `rust:` is the rebuild status: `pending` (not started) or `W0 spike` (proven in the W0 probe gate — PNG load, arrow/text/rect, undo/redo, gaussian blur, classic toolbar, transparent viewport).
**Use:** W1/W2/… tick rows by replacing `pending` with the wave id. A row unticked at parity-freeze is a regression.

Legend: the **rust** column is the checkbox — `pending` = not started, `W0 spike` = proven in the W0 probe gate, `n/a` = deliberately out of scope (don't rebuild). Tick a row by replacing the status with the wave that lands it (`W1`, `W2`, …).

**W6b honesty audit (2026-11-17).** 150 rows. Every tick was re-read against either the
wave delivery in `docs/plans/rust-rebuild-status.md` or the Rust source; rows the code
does not back stay `pending`, and the nine rows reverted at the W4 audit (OCR
language/busy UI, `make_timelapse` video assembly, `Ctrl+.`, shrunk countdown HUD,
upload POST flow) are still `pending`. Ticks added or narrowed in this pass are
code-verified at: `ui-egui/src/lib.rs` 4811/4826 (dashed effect preview), 3903/3906
(`Ctrl+C`/`Ctrl+V`), 3909 (`Ctrl+,`), 3928/3935 (Esc, Delete/Backspace), 3866/3920/3063
(Enter: crop, caption, widget), 960+1774 (autosave capture), 2317/4492 (drag-drop),
2680–2901 (settings panel: format, quality, watermark, share URL, save folder file-name
pattern, hotkey triggers, theme); engine crop-undo test (`crates/engine/src/lib.rs`);
status wake #17 (file associations, `%F` + argv). Three rows are ticked with a
parenthetical because only part of the live behaviour exists — read the cell, not just
the wave letter.

---

## 1. Tools — the 13 classes as implemented (`src/editor/tools.ts`)

Registry: `createTool()` switch `tools.ts:1917-1946`; `ToolId` narrowing at `tools.ts:482`; `Tool` interface `tools.ts:481-534` (`activate/deactivate/pointerDown/pointerMove/pointerUp/keyDown/cursor`). Toolbar order and bare-letter routing is `app.ts:5156-5170`.

**Shared option surface** (every tool reads only its own groups — contextual rail `app.ts:348-380`, `PropGroup` `app.ts:341`):
- Palette: 8 swatches Red `#ff3b30`, Orange `#ff9500`, Yellow `#ffcc00`, Green `#34c759`, Blue `#007aff`, Purple `#8b7cf6`, Black `#1c1c1e`, White `#ffffff` — `app.ts:330-339`
- Stroke widths (annotation): default at `tools.ts:33`; consumed via `host.strokeWidth()` `tools.ts:640,960`
- Fonts: 10 bundled OFL families — Inter, Lato, Source Serif 4, Playfair Display, Oswald, Archivo Black, JetBrains Mono, Fira Code, Caveat, Pacifico — `app.ts:471-487`, preloaded `app.ts:498-501`
- Shadow: UI is **direction 0–360° (default 90°), distance 0–50 px (4 px), blur, opacity 2–100 % (45 %)**; the stored shape stays `ShadowStyle { enabled, color, blur 0–60, offsetX ±50, offsetY ±50 }` with offsets derived from angle × distance — `tools.ts:404-417`, `shadowFromStyle` `:427-437` (`affectStroke: true` is load-bearing, else outline-only shapes/arrows never cast one), UI `app.ts:1068-1078`, derivation `:643-657`
- Text style: `TextStyle { fontFamily, fontSize 8–200, fontWeight normal|bold, fontStyle normal|italic }` — `tools.ts:395-400`; **no underline, no alignment control** — clamp `app.ts:740-741`
- Stroke width clamp **1–12** — `app.ts:731`
- Style **presets**: seeded `Red arrow` (`#ff3b30`, w3), `Highlight` (`#ffcc00`, w5), `Blur strong` (gaussian, r40); user presets persisted and sanitised — `app.ts:718-722`, `sanitizeStyle` `:724-756`, `readPresets` `:758`

| # | Tool id | Shortcut | Behavior / options | Evidence | rust |
|---|---------|----------|--------------------|----------|------|
| 1 | `select` | `V` | Click-select, drag-move, Delete removes; discards active object on activate | `tools.ts:535-628` (`id` :536, activate :541-543); rail hint `app.ts:349-352` | W2 + W14 (move W2; corner resize handles + double-click Text/Callout re-edit W14 — the live SelectTool is a thin shell over fabric `applyInteractivity`, which provides these) |
| 2 | `shape` | `R` | Drag outline. 5 variants: **rect, ellipse, triangle, line, star** (`SHAPE_KINDS`) — star is a normalised 96×91 5-point path. Options: shape picker + color + stroke + shadow | `tools.ts:614-616`, star path :619, class :628-785, stroke from `host.color()`/`strokeWidth()` :639-640; picker buttons `app.ts:964-968`, rail `app.ts:361-364` | W2 |
| 3 | `blur` | `B` | Drag a region; renders per current blur **mode** (gaussian/pixelate/blackout), dashed preview outline | `tools.ts:785-811`, mode read :789, `blurRegionPreview` :790; class `BlurRegion` :54-104 | W2 |
| 4 | `spotlight` | `L` | Darkens everything except the dragged rect; outline thickness opt | `tools.ts:811-835`, thickness :815; `SpotlightRegion` :104-152 | W2 |
| 5 | `highlighter` | `H` | Translucent ink rect multiplied into pixels; opts `ink`, `alpha`, outline `thickness` | `tools.ts:835-861`, opts :168-169; `HighlighterRegion` :152-208 | W2 |
| 6 | `magnify` | `M` | Loupe lens: samples a source region and re-draws it scaled by `zoom` (**1.5–10×**) with a rounded-rect border + zoom label | `tools.ts:861-951`, `zoom` opt :276, rounded-rect helper :228-246, border :342, label :356; `LensBackground` :208; range `app.ts:3401` | W2 |
| 7 | `arrow` | `A` | Drag arrow, head scaled off stroke width (`strokeWidth*2`), fill = current color; shadow opt | `tools.ts:951-1045`, head :1014-1018; rail `app.ts:357-360` | W2 |
| 8 | `text` | `T` | Click to place, type inline; options: 10 font families, size 8–200, **bold**, **italic**, color, shadow (no underline or alignment) | `tools.ts:1045-1232` (`TextStyle` :395-400); style applied :1101-1102, stroke/color :1190 | W2 |
| 9 | `step` | `S` | Click to drop next number; numbers auto-ordered; ring width opt (rail label "Ring width") | `tools.ts:1232-1305`, click-default box :1221, radius/fill :1257; rail `app.ts:365-369` | W2 |
| 10 | `callout` | `K` | Drag a speech-box; width drives font size; default 240×112, min width clamp | `tools.ts:1305-1462`, `CALLOUT_DEFAULT` :1296, size calc :1350, font :1390 | W2 |
| 11 | `crop` | `C` | Drag handles to frame, **Enter** commits, Esc cancels | `tools.ts:1462-1557`; rail hint `app.ts:353-356` | W2 + W14 (W13 audit: "handles" is live hint prose — the live overlay is painter-only, every drag redraws; rebuild matches. W14: plain-click/tiny-drag <8px resets the crop, Enter needs ≥8px — MIN_DRAG/MIN_SIZE parity) |
| 12 | `bucket` | `G` | Flood-fill region with current color (`context.fillStyle`), raster op on the canvas | `tools.ts:1557-1824`, fill :1776 | W2 |
| 13 | `brush` | `E` | Freehand stroke, nib width **2–44 px** (`brushWidth()`, rail = "Brush size", nib preview circle), stroke replaced while redrawing | `tools.ts:1824-1916`, width :1902, commit :1912; nib range + preview `app.ts:433`, `:993-995`, label `:3385-3397` | W2 + W14 (nib preview circle W14: 48px box, ink-coloured dot at 1:1 diameter, value label) |

**Shape variants detail** (`tools.ts:614-616`, UI `app.ts:964-968`): rect, ellipse, triangle, line, star. Line variant is the one drag-with-endpoints path (`tools.ts:632-720` common opts `left/top`).

---

## 2. Blur modes + redaction (`src/editor/blur.ts`)

Effects are applied to the **image layer at export resolution** (scene px × export multiplier via `toDevice()`), not as canvas filters — WebKitGTK canvas-filter support is patchy, so gaussian goes through `stackblur-canvas` — `blur.ts:1-17`, `toDevice` `:60`.

| Feature | Evidence | rust |
|---|---|---|
| Three modes `gaussian` / `pixelate` / `blackout`, persisted in `snagcraft:blur-mode` | `blur.ts:18`; `app.ts:117` (`BlurMode`), `:421` (key), `:570` (fallback to default) | W2 |
| Gaussian via StackBlur convolution (same result on every WebEngine) | `blur.ts:6-11`, `gaussianBlur` `:174` | W2 |
| Pixelate — block-size mosaic | `blur.ts:219`, block size shares the radius setting `:19` | W2 |
| Blackout — solid ink `#111827`, no intensity slider | `blur.ts:32` (`BLACKOUT_INK`), `blackout` `:244`; UI `app.ts:3314` (`usesIntensity = blurMode !== "blackout"`), dimmed slider `app.ts:3480` | W2 |
| Radius/block size 2–60 scene px, **default 12**, clamped | `blur.ts:20-22`, `clampBlurRadius` `:49` | W2 |
| **Redaction honesty:** any blur under `SECURE_BLUR_MIN = 10` is raised to 10 so soft settings cannot leave legible text | `blur.ts:24-25` | W2 |
| Live dashed outline while dragging; effect baked in on commit through `applyBlurRegion` | `blur.ts:258`, `BlurRegion` `tools.ts:54-104` | W2 |
| Spotlight dim colour `rgba(8, 12, 18, 0.68)` | `blur.ts:30`, `spotlight()` `:283` | W2 |
| Highlighter tint alpha **0.36** | `blur.ts:28` (`HIGHLIGHTER_ALPHA`), `highlighter()` `:311` | W2 |
| Region keeps mode + radius through undo/redo | `BlurRegion` opts `tools.ts:69`; preset round-trip `app.ts:742-743` | W2 |

---

## 3. File operations

| Feature | Evidence | rust |
|---|---|---|
| **Open image** (`Ctrl+O`) — native picker, filters png/jpg/jpeg/webp/gif/bmp/avif/tif/tiff | `app.ts:4909-4931` (filters :4918), loader `read_image_file` `commands.rs:284-298` | W0 spike (PNG) |
| **Open from file association / `cli-open` event / launch_request.file** | `app.ts:5238-5262`, backend `cli.rs:62-80` | W3 |
| **Drag & drop** an image onto the window (`dataTransfer.files[0]`, `image/*`) | `app.ts:5099-5105` | W1 |
| **Save** (`Ctrl+S`) — writes a *new* copy with `-edited` suffix, never overwrites; status "Saved copy …"; guards "Nothing to save yet" / "No changes to save" | `app.ts:1862-1885`, `writeImage` :1811-1831 | W19 (W0 spike proved PNG out; full saveEdited semantics W19 — toolbar Save + close-guard Save match too; the editable .snagcraft save stays reachable as an explicit File-menu addition) |
| **Save As…** (`Ctrl+Shift+S`) — native dialog, suggested `buildFilename(pattern, format)` | `app.ts:1891-1914`, `save_image_as` `commands.rs:187-228` | W9 |
| **Auto-save capture on arrival** (setting; off = keep in editor only) | `app.ts:1837-1855`, `AUTOSAVE_KEY` `settings.ts:50`, default on `settings.ts:237` | W4 |
| **Export formats** PNG / JPEG / WebP, with quality 1–100 (JPEG/WebP only) | `settings.ts:10` (`ExportFormat`), picker `settings.ts:939-944`, default quality 90 `settings.ts:63`, encode `export.rs:85-166`, format enum `export.rs:28-83` | W0 spike (PNG) |
| **Filename pattern** — `{date}`, `{time}`, `{datetime}`; default `SnagCraft-{date}-{time}` | `settings.ts:65` (`DEFAULT_PATTERN`), tokens hint `settings.ts:966-969`, `buildFilename` | W9 |
| **Default save dir** — OS Pictures/SnagCraft; Change… / Reset to default | `commands.rs:230-269` (`default_save_dir`, `pick_save_dir`), `ensureSaveDir` `app.ts:1717-1731`, change/reset `:1733-1762` | W9 |
| **Separate recording folder** (`splitFolders`, default off) | `settings.ts:141`, `settings.ts:919-932`, `getRecDir` `app.ts:4439` | W11 |
| **Copy to clipboard as PNG** (`Ctrl+C`) | `app.ts:4422-4432`, `copy_png` `commands.rs:299-303`, `clipboard.rs:6` | W2 |
| **Paste** an image from the clipboard into the editor (`Ctrl+V`, `paste` event, first `image/*` item) | `app.ts:5081-5094` | W2 |
| **Watermark on export** — text, opacity 2–100 (default 45 %), corner bottom-right/bottom-left/top-right/top-left | `watermark.ts:13-49` (`DEFAULT_WATERMARK` :28-33 = opacity 0.45, bottom-right), draw :62-90 (Inter 600), UI `settings.ts:972-984` | W4 |
| **New blank canvas** (`Ctrl+N`) — 8 presets (Default 1280×800, HD, Full HD, QHD, 4K UHD, Square 1080², Story 1080×1920, Small 800×600) as radio cards (click fills the fields, matching card highlights), white or transparent background, dims clamped 16–8192, last size/bg remembered | `app.ts:4939-4975` (`NEW_PRESETS` :4941-4950, `clampDim` :4952, `blankCanvasDataUrl` :4956-4969, `newCanvas` :4962), `syncNewSelection` :5004, `readNewPrefs` :4984, `style.css:4661-4706` | W10 + W23 (the W10 tick overclaimed "white or transparent": doc.new was white-only and presets created immediately; W23 adds the bg choice, radio semantics, vertical cards, remembered prefs) |
| **Resize** the canvas (dialog opened from the toolbar, closes on Esc) | `openResize` `app.ts:3622`, popover `:5118-5121` | W10 |
| **Recent strip** — newest-first, in-session cap `RECENT_LIMIT = 12`, plus a folder scan of the save (and recording) folder with thumbnails; clicking reads from disk, recordings open in the OS player | `app.ts:672` (`RECENT_LIMIT`), `refreshRecentFromFolder` `:3946-3980`, `list_folder_media` `commands.rs:386-429`, `open_path` `commands.rs:503-532` | W9 |
| **Recent tile = live vertical film-item** — thumbnail on top, format badge + time caption below, accent ring on the loaded capture (`loadedRecentId`), hover lift; video/unreadable gets a painter-drawn film placeholder | `style.css:3411-3490` (`film-item`/`film-thumb`/`film-caption`/`film-badge`/`film-time`), `renderFilm` `app.ts:3909-3940` | W22 (was a horizontal image-and-text chip; badge/time/active all missing) |
| **Pin** (pin-a-shot-to-screen) — *removed*, must NOT be rebuilt | `CHANGELOG.md:201` ("Removed Pin") | n/a |

---

## 4. History

| Feature | Evidence | rust |
|---|---|---|
| Undo/redo of flattened canvas states, cap **80** entries | `editor.ts:115` (`MAX_HISTORY = 80`), stack :315, cursor :547 | W2 |
| `Ctrl+Z` undo · `Ctrl+Shift+Z` / `Ctrl+Y` redo | `app.ts:5194-5202`; in-canvas handler `editor.ts:1261-1276` | W0 spike |
| History reset on new document | `editor.ts:404` | W1 |
| Crop re-sizes history states so undo can restore any earlier canvas size | `editor.ts:254`, :997 | W2 |
| Text caption commit on settle (not per keystroke) | `editor.ts:1178-1179`, :435-436 | W12 |
| Editor-only decorations (crop shading) excluded from history/export | `editor.ts:245`, :1042 | W12 |
| Dirty-state derived from history depth → gates Save | `app.ts:1916-1917` (`isDirty`) | W12 |

---

## 5. Shell / OS integration

| Feature | Evidence | rust |
|---|---|---|
| **Floating widget** — frameless always-on-top window, two states: folded tab **58×74**, expanded pill **292×50** (window 304×60) | `windows.rs:186-260` (sizes doc :205-213, `WIDGET_LABEL` :203, `build_widget` :228, always-on-top :259), `widget.css` | W3 |
| Widget buttons: Full, Region, **Rec** (red dot → Stop square mid-take), fold, close, drag grip; `Rec` tooltip "Record the screen (V)" | `widget-main.ts:9-27` (invoke modes :26-27), buttons :210-245, rec icon :104-105, tip :226 | W4 |
| Widget `V` shortcut → recording; mirrors `recording-state` broadcast from any window | `widget-main.ts:412-416`, tip :226 | W4 (widget V / main Ctrl+Alt+V) |
| Widget keyboard: Enter/Space activate the tab (a11y) | `widget-main.ts:336-339` | W3 |
| Widget parks top-right, kept in place across states | `windows.rs:270-287` (`widget_apply_size` :288) | W3 |
| **Tray**: left/double click restores+focuses main window; menu Show SnagCraft / Capture / Floating widget / ─ / Quit; right-click menu, secondary-activate = middle click | `tray.rs:6-25` (docs), `init_tray` :55-100 (items :56-63, `show_menu_on_left_click(false)` :74, ID_SHOW :76, capture emit :80, quit :85) | W3 |
| **Global hotkeys** via `org.freedesktop.portal.GlobalShortcuts` (Wayland-only path); actions `capture-full`, `capture-region`; default triggers `Print` / `SHIFT+Print` | `hotkeys.rs:1-28` (module doc), status enum :45-75, `xdg_trigger` :156-196, `normalize_key` :197, `description_for` :234-237; defaults `settings.ts:66-67`; frontend listen `app.ts:5265-5269` | W3 |
| Re-binding allowed only per new portal session (one BindShortcuts per session) | `hotkeys.rs:28` | W3 |
| **Close guard** — `CloseRequested` is always prevented, then a decision tree runs: force-quit flag → exit; **dirty document** → emit `close-requested` so the frontend shows its own non-blocking save modal (which calls `force_close`); widget visible → hide main, keep the pill; "Minimize to tray" + a real tray present → hide + `window-hidden-to-tray`; otherwise exit | `windows.rs:26-28` (flags), `request_quit` `:89-94`, `handle_close` `:96-140` (`:100`, `:105-110`, `:112-122`, `:126-137`), `register_close_guard` `:142-154`, `force_close` `:156-160`; setting `app.ts:1657` | W3 |
| **Single instance** — a second launch forwards argv to the running app, which re-dispatches it | `lib.rs:30-33` (`tauri_plugin_single_instance::init`), `cli.rs:5` | W3 |
| **Start at login** via `tauri-plugin-autostart` (XDG autostart on Linux, Run key on Windows) | `lib.rs:35-39`, UI `settings.ts:804-806`, README:127 | W3 |
| **File associations** — open images with SnagCraft from the file manager | `tauri.conf.json:47` (`fileAssociations`), `cli.rs:45-57` (path in argv → `file`) | W3 |
| **CLI** `snagcraft --capture` / `--capture-full` / `--region` / `--capture-region`, `--help` early exit, image path as arg | `cli.rs:45-60` (`parse_args`), `early_exit` :82-95, `dispatch` :62-75, `launch_request` :77-80 | W3 |
| Frontend glue: `tray-capture`, `cli-capture`, `cli-open`, first-launch `launch_request` poll, `hotkey-activated` | `app.ts:5222-5269` | W3 |
| PrintScreen = full, Shift+PrintScreen = region (in-app fallback) | `app.ts:5128-5132` | W11 |

---

**Capture sources** (mode union `full | region | window | monitor | scrolling | interval`, `app.ts:36`): full-screen `capture.rs:15-29`, window `capture.rs:46-99`, monitor `capture.rs:54-104`; Linux portal path `portal_capture.rs:80` (`ashpd` Screenshot); commands `capture_screenshot`/`capture_window`/`capture_monitor` `commands.rs:78-145`; Capture-menu modes and labels `app.ts:838-870`.

---

## 6. Recording

| Feature | Evidence | rust |
|---|---|---|
| Linux pipeline: ScreenCast portal session → PipeWire fd → frame conversion → ffmpeg stdin (two non-shared halves) | `recording.rs:1-18` | W4 |
| Commands: `start_recording(format, dir, filename, region?, audio?, source?)`, `stop_recording`, `is_recording`, `pause_recording`, `resume_recording`, `list_audio_sources` | `recording.rs:19-28` | W4 |
| Containers **MP4** (default) / **GIF**; GIF is silent by construction | `recording.rs:23,31-32`; `RecFormat` `settings.ts:12`; picker `settings.ts:951-955` | W4 |
| **Region recording** — `{left,top,width,height}` in stream px, every frame cropped, ffmpeg sized to even-padded region | `recording.rs:41-44` | W4 |
| **Source** `monitor` (default) vs `window` — narrows what the portal picker offers | `recording.rs:34-35` | W4 |
| **Pause/Resume** — frames dropped consumer-side, session + stream + ffmpeg survive, resume writes the same file; paused time leaves no frames | `recording.rs:37-39`; UI button `app.ts:819`, `recPauseBtn` `:1320`, paused-total bookkeeping `:1472` | W4 + W18 (engine pause/resume W4; HUD clock freeze at the pause moment + paused-span skip on resume W18 — live `paintRecTimer` `app.ts:2289-2292`) |
| **Audio** — mic (`list_audio_sources` id) and/or default output **monitor**, mixed into one AAC track; never an error when empty | `recording.rs:30-32`; pretty-names `:147-200`; UI mic select `settings.ts:855-863`, system-audio switch `settings.ts:866-870`; `list_audio_sources` wiring `app.ts:4453-4461` | W4 + W13 (single-source W4; mic+monitor `amix` mixing, `@DEFAULT_MONITOR@`, settings fallback W13 — live-verified on pipewire-pulse) |
| `recording-state {recording, path}` events so a window that missed the invoke result resyncs | `recording.rs:46-47`; widget mirrors it `widget-main.ts:412-416` | W4 |
| Elapsed timer pill, repainted every 500 ms while a take runs | `app.ts:2326-2340` | W4 |
| Recording filename `SnagCraft-{date}-{time}.{format}` (tokens expanded at start, not an epoch) | `app.ts:2368-2369` (`recordingFilename`) | W18 |
| Stop flow: "Finishing the recording…" → "Recording saved: {path}" + immediate Recent-rail pickup; an empty path reports "no file was written" | `app.ts:2497-2515` (`stopRecording`), `:5313-5321` (state mirror) | W18 |
| Start flow: waiting status names format/region; success "Recording — click Stop when you're done."; errors classified (neutral cancel / permission-recovery hint / portal-log pointer); a capture in flight blocks a new take | `app.ts:2402-2405`, `:2439-2444`, `:2475`, `:2481-2494` | W18 |
| HUD "Region" scope tag on region takes (absent on whole-screen) | `app.ts:2308-2310` (`paintRecording` / `recScopeEl`) | W18 |
| Recording thumbnails for the Recent rail | `commands.rs:533-545` (`recording_thumb`) | W9 (film-icon placeholder; W22 painter-drawn film frame + play triangle — the bundled fonts have no ▶ glyph) |
| Copy diagnostics (portal + per-take `.rec.log`) to clipboard | `commands.rs:457-501`, `settings.ts` troubleshooting button `:996-1004` | W11 |
| **Trim** a finished take | *not implemented in v1.5.3* — no trim command or UI (grep `trim` in `recording.rs`/`app.ts` is string-truncation only) | pending |
| ffmpeg is a hard runtime dependency; absence is reported as an actionable message | `timelapse.rs:17-18` (same convention for recording), README:128 | W12 |

---

## 7. Scrolling capture

| Feature | Evidence | rust |
|---|---|---|
| Manual-scroll stitching: select region once, cast the monitor, grab the newest frame, align to the last accepted one, append only new rows → one tall PNG. No synthetic input. | `scrolling/mod.rs:1-8` | W4 |
| Commands `scrolling_start(region)`, `scrolling_done() → {dataUrl,width,height}`, `scrolling_cancel()`, `set_click_through(ignore)` | `scrolling/mod.rs:9-14` | W4 + W20 (start/stop/poll W4; `scroll.cancel` discard W20 — the W4 tick overclaimed scrolling_cancel) |
| `scrolling-status {frames,height,confidence,warning,finished}` events; `finished` fires exactly once | `scrolling/mod.rs:16-21` | W4 |
| Stop rules: 2.5 s idle after scrolling starts, two identical consecutive frames, **40 000 px** height cap, or explicit Done | `scrolling/mod.rs:26-27` | W4 |
| Region is in **physical (stream) px**, not CSS px | `scrolling/mod.rs:23-24` | W4 |
| Teardown order: stitcher thread → PipeWire thread → drop portal `Session` | `scrolling/mod.rs:28-30` | W4 |
| Sub-modules: frame alignment, stitch accumulation, source/cast, capture | `scrolling/align.rs`, `scrolling/stitcher.rs`, `scrolling/source.rs`, `scrolling/cap.rs` (`stitcher.rs:183`) | W4 |
| UI: freeze, select the region, then hand the wheel back to the user; Capture → Scrolling mode | `app.ts:2719`, mode `app.ts:36` | W4 |
| Cancel discards the partial scroll — HUD Cancel button + Esc, nothing written; a cancel during the portal wait applies when the handle lands | `app.ts:2757-2763` (`scrolling_cancel`) | W20 |
| Done summary "Scrolling capture: W × H, N frames · saved {filename}" + immediate Recent pickup; the result is saved under the normal capture filename pattern (deduped) | `app.ts:2770-2786` (`writeImage` + `addRecent`) | W20 |
| Error classification: locked-screen, 40 000 px height-cap and screen-sharing-refusal recovery hints | `app.ts:2586-2599` (`scrollErrorText`) | W20 |

---

## 8. OCR

| Feature | Evidence | rust |
|---|---|---|
| Extract text from the current capture; language `eng` \| `swe`, stored in `snagcraft:ocr-lang`; "Capture something first" guard; empty result suggests another language, success points at Copy | `app.ts:671-676` (`OCR_LANG_KEY`, `OCR_LANGS`), toolbar chip `:906`, `runOcr` `:4149-4179` | W7 + W21 (guard + live result wording W21; the worker also runs tesseract WITHOUT the session lock now) |
| Modal: language select, "Extract text", busy "Reading…", read-only textarea, live status | `app.ts:1265-1287` (lang `:1278`, run `:1283`, busy `:1284`, textarea `:1286`) | W7 |
| Backend: `ocr_image(dataUrl, lang?)`, `ocr_available()`, `ocr_languages()` | `ocr.rs:183-206`; `sanitize_lang` `:49`, `clean_output` `:72` | W4 |
| Copy the recognised text to the clipboard — disabled until there is text | `app.ts:4181` (`copyOcrText`, `ocrCopyBtn.disabled` `:4162`), `copy_text` `commands.rs:431-435` | W4 + W21 (disabled-until-text affordance W21) |

---

## 9. Timelapse / interval capture + HUD

| Feature | Evidence | rust |
|---|---|---|
| Interval loop: one PNG per tick into the save folder (live `SnagCraft-interval-*.png`; rebuild `snagcraft-timelapse-NNNNN.png` — zero-padded index = playback order) | `timelapse.rs:1-6`, `app.ts:2820-2830` | W4 |
| Stop converts the run to MP4 — auto `make_timelapse` at 10 fps into the run folder (`SnagCraft-timelapse-{date}-{time}.mp4`); zero shots report "nothing was saved"; a failed render never costs the shots | `app.ts:3019-3047`, `timelapse.rs:100` | W15 |
| `make_timelapse(dir, pattern, fps, out)` — glob resolved inside `dir` (no separators), ffmpeg `-pattern_type glob`, zero-padded numbers give playback order | `timelapse.rs:8-18`, `:100` | W7 |
| Interval period setting (default **30 s**), select "every N s / every 5 min" | `settings.ts:69` (`DEFAULT_INTERVAL`), select `settings.ts:837-842` | W4 |
| Capture → Interval menu entry ("a screenshot every few seconds, then a timelapse") + per-mode banner copy | `app.ts:865-870`, `app.ts:2239-2243` | W4 |
| **`Ctrl+.`** stops a running interval capture (same as Stop) | `app.ts:5181-5188` | W8 |
| One live loop guarded by an `IntervalRun` record with a zero-padded `seq` counter | `app.ts:2820-2830` | W4 |
| **Countdown HUD** — the shell shrinks the window to a 280×228 HUD and hides `#app` (`set_hud_mode`), timer parented to `<body>` so it stays visible; older backends fall back to a full-window overlay | `app.ts:1951-1984` (`set_hud_mode(true,280,228)` `:1983`) | W8 |
| Capture delay pills (0 s … 60 s, `DEFAULT_TIMER = 0`) | `settings.ts:64`, `settings.ts:830-836`, `settings.ts:219-227` | W4 |

---

## 10. Upload

| Feature | Evidence | rust |
|---|---|---|
| Provider-agnostic: POST the flattened PNG to a user-configured endpoint, copy back whatever link the server returns; ships no credentials, no hardcoded service | `upload.rs:1-17` (`upload_image` `:17`) | W8 |
| Transport is `curl` (no bundled TLS stack) | `upload.rs:8-9` | W8 |
| File → **Upload…** menu item | `app.ts:797`, dispatch `:4572-4573` | W8 |
| Flow: refuse without an image, require an endpoint ("Set an upload endpoint in Settings → Export first" + open the panel), "Uploading…", then copy the link and say so — off-thread, the editor never blocks on curl | `app.ts:4499-4523` (`:4501`, `:4504-4506`, `:4511`, `:4518-4520`) | W8 + W21 (the W8 tick overclaimed "Uploading…": the rebuild blocked the UI on curl up to 30 s until W21 moved it off-thread with a brief render lock) |
| `open_url` for the returned link | `upload.rs:89` | W4 |

---

## 11. Settings panel items (`src/ui/settings.ts`)

Sections rendered: General, Capture, Recording audio, Global hotkeys, Save location, Export, Troubleshooting, Theme — `settings.ts:795-1010`.

| Item | Default | Evidence | rust |
|---|---|---|---|
| Minimize to tray | off | `settings.ts:799-802`, `TRAY_KEY` `:43` | W3 |
| Start at login (hidden if unsupported) | off | `settings.ts:804-806` | W3 |
| Show floating widget | — | `settings.ts:813-816` | W3 |
| Auto-save captures | **on** | `settings.ts:820-823`, `AUTOSAVE_KEY` `:50`, default `:237` | W3 setting / W4 behavior |
| Capture delay (seconds pills) | 0 | `settings.ts:830-836`, `:64` | W3 |
| Interval capture period | 30 s | `settings.ts:837-842`, `:69` | W11 |
| Microphone source | None | `settings.ts:855-863` | W11 |
| System audio (output monitor) | off | `settings.ts:866-870` | W11 |
| Enable global hotkeys | off | `settings.ts:877-880`, `HOTKEYS_ENABLED_KEY` `:45` | W11 |
| Capture full screen trigger | `Print` | `settings.ts:883-888`, `:66` | W3 |
| Capture region trigger | `SHIFT+Print` | `settings.ts:890-895`, `:67` | W3 |
| "Register hotkeys" button + status line | — | `settings.ts:899-902`, `:903` | W11 |
| Save folder: path, Change…, Reset to default | OS Pictures/SnagCraft | `settings.ts:910-918` | W3 (folder + Choose…; no Reset) |
| Separate recording folder + its own Change/Reset | off | `settings.ts:919-932`, `:141` | W11 |
| Export format (PNG / JPEG / WebP) | **PNG** | `settings.ts:939-944`, `DEFAULT_FORMAT` `:61` | W3 |
| Recording format (MP4 / GIF) | MP4 | `settings.ts:951-955` | W11 |
| Export quality slider 1–100 (hidden for PNG) | 90 | `settings.ts:961-963`, `:63` | W3 |
| Filename pattern + token hint + live preview | `SnagCraft-{date}-{time}` | `settings.ts:966-969`, `:65` | W11 |
| Watermark text (empty = none) | "" | `settings.ts:972-974` | W4 |
| Watermark opacity | 45 | `settings.ts:977-978`, `watermark.ts:30` | W4 |
| Watermark corner (BR/BL/TR/TL) | bottom-right | `settings.ts:980-984`, `watermark.ts:32` | W4 |
| Upload endpoint | "" | Export section `settings.ts:935-995`; consumed `app.ts:4504` | W4 (field + open page; no POST) |
| Troubleshooting: copy diagnostics | — | `settings.ts:996-1004` | W11 |
| Theme picker + version footer | light | `settings.ts:1005-1010`, `:339` | W3 (picker; no version footer) |

---

## 12. Themes

| Feature | Evidence | rust |
|---|---|---|
| Two themes, **light** (default) and **dark**, persisted under `snagcraft:theme`, applied as `data-theme` + `color-scheme` on `<html>` | `settings.ts:11` (`Theme`), `:14` (`THEME_KEY`), `readTheme` `:18-24`, `applyTheme` `:26-38` | W3 |
| Accent light `#2266b3`, dark `#3d7ec2` | `src/style.css:133` (`--accent-solid: #2266b3`), `src/style.css:178` (`#3d7ec2`) | W3 |
| Widget accent: dark `#6fa3dc`/`#9cc0ef`, light `#2266b3`/`#1a5296` + soft/press alphas | `src/widget.css:14-17`, `:42-45` | W3 |
| Theme switch from the settings panel and the shell | `settings.ts:341` (`setTheme` option), wired `app.ts:4443`, `settings.ts:1005-1010` | W3 |

---

## 13. Shortcuts sheet — every key the app documents (`app.ts:1190-1230`)

| Key | Action | Evidence | rust |
|---|---|---|---|
| `PrintScreen` | Capture full screen | `app.ts:1200`, handler `:5128-5132` | W11 |
| `Shift+PrintScreen` | Capture a region | `app.ts:1201`, handler `:5130` | W11 |
| `Ctrl+Z` | Undo | `app.ts:1202`, `:5194-5198` | W0 spike |
| `Ctrl+Shift+Z` · `Ctrl+Y` | Redo | `app.ts:1203`, `:5196-5202` | W0 spike |
| `Ctrl+C` | Copy to clipboard | `app.ts:1204`, `:5203-5205` | W2 |
| `Ctrl+V` | Paste an image to annotate | `app.ts:1205`, listener `:5081` | W2 |
| `Ctrl+scroll` | Zoom in / out | `app.ts:1206` | W12 |
| `V` `C` `A` `T` `R` `B` `E` `G` `L` `H` `K` `S` `M` | Select, Crop, Arrow, Text, Shapes, Blur, Brush, Fill, Spotlight, Highlighter, Callout, Step, Magnify | `app.ts:1207-1226`, map `:5156-5170` | W2 |
| `Ctrl+N` | New blank canvas | `app.ts:1227`, `:5213-5216` | W12 |
| `Ctrl+O` | Open an image | `app.ts:1228`, `:5210-5212` | W12 |
| `Ctrl+S` | Save | `app.ts:1229`, `:5206-5209` | W12 |
| `Ctrl+Shift+S` | Save As… | `app.ts:1230`, `:5208` | W12 |
| `Ctrl+,` | Settings | `app.ts:1231`, `:5189-5193` | W3 |
| `Delete` / `Backspace` | Delete selected annotation | `app.ts:1232` | W12 |
| `Esc` | Cancel / deselect / close (also closes About, New, Resize, Shortcuts) | `app.ts:1233`, `:5108-5127` | W2 |
| `?` | Shortcuts help | `app.ts:1234`, `:5138-5146` | W12 |
| `Ctrl+.` | Stop interval capture (documented in code, not in the sheet) | `app.ts:5181-5188` | W8 |
| `Enter` in crop | Commit the crop | `tools.ts` crop `1462-1557`, hint `app.ts:354` | W2 |
| `Enter`/`NumpadEnter` in text | Commit the caption | `app.ts:2691` | W2 |
| `Enter`/`Space` on the widget tab | Expand the pill | `widget-main.ts:336-339` | W3 |

---

## 14. About box

| Feature | Evidence | rust |
|---|---|---|
| Modal with logo mark, name, version (from backend `get_version`), one-line description, repo `github.com/rostaddotcc/snagcraft`, license `GPL-3.0-or-later · commercial license available`, OK button | `app.ts:1169-1188` (version `:1181`, repo `:1183`, license `:1184`), `commands.rs:437-444` | W12 + W19 (license line W19 — the W12 tick overclaimed it; vision caught it) |
| Opened from File → About (menu shows the version as its shortcut hint); Esc / OK close it | `app.ts:803`, `:4582`, `openAbout` `:4854-4866` | W12 |

---

## Wave coverage at W23 (2026-11-19)

The W1-era scope note that used to sit here ("not started at all: shell, recording,
scrolling, OCR, timelapse, upload, settings, themes, shortcuts, About") is obsolete —
W2–W5 landed most of it, and W7–W23 closed the rest. The W6b summary that used to
sit here (45 `pending`) was stale: those rows were closed by W7–W14 but the table
was never recounted. Current state of the checklist (recounted row-by-row at W22,
W23 extended existing rows without adding new ones):

| Status | Rows |
|--------|------|
| `W2` (incl. 3 `W2 + W14`) | 33 |
| `W4` (incl. `W4 + W13`, `W4 + W18`, `W4 + W20`, `W4 + W21`) | 33 |
| `W3` | 30 |
| `W11` | 14 |
| `W12` (incl. `W12 + W19`) | 13 |
| `W8` (incl. `W8 + W21`) | 7 |
| `W0 spike` | 5 |
| `W9` | 5 |
| `W18` | 4 |
| `W7` (incl. `W7 + W21`) | 3 |
| `W20` | 3 |
| `W1` | 2 |
| `W10` (incl. `W10 + W23`) | 2 |
| `W15` | 1 |
| `W19` | 1 |
| `W22` | 1 |
| `pending` (Trim — rostad-gated) | 1 |
| `n/a` | 1 |
| **total** | **159** |

What the waves delivered, per `docs/plans/rust-rebuild-status.md`: 13-tool annotation
parity with 10 object kinds and 5 shape variants (W2); tray, global hotkeys,
autostart, single instance, floating widget, close guard, settings panel and themes
(W3); recording (MP4/GIF, region, pause/resume), scrolling stitch, OCR,
interval/timelapse frames and watermark (W4); CLI, TCP control, MCP and the WASM web
app (W5); AppImage + `.deb` + CI artifacts (W6a); OCR language/busy UI + timelapse
assemble command (W7); `Ctrl+.`, compact countdown HUD, upload POST (W8); save-dir /
Save As / filename tokens / Recent strip (W9); New-canvas presets + Resize (W10);
the settings cluster + diagnostics copy + PrintScreen (W11); shortcuts overlay +
About + derived dirty-state (W12); mic + system-audio mixing (W13); select-mode
resize handles, double-click text/callout re-edit, brush nib preview, crop
click semantics (W14); **stop-time timelapse → MP4 auto-assembly in the UI flow
(W15 — the W4 tick on the old combined row overclaimed: the loop shipped, the
stop→video conversion did not until W15).** W16 closed the web clipboard/drag-drop
limits (gaps.md C2/C3). **W18 walked the recording flow end-to-end against the live
anchors and closed seven residuals the W4 ticks had papered over: live filename
pattern, stop→Recent pickup + empty-stop report, live start/stop status wording,
start-error classification, capture-busy guard, pause-aware HUD clock, and the
Region scope tag (four new rows + the Pause/Resume row split).** W19 walked the
save/export flow: full live `saveEdited` semantics (guards, `-edited` copy, "Saved
copy" wording, save point, Recent pickup), Save As cancel report, auto-save
outcomes all visible (the failure was swallowed), toolbar/close-guard Save parity,
the editable save kept as an explicit File-menu addition, and the About license
line the W12 tick had overclaimed (vision caught it). **W20 audited the last
headline flow, scrolling capture: Cancel/discard (HUD button + Esc + engine
scroll.cancel, including cancels during the portal wait), the live done summary
+ Recent pickup + pattern filename, and scrollErrorText classification — the W4
commands row had overclaimed scrolling_cancel.** W21 re-audited OCR + upload:
live OCR guards/result wording + Copy-disabled-until-text, upload moved OFF-THREAD
(the W8 tick overclaimed "Uploading…" — curl blocked the UI up to 30 s), missing
endpoint opens Settings, "Uploaded — link copied" wording; both workers now run
their subprocesses WITHOUT the session lock (the toolbar locks it every frame).
**W22 rebuilt the Recent tile as the live vertical film-item (thumbnail on top,
format badge + time caption, accent ring on the loaded capture, hover lift) — the
rebuild had a horizontal chip with none of that — and replaced the ▶ video
placeholder with a painter-drawn film icon (the bundled fonts have no ▶ glyph).**
**W23 finished the New-canvas dialog against the live design: the white/transparent
background choice (the W10 tick overclaimed it — doc.new was white-only), radio
preset cards that fill the fields and highlight the match, and remembered size/bg
(the live localStorage prefs, now persisted settings).**

The single `pending` row is **Trim** — deliberately out of parity (the live v1.5.3
has no trim) and gated on rostad's build-or-drop decision (`docs/gaps.md` A2).
Every other row is backed by live code. The remaining open items in `docs/gaps.md`
are rostad gates (A1–A4), web-MVP limits (C1–C3) and future platform ports (D1–D2).

**Coverage:** 159 rows across 14 sections. Every TS line number came from a grep/read
against commit `3cec2ee` (baseline v1.5.3) during W1; the only exceptions are the two
`CHANGELOG.md` citations, marked as such. Rust statuses were re-audited at W6b as
described at the top of this file, recounted at W15, and flow-audited at W18–W23.
