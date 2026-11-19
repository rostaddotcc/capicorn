# W25 — Windows capture + in-app monitor picker (Rust edition)

Status: PLAN (probe passed 2026-11-19). Owner: Härmes loop. Gate: rostad said
"Ja, Windows-capture blir nästa stora wave (W25+)" (2026-11-19). Tauri stays cut.

## Why

GitHub issue #1 "Multiple monitors": a dual-monitor user wants a monitor-selection
prompt. Investigation (status log 2026-11-19) shows the stable Tauri app on
Windows/macOS captures only the primary monitor (`xcap::Monitor::from_point(0,0)`,
no picker) and `capture_monitor`/`capture_window` bail "not supported on this
platform yet". Linux has the portal picker; Windows does not. The fix lands in the
Rust edition (Tauri development is cut). xcap (Apache-2.0, allowlist-clean) fully
supports Windows monitor enumeration + per-monitor/window capture.

## Feasibility probe (PASSED 2026-11-19)

- `rustup target add x86_64-pc-windows-msvc` on pn53.
- `cargo check -p snagcraft-capture --target x86_64-pc-windows-msvc` → OK (xcap +
  windows crate 0.62 compile; only dead-code warnings for the Linux-only stitcher).
- `cargo check -p snagcraft-platform --target …` → OK (tray-icon is cross-platform;
  ashpd/hotkeys are cfg(linux)).
- `cargo check -p snagcraft --target …` → **the whole egui app compiles for Windows.**
  The ONLY blocker was Unix-only file permissions in `automation::write_token_file`
  (`std::os::unix::fs`, `Permissions::from_mode`) — fixed in W25a (cfg(unix) keeps
  0600; cfg(not(unix)) writes without mode bits, token lives in user-scoped app data).

So the app shell, capture, platform and automation already cross-compile. What is
MISSING for issue #1 is monitor choice, not a from-scratch port.

## Scope (sub-waves)

### W25a — portability groundwork (DONE this wake)
- `automation::write_token_file` split cfg(unix)/cfg(not(unix)). Linux 0600 behavior
  unchanged (tests green); Windows writes without Unix mode bits.
- Whole app `cargo check`s for x86_64-pc-windows-msvc.

### W25b — capture backend: monitor enumeration + per-monitor capture (DONE 2026-11-19)
- `crates/capture` (non-Linux): `list_monitors() -> Vec<MonitorInfo>` via
  `xcap::Monitor::all()` (id/index, friendly_name, x/y/width/height, is_primary,
  scale_factor). `capture_monitor(index)` and `capture_window(...)` via xcap.
  Linux keeps the portal path; add a parallel `list_monitors` if feasible else the
  picker is non-Linux-only (Linux already has the OS picker).
- `MonitorInfo` is a plain serde struct (no xcap type leaks past the crate).

### W25c — engine commands (DONE 2026-11-19)
- `capture.monitors {}` → `{monitors:[MonitorInfo]}` (native).
- `capture.monitor {index}` → capture that output (native; Linux uses the portal
  MONITOR picker OR direct index if the portal can target one).
- `capture.window` non-Linux path (xcap Window::all + pick). Registry grows →
  bump BOTH count guards (automation + engine) per the skill.

### W25d — UI: in-app monitor picker (DONE 2026-11-19)
- Where no OS picker exists (Windows/macOS), Capture → "Choose screen" opens an
  egui dialog listing monitors (friendly name + resolution + primary badge);
  clicking one captures it. On Linux the existing portal picker stays.
- kittest gates: picker lists seeded MonitorInfo, click → capture.monitor dispatch,
  Esc closes. Snapshot + vision.

### W25e — verification + CI (DONE 2026-11-19)
- GitHub Actions `windows-latest` job added (rust.yml `windows`): `cargo build
  --workspace` (whole app compiles for Windows) + `cargo test --workspace
  --exclude snagcraft-ui-egui --no-fail-fast` (headless logic crates; the
  GPU/display-dependent kittest UI suite stays Linux-gated). **GREEN** on run
  44487918920 (commit bed5487): build success + all logic tests pass on a real
  Windows runner.
- Cross-platform test gating to get there: `cargo check --workspace --tests
  --target x86_64-pc-windows-msvc` (catches test-COMPILE issues plain check
  misses). cfg(unix): 0600 token-mode asserts, Unix-socket single_instance tests,
  file_uri percent-decode tests, XDG autostart test. Self-skip when tool absent:
  io OCR x4 + recording ffmpeg (no tesseract/ffmpeg on the Windows runner).
  Runtime fixes: timelapse "impossible folder" uses a dir under a regular temp
  file (not /proc); recording a_sane_config uses the real temp dir (validate
  checks parent-is-dir). engine ocr.available asserts a bool, not a host value.
- CI hardening: timeout-minutes on gates/appimage/windows jobs + step-level apt
  timeouts + Acquire::Retries (an apt-get install hung 30min+ on a runner).
- Linux suite stays green throughout (542/0/11, no portal-path regression).
- Live interactive Windows multi-monitor pass = rostad (or a CI artifact he runs)
  — pn53 cannot run Windows. Compile + logic tests are the offline evidence.

## Verification limits (honest)

- pn53 is Linux; it can COMPILE for Windows (check/clippy) and run the headless
  monitor-list/picker logic tests, but cannot RUN Windows capture. The real
  multi-monitor capture proof needs a Windows host (rostad or CI).
- Do not claim "Windows capture works" until a Windows run confirms it. Compile +
  logic tests are the offline evidence; label them as such.

## Out of scope (still gated / later)

- Windows recording (PipeWire is Linux; xcap recording is WIP upstream) — separate.
- Windows global hotkeys / autostart (platform crate cfg(linux) today) — graceful
  absence is fine for now; the in-app PrintScreen fallback covers capture.
- Release cutover / relicense — rostad gates, unchanged.