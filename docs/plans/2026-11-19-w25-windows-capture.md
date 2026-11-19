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

### W25e — verification + CI
- `cargo check`/`clippy` for the Windows target stays clean (add to xtask or CI).
- GitHub Actions `windows-latest`: build + `cargo test` (headless logic) + a capture
  smoke if a virtual display is available. Live interactive Windows pass = rostad
  (or a CI artifact he runs) — pn53 cannot run Windows.
- Linux suite stays green throughout (no regression to the portal path).

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