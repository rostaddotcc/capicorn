# W4 — Recording, Scrolling Capture, OCR, Timelapse, Upload/Watermark (skeleton)

> **For Hermes:** draft prepared wake #12 while W3b runs. Refine + dispatch after
> W3 sign-off. Proven recipes: tauri-desktop-app skill references/recording.md,
> references/scrolling-capture.md, references/smart-redaction.md (OCR TSV path).

**Scope (spec §5 W4):** recording + scrolling capture + OCR + timelapse + upload/watermark.
~5.5k lines of live Rust to port (src-tauri/src/): recording.rs 2665, scrolling/ 2323,
ocr.rs 291, timelapse.rs 147, upload.rs 110. Strip tauri:: glue; engines sans-I/O
where possible, subprocess/portal at L4.

**Prerequisites VERIFIED on pn53 (wake #12):** libpipewire-0.3 1.6.2 + libspa-0.2
dev headers ✓, ffmpeg 8.0.1 ✓, tesseract 5.5.0 ✓. pipewire crate 0.10 (v0_3_34
feature) per live Cargo.toml; bindgen needs BINDGEN_EXTRA_CLANG_ARGS (skill recipe).

## Sub-waves
- **W4a — recording backend (L4 crates/recording or capture::recording):**
  ScreenCast portal session (ashpd screencast feature) → PipeWire fd+node →
  ffmpeg subprocess (MP4 h264/aac, GIF palettegen/paletteuse); Region/monitor/window
  sources; pause/resume (ffmpeg stdin 'q' stop); audio sources (mic monitor via
  pipewire, list_audio_sources); RecordingState machine. Port from recording.rs;
  two threads joined by fd+node id; session kept alive; GREP_DEADLOCK_NOTE teardown
  (leak PipeWire objects on exit — documented deadlock). Probe offline first
  (local Video/Source test pattern) before portal leg (needs unlocked session).
- **W4b — scrolling capture (L4):** reuse ScreenCast source (CursorMode::HIDDEN,
  no ffmpeg); manual-scroll stitch: row-signature + MAD aligner (align.rs),
  stitcher.rs overlap blend, 40 000 px cap, stop rules; synthetic-frame unit test
  (skill reference). UI: click-through overlay guidance.
- **W4c — OCR + timelapse + upload/watermark (L4):** tesseract subprocess
  (sanitize_lang, tmp PNG, stdout tidy; TSV word-box path for smart redaction
  later); interval capture (timelapse.rs) + HUD; upload.rs (destination config);
  watermark export (watermark.ts parity: text, opacity 2–100 default 45, corner).
- **W4d — UI (L6):** recording controls (region/screen → MP4/GIF, trim, pause/resume,
  audio monitor/window sources), widget Rec button LIVE (V shortcut), scrolling UI,
  OCR result panel (copy text), timelapse HUD, upload settings. Engine commands for
  every action; parity ticks.

## Risks
- PipeWire bindgen compile time + clang args (parent pre-wires manifest + probe
  BEFORE dispatch, per delegation-tuning).
- Portal legs need unlocked session → rostad KDE validation; offline probes first.
- ffmpeg 8.x vs live's version: verify filter syntax (palettegen etc.) with a probe.
- Recording trim: live has NO trim (parity doc §6 gap) — spec §4 lists trim;
  decide: implement simple post-stop trim dialog (ffmpeg -ss/-to re-encode) or
  log as gap. Ask rostad if ambiguous at W4d.

## W4a/W4c LOCKED CONTRACTS (dispatched wake #18)

```rust
// crates/recording (L4, scaffold pre-wired, bindgen fixed)
pub struct Region { pub left: i32, pub top: i32, pub width: u32, pub height: u32 }
pub enum RecordingFormat { Mp4, Gif } // parse/extension
pub struct AudioSource { pub id: String, pub name: String, pub kind: String }
pub struct AudioConfig { pub source: Option<String> } // None = silent
pub struct RecordingConfig { pub format: RecordingFormat, pub region: Option<Region>,
  pub audio: AudioConfig, pub fps: u32 /*clamp 5..=60, default 30*/, pub output: PathBuf }
pub enum RecordingEvent { Started { width: u32, height: u32 }, Stopped { path: PathBuf },
  Failed(String), Paused, Resumed }
pub struct RecordingHandle; // stop() / pause() / resume() / is_active() / is_paused()
pub fn spawn_recording(config: RecordingConfig, events: Sender<RecordingEvent>)
    -> Result<RecordingHandle, RecordingError>;
pub fn ffmpeg_available() -> bool;
pub fn list_audio_sources() -> Result<Vec<AudioSource>, RecordingError>;
pub fn crop_frame_rgb24(frame: &[u8], w: u32, h: u32, region: Region) -> Vec<u8>;
// PURE + unit-tested: pixel-exact recovery, edge clamping, odd→even padding (yuv420p).
// Architecture: portal half (ashpd Screencast, tokio multi-thread, session KEPT ALIVE)
// + capture half (dedicated thread, MainLoopRc !Send) joined by OwnedFd + node id.
// ffmpeg subprocess lazily on first negotiated frame; stop = close stdin + wait.

// io::ocr (L4 module) — tesseract subprocess (system binary, NOT linked)
pub fn ocr_available() -> bool; pub fn ocr_languages() -> Vec<String>;
pub fn sanitize_lang(lang: &str) -> String; pub fn clean_output(raw: &str) -> String;
pub fn run_ocr(img: &Surface, lang: &str) -> Result<String, IoError>;
// tmp PNG → tesseract <tmp> stdout -l <sanitized> → clean_output; bounded.

// io::upload (L4 module)
pub fn open_url(url: &str) -> Result<(), IoError>; // xdg-open/linux, open/mac, cmd/start/win; http(s) only

// capture::timelapse (L4 module)
pub struct TimelapseConfig { pub period_secs: u32 /*clamp 1..=3600*/, pub count: Option<u32>, pub dir: PathBuf }
pub fn spawn_timelapse(config, events: Sender<TimelapseEvent>) -> Result<TimelapseHandle, CaptureError>;
// interval capture.full → save PNG frames; stop handle; events Frame{path}/Done/Failed.

// compose::watermark (L3 addition)
pub fn apply_watermark<F: Font>(img: &mut Surface, text: &str, opacity: f32 /*0.02..1*/,
    corner: Corner /*BottomRight default, BottomLeft, TopRight, TopLeft*/, font: Option<&F>, size: f32);
// live watermark.ts parity: default opacity 0.45, bottom-right, margin ~16px.

// engine commands (parent wires after tracks land): record.start/stop/pause/resume,
// record.state, audio.sources, ocr.run/available/languages, upload.open,
// timelapse.start/stop, doc.export watermark params.
```

**W4a critical pitfalls (from tauri-desktop-app › references/recording.md — READ IT):**
no `target.object` in stream props (pass node id only to stream.connect AFTER pumping
the loop until the node appears in the registry, 2s cap); NO SPA_PARAM_Buffers on the
consumer stream; VideoSize as RANGE choice (not fixed) for window picks; MONITOR|WINDOW
sources; keep select_sources Request handle alive until start() returns; BGRx→rgb24
stride-aware; teardown order (dropping pw objects after mainloop.run() deadlocks);
surface real portal errors + log steps to ~/.cache/snagcraft-portal.log; offline
Video/Source probe FIRST (ffprobe nb_frames/codec/duration), portal leg = rostad KDE.

## Appendix — measured reality

| Claim | Measured | What changed |
|---|---|---|
| pipewire 0.10 binds out of the box on pn53 | libspa-sys build.rs panicked: clang can't find stdbool.h (no clang driver installed, only libclang) | rust/.cargo/config.toml [env] BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/15/include" — recording scaffold now compiles. Path is pn53-specific (gcc 15); CI ubuntu-latest has a clang driver so the extra -I is harmless there. W4a children inherit a compiling tree. |