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

## Appendix — measured reality

| Claim | Measured | What changed |
|---|---|---|
| pipewire 0.10 binds out of the box on pn53 | libspa-sys build.rs panicked: clang can't find stdbool.h (no clang driver installed, only libclang) | rust/.cargo/config.toml [env] BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/15/include" — recording scaffold now compiles. Path is pn53-specific (gcc 15); CI ubuntu-latest has a clang driver so the extra -I is harmless there. W4a children inherit a compiling tree. |