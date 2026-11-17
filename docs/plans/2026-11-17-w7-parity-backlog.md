# W7 — Parity backlog: timelapse video assembly + OCR language/busy

> **For Hermes:** post-W6 parity backlog (docs/gaps.md B4 rows 1–3). Closed
> offline on pn53 (ffmpeg 8.0.1 + tesseract present). No rostad gate needed
> for these (not trim/relicense/cutover/external). Rows 4–9 (Ctrl+., countdown
> HUD, upload POST) deferred — need live session / network scope decision.

**Goal:** close parity rows B4#1 (OCR language picker), B4#2 (OCR busy modal),
B4#3 (timelapse ffmpeg video assembly) with offline evidence.

## LOCKED CONTRACTS

### Feature 1 — timelapse.assemble (engine + capture/recording)
New native command `timelapse.assemble {dir, pattern?, fps?, out?}`:
- dir: directory of PNG frames (required, must exist). pattern: glob default
  "frame-*.png" (validate: no shell metachars beyond glob * ? [ ]; reject
  absolute/.. ). fps: 1..=60 default 10 (hostile-safe clamp). out: output mp4
  path default dir/timelapse.mp4.
- ffmpeg subprocess: `ffmpeg -y -framerate <fps> -pattern_type glob -i
  '<dir>/<pattern>' -c:v libx264 -pix_fmt yuv420p -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2"
  <out>` (even-dims pad for yuv420p). Reuse recording::ffmpeg spawn/wait
  pattern (bounded wait, kill on timeout). Capture stderr for error surfacing.
- Returns {video: out, frames: <count matched>, fps}. Errors: no frames matched
  → CommandError::InvalidParam; ffmpeg missing/fail → CommandError::Io with
  stderr tail. Register in native registry (web: absent). Add to commands.list.
- Offline test: write N temp PNG frames (snagcraft_raster), assemble, ffprobe
  the mp4 (codec h264, nb_frames ~ N, dims even). Skip-if-no-ffmpeg guard.

### Feature 2 — OCR language picker (ui-egui + io settings)
- io::settings: add `ocr_lang: String` default "eng" (serde default, validate
  non-empty ASCII alphanumeric/-/_ , clamp length). Round-trip + old-file test.
- ui-egui OCR window: language ComboBox populated from `ocr.languages` command
  (cached; fallback ["eng"] if unavailable). Selected lang persisted to
  settings.ocr_lang via settings.set. run_ocr uses selected lang instead of
  hardcoded "eng". kittest: picker shows languages, selection changes run_ocr
  payload (assert via a seam or settings read).

### Feature 3 — OCR busy modal (ui-egui)
- While ocr.run executes off-thread, OCR window shows a busy/progress state
  ("Reading text…" + spinner/disabled Extract button), cleared on result/error.
  OcrState gains a Busy variant or busy flag. kittest: inject busy → snapshot
  shows progress; result → text shown.

## Gates
cargo test --workspace (native, all green incl new), clippy --workspace
--all-targets -D warnings, fmt, xtask layers. Web check still compiles
(timelapse.assemble native-gated; OCR picker works on web? ocr is native-only
→ OCR window hidden on web already, picker cfg-gated native). ffprobe evidence
for the mp4. Tick parity rows B4#1–3 with evidence; update gaps.md (move 1–3
to closed). NO git by child; parent commits.

## Appendix — measured reality