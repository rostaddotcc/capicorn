# SnagCraft Rust rebuild — consolidated gap register

Status: W6b (2026-11-17). Scope: the native Rust rebuild in `rust/` against the
Tauri v1.5.x baseline. This file consolidates every gap logged in
`docs/plans/rust-rebuild-status.md` so nothing is only discoverable in a wake log.

Each entry states **what** is missing, **why** it is missing, and **where** it was
logged. Nothing here is a promise of a date. Gates that require rostad's decision
are marked as gates and are not worked around.

Evidence baseline: 392 workspace tests green, `cargo xtask layers` legal (23
crates), AppImage launch-tested on pn53, TCP control + MCP live round-trip
verified, WASM web app browser-verified (status log, W5 wake #50).

---

## (a) Awaiting rostad

### A1 — W3/W4 KDE Plasma Wayland live pass (gate)

- **What:** the interactive portal legs of W3 and W4 have not been exercised on
  rostad's KDE box: tray click-to-restore, `Print` / `Shift+Print` global-shortcut
  consent, widget drag/transparency, hide-to-tray, settings persistence across
  restart; and for W4: `Rec` → portal consent → record → stop → file, scrolling
  capture, OCR window, timelapse.
- **Why:** pn53 runs a **locked GNOME** session. A locked session delivers no
  frame callbacks and cannot show portal consent dialogs, so those paths cannot be
  driven from here. What *is* proven offline: the PipeWire/ffmpeg recording probe
  (ffprobe-verified h264 MP4 + GIF), platform unit tests, and headless kittest
  snapshots of every shell surface.
- **Where logged:** status wake #17 (W3 sign-off), wake #32 (W4 delivery),
  wake #50 (W5 close, still pending).
- **Not a blocker for:** headless use (CLI, MCP, control channel) and the web app.

### A2 — Recording trim: spec lists it, live app does not have it (gate: rostad decides)

- **What:** trimming a recording (cut start/end) before save.
- **Why:** the rebuild targets parity with the shipped Tauri v1.5.x app, which has
  no trim feature. The rebuild spec lists trim as a W4 item, so the two documents
  disagree; W4d deliberately left trim **out** rather than invent a UI rostad has
  not asked for. Decision needed: build it (new feature beyond parity) or drop it
  from the spec.
- **Where logged:** W4 plan (`docs/plans/2026-11-16-w4-recording-ocr.md`), status
  wake #23 ("Trim deliberately OUT … ask rostad"), wake #32.

### A3 — Relicense GPL-3.0-or-later → MIT/Apache-2.0 (gate)

- **What:** the workspace is `GPL-3.0-or-later` with a commercial dual-licence
  option. A permissive relicense (MIT or Apache-2.0) has been discussed but not
  decided or performed.
- **Why:** licence choice is rostad's; it also affects the ArtCraft suite
  candidacy (see `docs/suite-candidacy.md`, which treats the current dual licence
  as a differentiator rather than assuming a relicense).
- **Where logged:** spec `docs/specs/2026-11-16-snagcraft-rust-rebuild.md`
  (open gates), status wake #1.

### A4 — Cutover from Tauri v1.5.x to the Rust build (gate)

- **What:** making the Rust rebuild the shipped SnagCraft instead of the Tauri
  app (repo root `src-tauri/`, `src/`, `package.json`).
- **Why:** cutover changes what users download and what the repo builds by
  default; it is rostad's call. W6 ships artifacts and docs only — no release
  publish, no default-branch switch.
- **Where logged:** spec (open gates), W6 plan
  (`docs/plans/2026-11-17-w6-packaging-docs.md`), status wake #1.

---

## (b) W4 functional gaps

### B1 — Audio: microphone + monitor mixing

> **W13 update (2026-11-17): CLOSED.** `AudioConfig` now carries `source` (mic)
> plus `system_audio`; the encoder opens one `-f pulse` input per source
> (`@DEFAULT_MONITOR@` for system audio, so an output switch mid-take keeps
> working) and mixes two inputs with `amix=inputs=2:duration=longest` into a
> single AAC track — the live `audio_output_args` recipe ported verbatim
> (aac 160k, `-shortest`, mic input before monitor). `record.start` accepts
> `system_audio`, and both audio params fall back to the persisted settings
> (W11 format precedent), so the W11 mic dropdown + system-audio toggle are
> fully wired. Verified live on pn53 (pipewire-pulse): the encoder child mixed
> two pulse sources into exactly one ffprobe-confirmed aac stream beside h264.
> GIF stays silent by construction; zero sources = `-an`, never an error.

- **What:** recording captures **one** audio source at a time (microphone *or*
  system monitor), not a mix of both. `audio.sources` enumerates what is
  available; there is no mixing stage.
- **Why:** the W4a recording backend feeds a single source into ffmpeg. Mixing
  needs either a second PipeWire capture stream joined in ffmpeg (`amix`) or a
  filter graph, plus UI for per-source gain — not built.
- **Where logged:** status wake #32 (W4 remaining gaps).

### B2 — Recent-media rail (missing entirely, video thumbnails included)

- **What:** the rebuild has no recent-captures strip. The live app has a newest-first
  rail (cap 12) that scans the save/recording folders, shows image thumbnails, and
  opens recordings in the OS player; it also renders a recording thumbnail
  (`recording_thumb`). None of that exists here.
- **Why:** it was never built in W1–W4; the logged W4 gap named the video-thumbnail
  part, but the underlying rail is absent too. Building it needs a folder scan, a
  thumbnail cache and an MP4/GIF frame extraction for recordings.
- **Where logged:** status wake #32 (W4 remaining gaps, "Recent rail video
  thumbnails"); parity checklist §3 "Recent strip" and §6 "Recording thumbnails for
  the Recent rail" — both still `pending`.

### B3 — Copy-diagnostics button

- **What:** there is no one-click "copy diagnostics" action (versions, backend
  availability, portal/tray/OCR status) for bug reports.
- **Why:** diagnostics are visible in logs/settings but not exposed as a
  copyable blob; the live Tauri app has such a button, the rebuild does not.
- **Where logged:** status wake #32.

### W12 shortcuts + About + editor residuals (2026-11-17) — CLOSED

Shortcuts overlay (? key + File→Keyboard shortcuts, real bindings sheet with
always-visible scrollbar; newly wired Ctrl+O + Ctrl+scroll zoom 0.25–4.0), About
modal (File→About SnagCraft, logo+name+version 0.1.0+tagline+repo, Esc/OK),
derived dirty-state (engine saved_depth + dirty()/history_depth(); save point
moves on save/export, undo-to-save-point is clean), ffmpeg-absent actionable
message. Editor residuals verified (caption settles on blur/Enter; crop shading
painter-only). Only TRIM remains pending — rostad-gated (not in live v1.5.3).

### W11 settings cluster + diagnostics + PrintScreen (2026-11-17) — CLOSED

Interval period, mic source (audio.sources dropdown), system audio toggle,
enable-global-hotkeys + Register button, recording format MP4/GIF, separate
recording folder + Change/Reset, filename pattern live-preview (build_filename),
engine diagnostics.copy + Troubleshooting button, PrintScreen/Shift+PrintScreen
in-app fallback. Settings window now vscroll (panel outgrew short screens);
lower rows reached via scroll, kittest uses click_accesskit for off-screen radios.

### W10 canvas presets + Resize (2026-11-17) — CLOSED

New-canvas dialog (Ctrl+N + File→New, 8 size presets Default 1280×800 …
Small 800×600 + custom W/H + Create, dirty-guard) and Resize dialog (engine
doc.resize scales base + annotation geometry, clamp 1..=16384; toolbar Resize…,
Esc closes). kittest.toml added: max_failed_pixels=64 absorbs a ~17px blinking-
caret render variance (real regressions still fail).

### W9 save & recent UX (2026-11-17) — CLOSED

Filename token expansion ({date}/{time}/{datetime}), Save As (Ctrl+Shift+S,
doc.save_as), default save dir (HOME/Pictures/SnagCraft + Change/Reset), and
the Recent strip (newest-first cap 12, in-session + folder scan, click-to-open,
toggle chip + count + Hide). Recording thumbnails use a film-icon placeholder
(real video first-frame thumb deferred). Still pending: separate recording
folder, New-canvas presets, Resize dialog, settings filename live-preview.

### B4 — The nine reverted parity rows

> **W15 update (2026-11-18): row 3's UI residual CLOSED.** The old "no Make
> video button" framing was wrong — the live app auto-assembles the MP4 when a
> timelapse run stops (`app.ts:3019-3047`), it has no button. The rebuild's
> `poll_timelapse` only reported the frame count; the stop→video conversion was
> never wired (the W4 tick on the combined parity row overclaimed). W15 wires
> it: `finish_timelapse` → off-thread `assemble_timelapse` (no session lock, so
> the editor keeps painting), live status wording ("Building…" / "Timelapse
> saved: … (N shots, 10 fps)" / "Timelapse failed: … shots are still in …"),
> zero-shot "nothing was saved" report, failed-run shots-kept note, and the
> finished video pushed to the Recent rail. kittest gates: real ffprobe-verified
> h264 encode + recent pickup, empty-run wording, NoFrames failure + re-assemble.
>
> **W8 update (2026-11-17): rows 4–9 CLOSED** — Ctrl+. stops a running
> timelapse (live parity, app.ts:5181), compact countdown HUD, and the upload
> POST flow (io `upload_image` via curl + engine `upload.send` + File→Upload,
> mock-server tested). **All nine B4 rows now closed.**
>
> **W7 update (2026-11-17): rows 1–3 CLOSED** — OCR language picker
> (persisted `ocr_lang`), OCR busy modal (off-thread, cleared on result/error),
> and `timelapse.assemble` (ffmpeg glob → h264 mp4, ffprobe-verified). Rows 4–9
> remain pending (Ctrl+., countdown HUD, upload POST). See status log wake #65.

At W4 close, nine checklist rows were **bulk-ticked and then reverted** after
audit because the code did not back them (see
`docs/plans/rust-rebuild-parity.md`). They remain pending:

| # | Row | What exists now | What is missing |
|---|-----|-----------------|-----------------|
| 1 | OCR language select (UI) | **CLOSED W7**: ComboBox from `ocr.languages`, persisted `ocr_lang` | — |
| 2 | OCR busy modal | **CLOSED W7**: off-thread worker, spinner + disabled Extract, cleared on result/error/dead worker | — |
| 3 | Timelapse video assembly (`make_timelapse`) | **CLOSED W7**: `timelapse.assemble` ffmpeg glob → h264 mp4 (ffprobe: 34×22 yuv420p, 5 frames). **W15**: the live app has no "Make video" button — it auto-assembles on stop (`app.ts:3019-3047`); the rebuild now does the same (off-thread encode, 10 fps, live status wording, recent-rail pickup) | — |
| 4 | `Ctrl+.` stop hotkey | stop via HUD button / `record.stop` | the live app's `Ctrl+.` global-ish stop shortcut |
| 5 | Shrunk countdown HUD | countdown HUD before recording starts | the shrunk/compact countdown variant |
| 6–9 | Upload POST-to-endpoint flow | `upload.open` opens an `http(s)` URL only (scheme-guarded) | POSTing the file bytes to an endpoint via `curl` as the live app does |

Rows 6–9 are one feature counted as four checklist rows (endpoint config,
request construction, response handling, error surfacing).

- **Why:** W4 was scoped to parity of behaviour that could be verified offline;
  these need either a live session (1, 2, 4, 5), a new backend path (3), or a
  network side effect that was out of W4 scope (6–9).
- **Where logged:** status wake #32 ("9 bulk-ticked rows reverted to pending
  after audit (overclaim catch)"), parity checklist §recording/OCR/upload.

### B5 — W2 cosmetic residuals (logged for polish, not re-audited in W6)

> **W14 update (2026-11-17): ALL CLOSED.**
> - Item 1 (crop frame re-grab/move): closed at the W13 audit — NOT A GAP (the
>   live crop overlay is painter-only; the "handles" rail hint is prose).
> - Item 2 (inline callout/text edit): **CLOSED W14.** Double-click a Text or
>   Callout in Select mode → inline re-edit (`edit.set_text`); emptying the box
>   removes the object (live settle parity). Rect-based selections also got
>   corner resize handles (`edit.resize`) — the live fabric `applyInteractivity`
>   equivalents. kittest pointer gates: resize+undo, text re-edit/append/empty-
>   remove, callout re-edit.
> - Item 3 (nib preview): **CLOSED W14.** Brush rail shows the 48px reference
>   box with an ink-coloured dot at the nib's 1:1 diameter + value label
>   (snapshot w14_brush_nib, vision-verified).
> - Crop plain-click nuance (W13 audit): **CLOSED W14.** <8px drag/click resets
>   the pending frame; Enter commits only ≥8px (MIN_DRAG/MIN_SIZE=8 parity).

- **What:** (1) dragging inside an existing crop frame starts a *new* crop
  instead of re-grabbing/moving the frame; (2) existing callouts cannot be edited
  inline after creation; (3) no brush nib cursor preview.
- **Why:** logged at W2 as polish, deferred to W3/polish; W3–W5 did not pick them
  up. Listed here so they are not lost.
- **Where logged:** status wake #7+ ("W2 gaps logged for W3/polish").
- **Caveat:** status as of W6 is "as logged at W2"; no W6 re-verification was done.

---

## (c) W5 web MVP limits

The WASM web build (`rust/apps/snagcraft-web`, trunk) is a working editor —
browser-verified boot, full editor render, PNG export-download — but it is an MVP,
not feature parity with the native app.

### C1 — Web command surface is 21 of 41 commands

- **What:** the web engine build (`--no-default-features --features web`)
  registers 21 commands. Native-only: `capture.full`, `clipboard.copy/paste`,
  `ocr.*` (3), `record.*` (6), `scroll.*` (3), `timelapse.*` (3), `upload.open`,
  `audio.sources`. `Capture`, OCR, widget and tray are hidden in the web UI.
- **Why:** those commands need Linux platform backends (portal, PipeWire,
  tesseract subprocess, arboard). The web build opens/saves documents and does the
  full annotation toolset.
- **Where logged:** status wake #39 (21/41 in wasm rlib), wake #50.

### C2 — Image copy/paste on web is status-only

- **What:** the web UI reports clipboard status but does not write the image to
  the browser clipboard; there is no `ClipboardItem` write, so Copy does not put a
  PNG on the system clipboard in the browser.
- **Why:** `web-sys` clipboard image write needs `ClipboardItem` +
  `write([...])` with a permission prompt, and Firefox lags on image writes. The
  native path (`arboard`) is unaffected.
- **Where logged:** status wake #50 ("web image copy/paste = status only (no
  ClipboardItem write)").

### C3 — Web drag-and-drop open is code-complete but not browser-exercised

- **What:** dropping an image file on the web canvas opens it in the code path,
  but this has not been driven in a real browser session; only boot → render →
  export was browser-verified.
- **Why:** the browser verification covered the critical path; drag-drop needs a
  synthetic DataTransfer in the browser harness and was not run.
- **Where logged:** status wake #50 ("drag-drop open code-complete but not
  browser-exercised").

---

## (d) Platform — FUTURE (not buildable in W6)

### D1 — Windows NSIS/MSI and macOS dmg installers

- **What:** the spec's packaging matrix lists Windows (NSIS/MSI) and macOS (dmg)
  installers. W6a ships **Linux only**: AppImage (`rust/packaging/appimage.sh`)
  and a `.deb` (`rust/packaging/deb.sh`).
- **Why:** the platform layer is Linux-only by construction —
  `ashpd` (Screenshot/ScreenCast/GlobalShortcuts portals), `ksni` via
  `tray-icon` (StatusNotifierItem), PipeWire for recording, `arboard` with
  `wayland-data-control`, and XDG paths in `io::settings`. Capture, tray, global
  hotkeys and recording would each need a platform backend per OS
  (Windows: `Windows.Graphics.Capture` + Win32 tray/hotkeys; macOS:
  ScreenCaptureKit + `NSStatusItem`/Carbon hotkeys), plus CI runners to build
  them. That is a porting project, not a packaging task.
- **Where logged:** W6 plan "Scope reality (measured)", status wake #32/W6;
  this file.
- **Cross-platform reach today:** the WASM web build runs in any modern browser
  on any OS — that is the current cross-platform story, with the limits in (c).

### D2 — No macOS/Windows code signing or notarisation

- **What:** no signing identities, notarisation or installer metadata for
  non-Linux targets.
- **Why:** follows from D1 (nothing to sign yet).
- **Where logged:** this file; W6 plan.

---

## Gates — never autonomous

| Gate | Owner | Status |
|------|-------|--------|
| Relicense GPL-3.0-or-later → MIT/Apache-2.0 | rostad | open (A3) |
| Cutover from Tauri v1.5.x | rostad | open (A4) |
| Any external posting (issues/PRs/comments to storytold repos, Discord, release publish) | rostad | forbidden in W6; the suite dossier is internal only |
| KDE live acceptance pass | rostad | open (A1) |
| Trim: build or drop | rostad | open (A2) |