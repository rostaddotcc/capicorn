# SnagCraft — Wave 25 plan (v0.25.0)

Goal: build every remaining feature the user asked for, **except AI redaction**
(postponed), and rebuild **Magnify** so the marked area is really enlarged live
with a chosen zoom (e.g. 10×).

## Ownership map (disjoint files — no two tracks touch the same file)

| Track | Owns | Delivers |
|---|---|---|
| **A — Magnify LIVE** | `src/editor/tools.ts`, `src/editor/editor.ts` | real lens render (source image scaled by `zoom`), size slider 1.5–10×, export special-case removed |
| **B — Interval + UI** | `src/ui/app.ts`, `src/style.css`, `src/ui/settings.ts`, `src-tauri/src/timelapse.rs` (new) | Capture → **Interval** mode + HUD, settings row, timelapse command + button; magnify tool entry; audio settings rows; Record window; Pause/Resume; About/version untouched |
| **C — Audio backend** | `src-tauri/src/recording.rs` | `list_audio_sources`, `AudioConfig` on `start_recording`, pause/resume gate, `source: monitor\|window` |
| **D — Parent (me)** | `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`, `src/editor/watermark.ts` (new), `src-tauri/src/upload.rs` (new) | register commands, watermark export pass, generic upload command, integration patches |

## Locked contracts

### start_recording (Track C)
```rust
#[derive(Deserialize)]
pub struct AudioConfig { pub mic: Option<String>, pub system: bool }

pub async fn start_recording(
    app, state, format: String, dir: String, filename: String,
    region: Option<Region>,
    audio: Option<AudioConfig>,     // NEW
    source: Option<String>,         // NEW: "monitor" | "window"
) -> Result<(), String>
```
- Linux: `-f pulse -i <mic>` / system audio = `@DEFAULT_MONITOR@`; mix with
  `-filter_complex amix=inputs=N` → `-map 0:v -map [aout] -c:a aac -b:a 160k`.
- GIF has no audio → drop the audio inputs for `gif`.
- `list_audio_sources() -> Vec<AudioSource{id,name}>` via `pactl list short sources`
  (Windows: `ffmpeg -list_devices true -f dshow -i dummy` parse; empty on failure).
- `pause_recording()` / `resume_recording()` → `AtomicBool` gate; the frame loop
  skips writing while paused.

### magnify (Track A)
- `ToolHost` gains `magnifyZoom()`, `sourceImage()`, `imageOrigin()`.
- `MagnifyRegion._render`: clip → `scale(zoom)` → draw the source image about the
  lens centre → chrome + `N×` badge. `objectCaching:false`.
- `TOOL_SIZE_RANGES.magnify = {min:1.5, max:10, step:0.5}`.

### UI (Track B)
- `CaptureMode` gains `"interval"`; menu row after Scrolling.
- Interval card: `set_hud_mode(272,168)` + shots/elapsed + Stop + "Stop + timelapse".
- Settings: `intervalSeconds` (5/10/30/60/300), `mic` (device id or ""), `systemAudio`, `recSource` ("monitor"|"window").
- Record menu: Record full screen / Record region / **Record window**.
- Recording pill: + Pause/Resume button.
- `ShellToolId` gains `"magnify"` + TOOLS entry + `M` shortcut + "Zoom" × slider.

### Upload (Track D)
```rust
#[tauri::command]
pub async fn upload_image(data_url: String, endpoint: String) -> Result<String, String>
```
POST `multipart/form-data` (`file` field) → trimmed response body = the link.

## Definition of done (this wave)
- `cargo test --lib` green, `tsc --noEmit` 0, `pnpm build` green.
- AppImage + deb + Windows `.exe`/`.msi` from CI, release v0.25.0 with notes.
- Verified in Chromium: interval card, magnify lens, audio settings rows, record menu.

## Wave 25b — user follow-ups (after the three tracks land)

### Open an image (use SnagCraft as a quick editor)
- File → **Open…** + `Ctrl+O`, using `tauri-plugin-dialog` (`open({ multiple:false,
  filters:[{name:'Images', extensions:['png','jpg','jpeg','webp','gif','bmp','avif']}] })`)
  → read the file → `editor.load(dataUrl)` → status "Opened <name>".
- Drag-and-drop and `Ctrl+V` already load an image; make the Open path share the same
  helper so file name, dirty state and recents behave identically.
- Recent list: an opened file should not be listed as a "capture"; add it as an "Open" entry.

### Shadow control, usable by hand
Replace the numeric `X offset` / `Y offset` inputs with:
- **Angle** dial/slider 0–360° (the direction the shadow falls)
- **Distance** 0–60 px
- **Blur** 0–60 px
- **Opacity** 2–100 %
- **Colour** swatch (existing)
- a **live preview tile** (a rounded box on a checker tile) that draws the shadow exactly as
  it will look, driven by the same values.

`offsetX = cos(angle) * distance`, `offsetY = sin(angle) * distance` — the stored
`ShadowStyle` keeps offsetX/offsetY, so existing images and presets stay valid.
