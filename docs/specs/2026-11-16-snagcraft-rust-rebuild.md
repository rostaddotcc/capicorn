# SnagCraft — Full Rust Rebuild Spec (ArtCraft architecture)

> **For Hermes:** this is a goal/loop mission spec. Work wave by wave (W0 → W6),
> re-checking state fresh on every wake. Use subagent-driven-development for parallel
> tracks with locked contracts; verify children's output against artifacts before
> claiming done. Follow writing-plans: write the detailed task plan for each wave
> before dispatching it, and append measured reality to the appendix, never rewrite prose.

**Goal:** rebuild SnagCraft as a Rust-only native app (egui/eframe on wgpu) following the
ArtCraft suite's code architecture and philosophy, reaching feature parity with the Tauri
v1.5.x line — which keeps shipping untouched until an explicit cutover decision.

**Architecture:** the PhotoCraft model — enforced L0–L6 crate layering, sans-I/O engines,
an L5 engine whose command registry drives UI, CLI, JSON control channel and MCP alike,
and a thin egui shell. The same Rust compiles to a WASM web build.

**Tech stack:** Rust stable, egui/eframe, wgpu, ashpd (freedesktop portals), xcap, arboard,
image, webp, ffmpeg (subprocess), tray-icon with ksni backend, chrono, tracing;
trunk + wasm-bindgen (web); cargo xtask (layers/ci/version). No Tauri, no JS/TS, no webview.

**Context / correction:** rostad's original brief was to follow ArtCraft's architecture,
not just its spirit. The Tauri stack was an assistant deviation, not his choice
(correction 2026-11-16). This rebuild is the directed path. Do not defend the old stack.

---

## 1. Ground truth — read first, every session

- Repo: `~/projects/snagcraft` (github.com/rostaddotcc/snagcraft). Tauri app at root;
  the rebuild lives in a NEW cargo workspace `rust/` — never break the root app.
- Architecture model (authoritative): `storytold/photocraft` — AGENTS.md,
  docs/architecture.md, docs/control-protocol.md, docs/ui-design.md. Fetch raw copies
  into `tmp/research/` (gitignored) and cite them; re-fetch if stale.
- Suite rules: `storytold/craft-launcher` AGENTS.md + `storytold/craft-contrib`
  standards/house-rules-core.md and house-rules-rust.md.
- Feature baseline: root `CHANGELOG.md`, `docs/specs/2026-11-14-snagcraft-design.md`,
  and the live source `src/editor/tools.ts` + `src/ui/app.ts` (source of truth for parity).
- Progress log: `docs/plans/rust-rebuild-status.md` — append one entry per wake.
- Skills: tauri-desktop-app (Wayland recipes still apply), subagent-driven-development,
  delegation-tuning, writing-plans, verification-before-completion.

## 2. Target architecture (layering enforced by `cargo xtask layers`)

```
rust/crates/
  geom color raster        L0  geometry, pixel formats, blend math
  doc                      L1  document model: base image + annotation objects (pure data)
  ops algo text vector     L2  history (undo/redo), blur/imaging algorithms, type engine, paths
  compose format           L3  CPU compositor (oracle) + wgpu compositor; .snagcraft native format
  io capture               L4  PNG/JPEG/WebP, portal capture, scrolling capture, recording, OCR
  engine                   L5  Session + command registry — every action is a command
  ui-egui automation       L6  thin egui shell; MCP server
rust/apps/
  snagcraft                desktop (eframe/wgpu) + TCP JSON control server
  snagcraft-cli            headless CLI (capture/edit/convert/run/commands/mcp)
  snagcraft-web            same app in the browser (trunk + wasm-bindgen)
rust/xtask/                layers | ci | wasm | version
```

Hard rules (from the suite, non-negotiable):
- Nothing below L6 may depend on egui/eframe/winit/rfd. New crates register in xtask layers.
- Sans-I/O engines: L2 takes data, returns data. No fs/network/OS calls below L4.
- Never crash: every production crate root carries
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`;
  no `unsafe`; treat all input as hostile; bound recursion and allocations.
- Licence allowlist for EVERY dependency and asset (SPDX): ISC, MIT, BSD-2-Clause,
  BSD-3-Clause, Apache-2.0, OFL-1.1, CC0-1.0, CC-BY-4.0, CC-BY-SA-4.0, public domain.
  **GPL is NOT on the allowlist.** Check before adding any crate.
- Clean-room: learn from Snagit's observed behaviour and public specs; never copy
  TechSmith code, assets, strings or artwork. Every asset recorded in ATTRIBUTION.md.
- Time via chrono. Logging via log/tracing — never println!/eprintln! outside tests.
- Format crate fails to compile when a doc struct gains a field (no silent save loss).

## 3. Reuse inventory — port, don't rewrite (from src-tauri/src/, v1.5.2, ~9.2k lines)

| Source | Lines | Destination |
|---|---|---|
| portal_capture.rs + capture.rs | 1090 | L4 capture (ashpd portal Linux, xcap Win/macOS) |
| recording.rs (ffmpeg subprocess) | 2665 | L4 recording |
| scrolling/{mod,cap,source,align,stitcher}.rs | ~2247 | L4 scrolling capture |
| ocr.rs / export.rs / clipboard.rs / upload.rs / timelapse.rs | 1276 | L4 |
| hotkeys.rs (ashpd GlobalShortcuts portal) | 501 | L6 platform — NOT the X11-only plugin |
| tray.rs (ksni, default-features=false — load-bearing, avoids libxdo) | 114 | L6 platform |
| cli.rs | 142 | apps/snagcraft-cli seed |

Strip all `tauri::` dependencies. commands.rs / windows.rs / lib.rs are Tauri glue —
reimplement as engine + eframe shell; do not port them.

## 4. Rebuild scope — the ~12.2k lines of TS (parity checklist source: live v1.5.x)

Editor = annotation scene graph over a base image, rendered with egui Painter + wgpu:
- The 13 classic toolbar tools (enumerate exactly from src/editor/tools.ts at W1):
  select/move, crop, arrow, text, rect, line, blur, step, bucket, magnify, spotlight,
  highlighter, callout. Text: fonts, size, color, bold/italic, movable box, quick
  font/size bar while editing, no auto-"Text" placeholder.
- Blur modes: real gaussian (multi-pass), pixelate, blackout; secure minimum radius for
  redaction; blur REPLACES region pixels (no ghost/shadow compositing).
- Undo/redo (L2 history), copy-to-clipboard, save PNG/JPEG/WebP + quality, Save As,
  default format + save-dir settings, open image, drag-drop, Ctrl+V, watermark export.
- Classic desktop UI — NOT ai-web-vibe: compact one-row icon toolbar at all widths,
  classic blue accent #2266b3 (light) / #3d7ec2 (dark), radii 4–8px, light + dark themes,
  settings panel, shortcuts overlay, About box.
- Shell: floating collapsible edge-tab widget (egui viewport; ~292px pill, red Rec button
  with live Stop state, V shortcut), tray click-to-restore, global hotkeys via portal,
  close-guard modal, single-instance, autostart, file associations, --capture/--region CLI
  with launch_request polling.
- Recording UI (region/screen → MP4/GIF, trim, pause/resume, audio monitor/window sources),
  scrolling-capture UI, OCR result UI, timelapse/interval mode + HUD.
- Requested next (spec separately when reached): local-AI auto-redaction of secrets.

## 5. Waves

- **W0 — PROBE GATE (do first, nothing else starts until it passes):**
  (a) egui editor spike: load PNG, arrow/text/rect, undo/redo, one multi-pass blur,
  classic toolbar look. (b) transparent frameless egui viewport on KDE Wayland
  (transparency, drag, stacking). Build + clippy on pn53; deliver a runnable spike
  AppImage/binary for rostad's KDE box. If a probe fails, record the fallback in the
  appendix and re-plan before W1. Probe code stays in `rust/probes/` (cite paths).
- **W1:** workspace scaffold (crates L0–L6 + apps + xtask layers), doc model, engine
  command registry, port capture backend. Desktop app: capture → editor shows image.
  AppImage builds in CI. Write the exact parity checklist from live v1.5.x source.
- **W2:** editor tool parity (all 13 tools), text engine, blur modes, history, crop,
  export formats/quality.
- **W3:** shell parity — tray, hotkeys, widget, settings, themes, close-guard, autostart,
  single-instance, file associations, CLI.
- **W4:** recording + scrolling capture + OCR + timelapse + upload/watermark.
- **W5:** automation — TCP JSON control channel + MCP server + headless CLI (every
  command drivable); WASM web build.
- **W6:** packaging polish (AppImage/deb, Windows NSIS/MSI, macOS dmg via GitHub Actions),
  docs (README rewrite, gaps.md, parity checklist vs v1.5.2), suite-candidacy dossier
  (ScreenCraft niche #2148/#2337 — differentiation notes; NO external posting without gate).

Each wave: write its task plan → dispatch parallel tracks on disjoint crates with locked
contracts → integrate → build + test + real run → tick parity items with evidence → commit.

## 6. Acceptance — definition of done

1. Feature parity with Tauri v1.5.x, every checklist item ticked with evidence in
   docs/plans/rust-rebuild-status.md.
2. `cargo xtask layers`, fmt, clippy (deny lints), full test suite green.
3. AppImage runs on rostad's KDE Plasma Wayland ultrawide: capture → annotate →
   copy/save in under ten seconds.
4. Every feature drivable via CLI and the JSON control channel; MCP exposes the commands.
5. Zero Tauri/JS/TS/webview dependencies anywhere in `rust/`.
6. All deps/assets on the licence allowlist; ATTRIBUTION.md complete.

## 7. Gates — STOP and ask rostad (never autonomous)

- Relicensing SnagCraft (GPL-3.0 + commercial → MIT OR Apache-2.0) — his business decision.
- ANY external post: issues/PRs/comments to storytold repos, Discord, releases, publishing.
- Cutover: deprecating/removing the Tauri app or changing the default release channel.
- Any destructive git/file operation.

## 8. Loop/goal behavior — every wake

1. Re-check state fresh: git log/status, docs/plans/rust-rebuild-status.md, latest
   artifacts. Never trust memory of previous wakes.
2. Pick the next unfinished wave item; work it to verified completion. Report only what
   ran — no fabricated results; say what you could not run.
3. Parallel subagents for disjoint tracks; no time pressure, let children finish; verify
   their claims against artifacts before repeating them.
4. Append a concise entry to the status doc; report count + highlights, never item dumps.
5. Deliver a runnable AppImage/spike the moment it is testable — do not hold for internal QC.
6. Blocked? One line saying what blocked it; move to another track or record the blocker.
7. Keep the root Tauri v1.5.x line green if anything touches it.

## Appendix — measured reality

(append one row per contradicted assumption: claim → measured → what changed)
