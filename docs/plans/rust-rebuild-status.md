# SnagCraft Rust rebuild — status log

Spec: `docs/specs/2026-11-16-snagcraft-rust-rebuild.md`
Baseline: Tauri v1.5.2 (commit d25adb1). Rebuild workspace: `rust/` (not yet created).

## Wave status

| Wave | Scope | Status |
|------|-------|--------|
| W0 | egui editor spike + Wayland viewport probe | KDE-approved by rostad 2026-11-16 ("appen lirar") |
| W1 | workspace scaffold, doc model, engine, capture port | KDE-approved by rostad 2026-11-16 |
| W2 | editor tool parity | KDE-approved by rostad 2026-11-16; UI polish pass landed (sliders) |
| W3 | shell parity (tray/hotkeys/widget/settings) | complete on pn53 (216 tests); KDE live pass pending rostad |
| W4 | recording, scrolling, OCR, timelapse | complete on pn53 (363 tests); KDE live pass pending; gaps logged below |
| W5 | CLI/JSON/MCP automation + WASM web | complete (3273af1 + watermark fix); web app browser-verified; W6 next |
| W6 | packaging, docs, suite dossier | complete (bbd2683 + docs); all waves W0–W6 BUILT |
| W7 | parity backlog (gaps B4#1–3) | complete — timelapse.assemble + OCR lang picker + busy modal (390 tests, ffprobe-verified) |
| W8 | parity backlog (gaps B4#4–9) | complete — upload POST flow + Ctrl+. (timelapse) + countdown HUD; all 9 B4 rows closed |
| W9 | parity long-tail (save & recent UX) | complete — filename tokens + Save As + default dir + Recent strip (451 tests) |
| W10 | parity long-tail (canvas presets + Resize) | complete — New-canvas presets + Resize dialog (459 tests, kittest tolerance) |
| W11 | parity long-tail (settings + diagnostics + PrintScreen) | complete — settings cluster + diagnostics.copy + PrintScreen fallback (468 tests) |
| W12 | parity long-tail (shortcuts + About + editor residuals) | complete — shortcuts overlay + About + derived dirty-state + ffmpeg msg (474 tests) |

**PARITY REACHED 2026-11-17 (wake #100):** every checklist row ticked with
evidence except **Trim** — rostad-gated (not implemented in live v1.5.3 either;
his decision). W0–W12 built; 474 tests / 0 failed; native AppImage + .deb + CLI
+ TCP/MCP automation + WASM web app, one Rust engine. Remaining: rostad's KDE
live pass + gates (trim/relicense/cutover/external post). NO releases published.

## Parity checklist

(to be generated from live v1.5.x source at W1 — tick with evidence)

## Wake log

### 2026-11-16 — spec written
- Full-Rust rebuild spec approved for goal/loop use; no code started yet.
- Inventory: ~9.2k lines Rust backend portable; ~12.2k lines TS editor/UI to rebuild.
- Open gates: relicense (GPL → MIT/Apache-2.0), external posts, cutover — all need rostad.

### 2026-11-16 — wake #1: W0 probe gate built and verified on pn53
- Architecture ground truth fetched to `tmp/research/photocraft/` + `tmp/research/suite/`
  (AGENTS.md, architecture.md, control-protocol.md, ui-design.md, house rules). egui API
  references in `tmp/research/egui/`.
- W0 plan: `docs/plans/2026-11-16-w0-probe-gate.md`. New workspace `rust/` (resolver 3,
  edition 2024, deny-lints + unsafe forbid, clippy.toml test exemptions).
- `rust/probes/editor-spike`: pure scene core (doc/history/3-pass blur/composite, 12 unit
  tests green) + thin egui shell (one-row classic toolbar, accent #2266b3, arrow/text/rect/
  blur tools, select+move, undo/redo, open/save PNG, drag-drop, inline text editing).
- `rust/probes/viewport-probe`: 292×64 transparent frameless pill, red Rec button,
  StartDrag anywhere, session info on-screen, Esc quits.
- Gates run: `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -D warnings`
  clean; 12/12 unit tests; headless kittest visual snapshot green AND vision-verified
  (`rust/probes/editor-spike/tests/snapshots/editor_spike.png` — toolbar, canvas, arrow,
  rect, text, blur all render correctly).
- Live launch on pn53 Wayland (Radeon 660M, RADV Vulkan): both binaries AND both AppImages
  stay alive (timeout exit 124). Only log noise: benign sctk_adwaita color-scheme portal
  timeout; arboard falls back to X11 clipboard (GNOME lacks ext-data-control — W3 note).
- Artifacts: `dist/w0-spike/SnagCraft_W0_Editor_Spike-x86_64.AppImage` (9.5 MB),
  `SnagCraft_W0_Viewport_Probe-x86_64.AppImage` (9.1 MB) — vs ~80–90 MB Tauri AppImage.
- W0 built by parent directly (two small probes; API verified in-context). Parallel
  subagent tracks start at W1.
- Remaining for W0 sign-off: rostad's KDE pass — desktop visible through pill corners,
  drag feel, stacking; editor flow capture→annotate→save on his box. `--selftest
  --shot-out` works only in an unlocked session (see appendix).

### 2026-11-16 — wake #2: W1 launched
- Fresh check: W0 committed b39da91 + pushed; no KDE feedback yet; W1 next.
- W1 plan with locked contracts: docs/plans/2026-11-16-w1-scaffold-engine-capture.md.
- Parent landed: full workspace scaffold (15 crates L0–L6 + 3 apps + xtask, 21 members),
  xtask layers (layer map + cycle detection + UI ban below L6) + ci, .cargo alias,
  packaging/ (desktop + icon + appimage.sh), .github/workflows/rust.yml (gates + AppImage
  artifact). ashpd 0.14.0 + xcap 0.9.10 + tokio/clap/cargo_metadata verified compiling.
- Parent landed L0 geom/color/raster + L1 doc (8 object kinds, serde, hit-test/shift) +
  L2 ops (bounded history): 20 unit tests green, clippy -D warnings clean, layers legal.
- Dispatched 4 parallel tracks (deleg_6ba30a31): C1 algo+compose, C2 format+io,
  D capture (ashpd Screenshot port; ScreenCast/PipeWire deferred to W4), P parity
  checklist from live v1.5.3 source → docs/plans/rust-rebuild-parity.md.
- Next: integrate children → engine (Session + command registry) → ui-egui shell
  (capture→editor) → apps/snagcraft + cli → gates → AppImage → deliver.

### 2026-11-16 — wake #3: W1 integrated end-to-end
- Children C1/C2/D/P all finished; parent verified artifacts: 39 crate tests green
  before integration, clippy clean, format compile-guard intact (no `..` rest).
- Parent landed: engine Session + 16-command registry (7 tests incl. adversarial
  params), ui-egui engine-driven shell (Capture button → threaded portal capture →
  editor; Select/Arrow/Rect/Text/Blur; Save native / Export PNG; undo/redo; drag-drop),
  apps/snagcraft (eframe, --capture/--selftest) + snagcraft-cli (capture/convert/
  commands/run — every registry command drivable headless).
- Two real bugs found + fixed by parent verification (not child self-reports):
  (1) compose text used ab_glyph raw font-unit metrics (ascent 932×scale → baseline
  off-canvas, silent text loss) → fixed with as_scaled()/ScaleFont pixel metrics;
  (2) CLI had no fonts because egui's own default features are EMPTY (default_fonts
  comes via eframe) → workspace egui now enables default_fonts explicitly.
  Regression probes: ui-egui render_probe tests + CLI text-export pixel check (417 px).
- Seeded demo scene now uses a detailed synthetic base (white-on-white blur was
  invisible in the first snapshot).
- Final gates: fmt ✓ clippy -D warnings ✓ 99 tests / 0 failed / 1 ignored (live
  portal, locked host) ✓ xtask layers 21 crates ✓. Headless w1_shell snapshot
  vision-verified: Capture button, 5 tools, arrow+rect+text+blur all render.
- Desktop release binary + AppImage (10.3 MB) launch-tested on pn53 Wayland (exit 124).
  CLI smoke: commands lists 16; convert PNG→JPEG→PNG round-trip; run edit.add +
  export with text verified pixel-wise.
- ATTRIBUTION.md added (icon original; no committed fonts; dep allowlist check).
- Parity checklist: docs/plans/rust-rebuild-parity.md — 149 evidence-backed rows,
  14 sections, baseline v1.5.3 @ 3cec2ee. W1 ticks: capture→editor flow, 8 object
  kinds, undo/redo, PNG/JPEG/WebP export, native format, CLI automation.
- Remaining W1 sign-off: rostad's KDE pass — live portal capture (unlocked),
  annotate → save/export under ten seconds. W2 next: full 13-tool parity.
- Post-batch follow-up (child reports arrived after integration; all matched the
  verified artifacts): confirmed C1's spotlight semantics (dim = darkening amount,
  factor 1−dim — matches contract intent); dropped capture's unused geom dep;
  hardened format::load to reject manifest-size/base-image mismatch (+tamper test,
  format now 11 tests). C2's WebPEncoder::new_lossless deviation accepted (image
  0.25 API). W2 inputs from parity doc: 5 shape variants (triangle/star missing in
  rust ShapeKind), live MAX_HISTORY=80, SECURE_BLUR_MIN=10, blackout #111827,
  spotlight rgba(8,12,18,.68), highlighter α .36, shortcuts V C A T R B E G L H K S M.

### 2026-11-16 — wake #4: W2 launched (tool parity)
- Fresh check: W1 stable at 942597f, no KDE feedback yet → next item W2.
- W2 plan: docs/plans/2026-11-16-w2-tool-parity.md (W2a crate tracks now, W2b
  engine+UI after). Parity inputs from checklist §1–2: 13 tools, 5 shape variants,
  blur radius 2–60 default 12 + SECURE_BLUR_MIN 10, blackout #111827, spotlight
  rgba(8,12,18,.68), highlighter multiply .36, shadow angle×distance→offsets,
  history 80, brush 2–44, magnify 1.5–10×, callout width→font-size.
- Font assets prepared parent-side: 14 OFL TTFs decompressed from the live app's
  woff2 (fontTools 4.61.1) → rust/assets/fonts/ + OFL texts; ATTRIBUTION updated.
  Italic = synthetic shear 0.2 (live bundles no italic faces either).
- Manifests pre-wired + compile-verified: doc +image (crop_imm), text +ab_glyph.
- Dispatched W2a (deleg_0688f850): T1 doc/ops/format (Shadow, Triangle/Star,
  Magnify/BrushStroke, crop, history 80, guard extension), T2 algo/compose
  (parity constants, flood_fill, resize_region, exact blends, shadows, styled
  composite + italic shear), T3 text FontDb (10 families, 14 faces).
- Next: verify T1–T3 artifacts → W2b engine commands + full 13-tool UI → gates →
  AppImage → tick parity rows.

### 2026-11-16 — wake #5: W2a verified, engine W2b landed, UI dispatched
- T1 (doc/ops/format) + T3 (text) finished; T2 (algo/compose) in final verification.
- Parent verified: doc 19 tests, format 14, ops 11, text 8 (live family order,
  14 faces, bold fallbacks, case-insensitive) — clippy -D warnings clean on all.
- Contract gap caught parent-side: Text/Callout lacked `family` (live TextStyle
  has 10 families). T1 finished before the steer landed → parent applied the
  family extension (serde default "Inter") to doc + format guard + 6 test literals.
  T2 consumed its steer: composite_styled now passes (obj.family, obj.bold) and
  applies italic shear per object.
- Engine W2b (parent): edit.crop + edit.bucket commands (18 total), blur default
  radius 12 clamp 2–60 (parity), render() switched to composite_styled with a
  Session-cached FontDb face table (resolve_shared/FontArc) — bundled OFL fonts
  now identical across UI/CLI/export; set_font removed (ui-egui + CLI updated,
  CLI egui dep dropped). 11 engine tests green incl. crop/bucket undo, hostile
  seeds, FontDb text pixel probe.
- Dispatched full 13-tool UI track (deleg_3ecf00d3): one-row toolbar + contextual
  properties rail + crop/brush/step/callout/magnify/bucket UX + quick font bar.
- Next: T2 + UI land → full gates → vision contact sheet → AppImage → tick parity.
- W2a batch closed: T2 final artifacts parent-verified (algo 22 + compose 34 tests,
  engine 11 re-run against final compose — all green, clippy clean, layers legal).
  Accepted deviations (cosmetic, logged for W2 polish/W6 gaps): spotlight hole +
  highlighter are hard rects (live rounds corners); shadow blur = one box pass
  (live canvas shadowBlur sigma). Arrow head uses the EXACT live formula
  (thickness=w*2, headLen=max(16,thickness*1.7), headHalf=max(9,thickness)).
  T3 contract conflict resolved well: ab_glyph FontVec is NOT Clone →
  resolve_shared/FontArc added as the cheap path (engine already uses it).
  T1 added sanitize() on add() + format::load (shadow clamps, brush point cap,
  non-finite point skip) — extra hostile-input hardening beyond contract.

### 2026-11-16 — wake #6: W2 polish while UI builds
- UI track (deleg_3ecf00d3) still running — lib.rs 714 → 1896 lines mid-build.
- Parent closed the two logged cosmetic parity gaps in compose (no sibling owns
  it now): spotlight hole radius min(12,min(w,h)/6) + highlighter band radius
  min(8,min(w,h)/4) with ≤0.5 hard-rect fallback (exact live blur.ts formulas,
  SDF coverage AA), shadow blur 3 box passes ≈ gaussian. 2 corner-coverage
  regression tests; 36 compose tests green, clippy clean. Committed c3a1a94.
- Parity checklist: 11 rows ticked W1/W2 (blur modes/constants/secure-min/
  blends, history 80 + reset-on-new).
- Next: UI lands → full gates + vision contact sheet + AppImage + W2 sign-off.

### 2026-11-16 — wake #7: UI pre-verification + step-number bug fix
- UI track still running (final verification); snapshots w2_shell + w2_shell_narrow
  blessed by child. Parent vision + pixel probes:
  * Shell structure PASS: exactly 13 icon tools on ONE row + Capture/Open/Save/
    Export/Undo/Redo; contextual rail (shape picker, 8-swatch palette, border,
    shadow angle/distance/opacity/blur); narrow width keeps one row with
    horizontal ScrollArea (tools 11–13 scroll-reachable, code-verified).
  * Vision flagged spotlight/blur/magnify as broken — pixel probes DISPROVED it
    (spotlight α=0.306 exact, blur smoothed, magnify shows zoomed content);
    vision misread the synthetic checker base as unflattened alpha.
  * REAL bug found: step-badge numbers invisible — compose asks fonts("", true)
    and the engine lookup was exact-match-only → None. Fixed: fallback chain
    exact → Inter → same-weight → first (mirrors FontDb::resolve). Regression
    test counts glyph pixels strictly inside the badge (first version
    false-passed on the white base — caught and fixed). 12 engine tests green.
- Steered UI child: re-bless both snapshots with the fixed engine; do NOT
  "fix" the proven-correct renderers. Committed f20cd2e + pushed.
- Next: UI lands → full gates + AppImage + W2 sign-off + delivery.

### 2026-11-16 — wake #7+: rostad's request — right-click + copy/paste (W2 finished)
- rostad (direct message): wants right-click classic context menu + Copy (Ctrl+C)
  so annotated captures paste into other chats. Live app has Ctrl+C but NO custom
  context menu (editor.ts:335 suppresses it) — this goes beyond live parity.
- Landed: io::clipboard (arboard 3 + wayland-data-control, ported from live
  clipboard.rs, Surface-based copy_image/paste_image, graceful headless errors);
  engine commands clipboard.copy / clipboard.paste / edit.duplicate / edit.reorder
  (22 total, 15 engine tests); UI: Ctrl+C/V/D hotkeys, toolbar Copy button, classic
  right-click menu (Copy Image, Paste | Duplicate, Delete, Bring to Front, Send to
  Back | Undo, Redo | Save As…, Export PNG…) with shortcut hints + disabled states.
- UI child (deleg_3ecf00d3) finished: full 13-tool shell, 16 ui-egui tests incl.
  parity.rs (shortcut routing 13/13, drag-create 7 region tools, crop Enter/Esc,
  step auto-numbering, brush, bucket, callout size table, shadow derivation).
- Context-menu kittest tests (right-click injection): menu opens, classic items
  present, object items on hit; snapshot vision-verified (10 items, hints,
  separators, disabled Redo — clean classic look).
- Full gates: fmt ✓ clippy -D warnings ✓ **180 tests / 0 failed / 1 ignored** ✓
  layers 21 ✓. Release + AppImage (11.75 MB, fonts embedded) launch-tested (124).
- Parity: 15 more rows ticked (13 tools + copy + paste) → 26 W2 rows total.
- Delivered W2 AppImage to rostad for the KDE pass (capture → annotate →
  right-click Copy → paste into chat is the acceptance flow).
- UI child completion report reconciled (all already integrated in 6b57dab):
  * APPROVED deviation: callout font size uses the LIVE formula
    min(rail, round(w/4)) clamp 12–120 (tools.ts:1387-1391), not the brief's
    clamp(w/10,12,48) — live parity wins; table-tested.
  * Child caught a real W1 bug en route: drag-start read hover_pos (already
    the moved point on egui's first drag frame) → every drag-create was 0px;
    fixed with pointer.press_origin(). W1 snapshots never caught it (seeded
    via commands, not drags) — parity.rs drag gates did.
  * Stale w1_shell.png removed (unreferenced after test rename); 18 ui-egui
    tests green.
  * W2 gaps logged for W3/polish: re-grab/move an existing crop frame (new
    drag replaces it), inline caption editing of existing callouts (created
    with default text), brush nib cursor preview.

### 2026-11-16 — wake #8: rostad KDE-approval + UI polish (reglage)
- rostad: "ser bra ut! appen lirar" — W0/W1/W2 ACCEPTED on his KDE box. New
  direction: replace numeric/coordinate fields with sliders + general polish.
- Polish pass (parent, ui-egui): ALL rail numeric DragValues → classic egui
  Sliders with blue trailing fill + integer readouts (Width/Border/Brush/
  Outline/Ring/Font size/Blur radius/Tolerance/Dim/Zoom + shadow Angle/Dist/
  Opacity/Blur). Shadow sliders moved to an EXPANDED SECOND RAIL ROW when
  Shadow is enabled (first attempt kept them inline → vision caught the
  right-edge overflow; restructure made all four visible at once).
  Status bar now: dimensions | active tool name | status.
- Vision-verified: no numeric boxes left, no clipping, blue fills, muted
  labels, classic look. Snapshots re-blessed (18 ui-egui tests).
- Gates: 180 tests / 0 failed, clippy/fmt/layers green. AppImage 11.77 MB
  launch-tested (exit 124). Delivered to rostad.
- Next: W3 shell parity (tray, global hotkeys, widget, settings, close-guard,
  single-instance, autostart, file associations) unless rostad redirects.

### 2026-11-16 — wake #9: W3 launched (shell parity)
- Fresh check: W0–W2 KDE-approved, polish delivered (c8f6714). Next: W3.
- W3 plan: docs/plans/2026-11-16-w3-shell-parity.md. Recon: live tray.rs
  (ksni load-bearing, left=restore/middle=widget/menu), hotkeys.rs
  (GlobalShortcuts portal, one BindShortcuts/session, Print/SHIFT+Print),
  close-guard tree (windows.rs:96-140), widget 58×74 ↔ 292×50.
- Parent scaffolded crates/platform (L6, tray-icon 0.25 ksni + ashpd
  global_shortcuts feature added workspace-wide), registered in xtask layers
  (22 crates), compile-verified before dispatch.
- Dispatched W3a (deleg_ff01afff): T1 platform (tray/hotkeys/autostart/
  single-instance socket, ported from live code, offline tests), T2 io::settings
  (Settings struct, XDG path, atomic save, corrupt-safe load) + engine
  settings.get/set commands.
- Next: W3b — settings panel UI, dark theme, close-guard modal, widget
  viewport, apps/snagcraft wiring (tray/hotkeys/CLI/launch_request/autostart/
  file associations), gates, AppImage.

### 2026-11-16 — wake #11: T2 verified + committed; W3b dispatched
- T2 (io::settings + engine settings.get/set) finished: parent-verified 19 io +
  22 engine tests, clippy clean; committed 950a01e. Manifest note: T2 added
  serde/serde_json to io (needed by Settings — parent wiring gap, accepted).
- T1 (platform) still running: all modules written (tray 264, hotkeys 328,
  single_instance 317, autostart 181, trigger 130, png 665 — png.rs likely a
  hand-rolled PNG→RGBA decoder for the tray icon since platform has no image
  dep; verify at integration).
- Dispatched W3b (deleg_2984262f): settings panel + themes + close-guard +
  widget viewport + tray/hotkey/instance/CLI wiring + file associations,
  against the verified-mechanism design + locked platform contract.
- Next: T1 + W3b land → verify → full gates → AppImage → KDE pass (tray click,
  Print/Shift+Print consent, widget drag, close-guard, settings persistence).
- W3a batch closed: T1 parent-verified (22 tests + 2 ignored live, clippy clean,
  layers 22) and committed 4350cc5 with assets/tray-32.png + ATTRIBUTION.
  Good child initiative: std-only PNG decoder (tray-icon 0.25 lacks a PNG entry
  point; manifests were off-limits) pixel-verified vs Python; 120s BIND_TIMEOUT
  after observing the portal hang with unshowable consent; edition-2024
  env::set_var is unsafe → pure resolvers instead (house-rule compliant).
  Steered W3b child: tray-32.png path, tray_available(), Bound/Failed gating.

### 2026-11-16 — wake #17: W3 integrated end-to-end
- W3b child finished (journal complete): settings panel (SettingsView mirror,
  settings.set dispatch + Shell-wrapper persistence), dark theme (all chrome
  reads visuals, accent #3d7ec2), close-guard decision tree via
  ViewportEvent::Close/CancelClose + doc_dirty tracking, widget immediate
  viewport (58×74 ↔ 304×60, park top-right, StartDrag, Enter/Space expand,
  Rec disabled until W4), app wiring (acquire_or_forward, tray/hotkey channels
  with graceful degradation, --capture/--region/path argv shared first+forwarded
  launch), desktop file %F + MIME (desktop-file-validate passes).
- Parent pre-verified all W3 snapshots by vision: settings (5 sections ✓),
  dark theme ✓, close-guard modal (plain copy + picture icon + Save/Discard/
  Cancel ✓), widget pill (white box in snapshot = kittest embedded-viewport
  artifact; code correct: transparent viewport + pill-only paint + clear 0).
- Parent gates: fmt ✓ (fixed own recording stub), clippy -D warnings ✓,
  **216 tests / 0 failed / 3 ignored** ✓, layers 23 ✓. Release + AppImage
  (12.8 MB) launch-tested (exit 124); tray gracefully unavailable on GNOME
  (no StatusNotifierWatcher — KDE has it); CLI 24 commands incl settings.
- Glyph lesson: ⚙ U+2699 absent from bundled fonts → painted gear (steered).
- Remaining W3 sign-off: rostad KDE live pass — tray click-to-restore,
  Print/Shift+Print portal consent, widget drag/transparency, hide-to-tray,
  settings persistence across restart.
- W3b completion report reconciled (all integrated in 55964c1): 28 ui-egui/app
  tests (10 w3_shell gates). Key design detail verified: eframe skips App::ui
  while hidden → channels drained in App::logic + 250ms heartbeat so hide-to-tray
  stays recoverable (lesson saved to skill; applies to W4 recording events too).
  SettingsView mirrors io::settings field-for-field (ui-egui stays io-free).
  autosave_captures: setting UI landed, save BEHAVIOR is W4 — parity row
  corrected to "W3 setting / W4 behavior" (honest ticking).

### 2026-11-16 — wake #18: W4 launched (recording + OCR/timelapse/upload/watermark)
- Fresh check: W3 complete on pn53 (d86b845), KDE live pass still pending rostad.
- W4 contracts locked in the plan (recording API, io::ocr/upload,
  capture::timelapse, compose::watermark). Loaded the proven recording recipe
  (tauri-desktop-app › recording.md) — all cost-real-time gotchas pasted into
  the W4a brief (target.object prop bug, registry wait, no SPA_PARAM_Buffers on
  consumer, VideoSize range, BGRx stride, teardown deadlock, offline probe).
- Dispatched W4a (recording: ScreenCast→PipeWire→ffmpeg, MP4/GIF, region crop,
  pause/resume, audio sources; offline Video/Source probe first) + W4c
  (OCR tesseract subprocess, upload open_url, timelapse thread, watermark).
- W4b (scrolling) follows W4a's ScreenCast source; W4d (UI: Rec live, recording
  controls, OCR panel, HUD) after backends. Trim question for rostad at W4d.

### 2026-11-16 — wake #20: W4c verified + committed; W4b dispatched
- W4c formally completed (transcript final line checked): parent-verified
  16 io + 44 capture + 42 compose tests = 103 (matches child report), clippy
  clean. Committed 9e7fbfe: io::ocr (tesseract, injection-safe lang, OCR-on-
  synthetic test), io::upload (http(s)-only), capture::timelapse, compose
  watermark (export-only by design).
- W3b straggler widget polish verified + committed f4ff0d3 (wake #19).
- Dispatched W4b scrolling (deleg_dab56bfb): pure row-signature + MAD aligner
  + vertical stitcher with synthetic-frame recovery tests first, ScreenCast
  HIDDEN-cursor source with the shared consumer traps; capture/Cargo.toml
  pipewire dep approved. W4a recording still running (modules written:
  lib 593 + audio 172 + crop 248 + ffmpeg 420).
- Next: W4a/W4b land → verify (offline probes) → engine command wiring
  (record/ocr/upload/timelapse/scroll + watermark export params) → W4d UI.

### 2026-11-16 — wake #21: W4c engine wiring landed
- Parent wired 7 new engine commands (31 total): ocr.available/languages/run
  (renders composite → tesseract), upload.open (scheme-guarded), timelapse.
  start/stop/poll (handle + event channel in Session, drained by poll), and
  doc.export watermark params {text,opacity,corner,size} → compose::apply_watermark.
- 3 new headless-safe tests (OCR availability + scheme guard, timelapse
  double-start/stop/missing-dir, watermark corner-pixel diff) — 22 engine tests
  green, clippy clean.
- W4a recording: 8 modules / 3084 lines written (capture.rs 802 PipeWire half,
  probe.rs 536 offline Video/Source probe, portal/session/audio/ffmpeg/crop) —
  still running. W4b scrolling: reading phase.

### 2026-11-16 — wake #23+: CI fixed, W4 backends landed, W4d dispatched
- rostad flagged GitHub CI notifiers: gates exit 101 + Node20/Ubuntu migration
  notices. Root cause: runners lacked libpipewire-0.3/libspa-0.2 dev headers
  (libspa-sys build panic). Fixed rust.yml: apt deps (pipewire dev, clang,
  tesseract, ffmpeg) + lavapipe (mesa-vulkan-drivers) for kittest snapshots
  (second failure: no wgpu adapter on bare runners), checkout/upload-artifact
  v4→v5 (Node24), pinned ubuntu-24.04. **CI GREEN** (run 43970030413, gates +
  appimage). Snapshots pass on lavapipe without tolerance changes.
- W4b scrolling landed + verified (49 tests; byte-exact 9-frame synthetic
  recovery, reverse scroll, guards) — committed 415c343; scroll engine
  commands wired (threaded start) 43f174c.
- W4a recording landed + verified (48 tests + offline probe ffprobe-proven:
  h264 200×120 19f MP4 + GIF) — committed b965a7f; record engine commands
  (start/stop/pause/resume/state/poll + audio.sources) wired f135c61.
  41 commands total, 24 engine tests.
- W4d UI dispatched (deleg_6fed6c7d): live Rec (V), recording HUD + region/
  format/audio, scrolling overlay + open-in-editor, OCR window + copy-text,
  timelapse dialog + HUD, watermark export settings, autosave-on-capture.
  Trim deliberately OUT (live has none; spec lists it — ask rostad).
- Next: W4d lands → verify → full gates → AppImage → W4 delivery + KDE pass
  (live recording/scrolling need rostad's unlocked session).
- W4a/W4c batch reports reconciled (already committed b965a7f/9e7fbfe/f135c61).
  New gold saved to skill: TWO real PipeWire deadlocks W4a found+fixed —
  (1) dropping ContextRc/CoreRc joins module-rt thread stuck in rtkit D-Bus →
  stop hangs; leak core+context+registry too, not just stream/listener/mainloop;
  (2) `pw-cli info 0` enters its REPL even with stdin(null) — not a liveness
  probe; use bounded producer startup. W4c extras: OCR test hand-drew a 5×7
  block font (O needed full-width bars or tesseract reads U); upload reaps the
  detached child on a helper thread (live blocked on .status()); timelapse
  frames 5-digit padded so ffmpeg glob order = frame order. Unused raster dep
  in recording manifest already dropped by parent.

### 2026-11-17 — wake #32: W4 integrated end-to-end
- W4d completed (1131s): live widget Rec (V widget-scoped / Ctrl+Alt+V main),
  recording HUD + region/format/audio options, scrolling overlay → open-in-
  editor, OCR window + copy-text, timelapse dialog + HUD, watermark export
  (persisted), autosave-on-capture dedup. Widget recording layout fixed per
  parent vision QA (filled red Stop square + single Pause + red dot + ×).
- Parent verification: 40 ui-egui/app tests (12 w4_live gates), **363 tests /
  0 failed / 5 ignored workspace-wide**, clippy/fmt/layers green. Release +
  AppImage (13.7 MB) launch-tested (exit 124), CLI 41 commands.
- Watermark/upload persistence closed both sides (engine fields c0a1bab +
  SettingsView wiring).
- Parity: 28 W4 rows ticked HONESTLY — 9 bulk-ticked rows reverted to pending
  after audit (overclaim catch): OCR lang-select/busy modal, timelapse ffmpeg
  video assembly (make_timelapse), Ctrl+. stop hotkey, shrunk countdown HUD,
  upload POST-to-endpoint flow (curl) — current upload = open_url only.
- W4 remaining gaps (W5/W6 backlog or rostad priority): recording trim
  (spec lists, live lacks — asked rostad), audio mic+monitor mixing, Recent
  rail video thumbnails, copy-diagnostics button, the 9 reverted rows above.
- Delivered W4 AppImage for rostad's KDE live pass: Rec → portal consent →
  record → stop → file; scrolling capture; OCR; timelapse. Portal legs need
  his unlocked session (pn53 locked; offline probes proven).

### 2026-11-17 — wake #33: W5 launched (automation + WASM)
- W4 delivered (3b45839), awaiting rostad KDE pass + trim decision. Next: W5.
- W5 plan: docs/plans/2026-11-17-w5-automation-wasm.md. Recon: photocraft
  control-protocol.md (loopback + token-file 0600 + first-frame auth +
  registry methods), rmcp 3.10 on crates.io.
- Parent pre-wired crates/automation (rmcp 3.10 server/transport-io/macros/
  schemars + tokio full-ish + getrandom 0.3 + schemars 1), compile-verified.
- Dispatched W5a (deleg_2502ae8e): TCP JSON control server (auth, commands.list,
  command.run, session.info, doc.inspect, ui.screenshot PNG-base64) + rmcp MCP
  stdio server (5 tools from registry), offline integration tests.
- W5b (parent, after W5a): app --control/--control-token-file flags + CLI
  mcp/control subcommands. W5c (next wake): WASM web build + xtask wasm gate
  (L0–L5 wasm32 compile, engine native/web feature split, trunk app).

### 2026-11-17 — wake #34: W5a committed; W5b + W5c-engine in flight
- W5a complete + committed (4fc29a6): TCP control + MCP stdio, 18 tests,
  rmcp Apache-2.0 (plan corrected). CI green after Node24 + snapshot-policy
  + fmt + consistent-tree fixes (run 44040349228 success).
- W5b dispatched (deleg_3927f082): ui-egui Session→Arc<Mutex> (sess() accessor,
  behavior-identical), app --control/--control-token-file + shutdown, CLI
  mcp/control subcommands (auth-first no-ack client).
- W5c recon: wasm32 target installed; ALL 8 pure crates (L0–L5) already
  compile wasm32 ✓. io/engine fail only on arboard (clipboard). capture/
  recording/platform/automation native-only.
- W5c-engine dispatched (deleg_ac3150ee, PARALLEL with W5b — disjoint crates):
  io/engine native/web feature split (native default byte-identical, web core
  compiles), xtask wasm gate. W5d (ui-egui web cfg + apps/snagcraft-web trunk)
  follows after W5b lands.

### 2026-11-17 — wake #39: W5b + W5c committed; W5d (web app) dispatched
- W5c-engine committed (32f90b9): native/web split, 21/41 commands in wasm
  rlib, xtask wasm gate (10 web crates), 68 tests, native byte-identical.
- W5b committed (7472f54): ui-egui Arc<Mutex> + sess(), app --control flags,
  CLI mcp/control. Parent LIVE round-trip verified: app --control 59990 ↔
  CLI → commands.list=41, doc.new=64x48, ui.screenshot=valid PNG, wrong
  token rejected, token 0600. Workspace 392 tests / 0 failed.
  (Gotcha: first smoke used STALE release binaries — the build had been in
  a rejected '& command'; rebuilt before the real round-trip.)
- W5d dispatched (deleg_d34add8c): ui-egui native/web feature split (platform/
  rfd/widget/clipboard cfg-gated, web PNG export-download via web-sys) +
  apps/snagcraft-web eframe WebRunner trunk app. trunk + wasm-bindgen-cli
  installing in background (proc_cd81b2c95668). Web app scaffold pre-wired.

### 2026-11-17 — wake #50: W5 COMPLETE (web app + watermark gap closed)
- W5d committed (3273af1): ui-egui 55 cfg gates (native byte-identical, 40
  tests), web fallbacks (PNG export-download, drag-drop open, clipboard
  status, Capture/OCR/widget/tray hidden on web); apps/snagcraft-web eframe
  WebRunner (body cfg wasm32, empty native lib), trunk dist 11MB wasm.
  Browser-verified: boot + full editor render + 14118-byte PNG export.
  Parent-verified: native 40 + workspace clippy + web clippy + fmt, 392/0.
  (Child over-verified with repeated browser screenshots — steered to wrap.)
- Watermark-on-web gap CLOSED: added Session::render_watermarked (public,
  same path as doc.export, refactored export to use it); web export_dialog
  now applies settings watermark via render_watermarked. engine+ui-egui 65
  tests green (export_watermark test preserved), web+native clippy clean,
  trunk rebuilt (11MB wasm). Web export now identical to desktop.
- W5 remaining MVP limits (documented, not gaps): web image copy/paste =
  status only (no ClipboardItem write); drag-drop open code-complete but not
  browser-exercised. recording crate has 9 pre-existing warnings on plain
  wasm check (outside web graph; clippy clean).
- W5 done: TCP control + MCP + CLI (live-verified) + WASM web app. Next W6.

### 2026-11-17 — wake #51: W6 launched (packaging + docs + dossier)
- W5 complete (794f143). W6 scope (spec §5): packaging polish (AppImage/deb,
  Windows/macOS aspirational), docs (README/gaps/parity), suite dossier.
- Scope reality: platform layer LINUX-ONLY (ashpd/ksni/PipeWire) → Windows
  NSIS/MSI + macOS dmg NOT feasible without platform backends; documented
  FUTURE in gaps.md. Cross-platform reach NOW via WASM web build.
- W6 plan: docs/plans/2026-11-17-w6-packaging-docs.md. dpkg-deb present →
  manual .deb (no cargo-deb). GATES: no release publish, no external posting,
  relicense/cutover = rostad.
- Dispatched W6a (sa-0, packaging: deb.sh + AppImage polish + rust-release.yml
  CI artifact workflow, NO publish) + W6b (sa-1, docs: README rewrite +
  gaps.md + suite-candidacy.md INTERNAL dossier + parity audit) — parallel,
  disjoint (packaging/.github vs docs/README).

### 2026-11-17 — wake #56: W6 COMPLETE — all waves W0–W6 built
- W6a committed (bbd2683): deb.sh (manual dpkg-deb, verified -I/-c, 10.2MB),
  rust-release.yml (workflow_dispatch + tag v* → AppImage/.deb/web-dist →
  upload-artifact ONLY, no release publish — gated), .desktop Categories
  fix (Graphics; only, validates clean). AppImage alive, YAML valid.
- W6b complete (docs verified grounded): rust/README.md (264 lines, L0–L7
  architecture, 41/21 commands, CLI/control/MCP/web, licence/gates),
  docs/gaps.md (14 entries A1–D2), docs/suite-candidacy.md (INTERNAL,
  no external posting), parity audit (150 rows, pending 68→45, 23 new
  code-verified ticks, 9 W4-reverted kept pending, stale W1 note replaced).
- W6b honest findings: NO recent rail at all (grep recent=0, not just video
  thumbs); capture.full has xcap macOS/Windows fallback (shell/recording
  Linux-only, capture not); ScreenCraft #2148/#2337 spec-only (dossier marks
  unverified); SUITE CANDIDACY BLOCKERS — GPL not on suite allowlist (core
  rule 9 stop-and-ask = relicense gate A3), fonts must move to craft-fonts
  (rust rule 3), CI must adopt craft-common-actions (core rule 11).
- MISSION BUILT: W0 probe gate, W1 scaffold/engine/capture, W2 tool parity
  (KDE-approved), W3 shell, W4 recording/scroll/OCR/timelapse, W5 automation
  + WASM web, W6 packaging/docs/dossier. 392 tests / 0 failed. Native
  AppImage + .deb + CLI + TCP/MCP control + browser web app, one Rust engine.
- REMAINING (all rostad-gated, not autonomous): W3/W4 KDE Plasma Wayland
  live pass; trim decision; relicense (GPL→MIT/Apache, also suite blocker);
  cutover; any external post / suite submission. NO releases published.

### 2026-11-17 — wake #57: loop re-fired after LOOP_COMPLETE → W7 parity backlog
- Loop woke again after wake #56's LOOP_COMPLETE. Re-checked fresh: W0–W6
  built (5474025), 392 tests. Full parity not yet met (45 pending rows);
  implementable offline backlog exists → continue toward parity (spec goal).
- W7 plan: docs/plans/2026-11-17-w7-parity-backlog.md. Target gaps B4#1–3
  (offline-verifiable, no rostad gate): timelapse.assemble (ffmpeg glob →
  h264 mp4), OCR language picker (io ocr_lang + ComboBox), OCR busy modal.
  Rows 4–9 (Ctrl+., countdown HUD, upload POST) deferred (live/network scope).
- Dispatched W7 (deleg_b4966464, one track, 3 features). ffmpeg 8.0.1 +
  tesseract confirmed present. Parent commits + ticks parity/gaps after.

### 2026-11-17 — wake #65: W7 COMPLETE (parity backlog B4#1–3 closed)
- W7 done (808s, 14 new tests): timelapse.assemble (recording/assemble.rs
  glob→h264 mp4, hostile-safe pattern, in-crate glob matcher, 300s bounded
  wait+kill, stderr log; engine native command, registry 41→42), OCR language
  picker (io ocr_lang persisted + ComboBox from ocr.languages), OCR busy modal
  (off-thread worker, spinner + disabled Extract, cleared on result/error/dead).
- ffprobe evidence: 5 odd 33×21 PNGs → h264 34×22 yuv420p nb_frames=5
  duration=0.416667 (even-dims pad works, decoy ignored).
- Parent fixed the sole red gate: automation control test hardcoded 41 →
  bumped to 42 (automation parent-owned, outside child scope) + rustfmt wrap.
- Parent-verified: workspace 390 passed / 0 failed / 5 ignored, clippy/fmt/
  layers/web-check clean. README 41→42 (web stays 21, assemble native-only).
- Parity B4#1–3 ticked W7; gaps.md B4 rows 1–3 marked CLOSED.
- Undone (documented): no "Make video" button in timelapse dialog (command +
  tests only, as scoped); ocr_lang single codes only (no eng+swe under the
  alnum/-/_ contract). Rows B4#4–9 still pending (Ctrl+., countdown HUD,
  upload POST — live/network scope).
- Child over-reconned early (steered to write) + vision-looped late (steered
  to cargo gates) — both steers effective.

### 2026-11-17 — wake #73: W8 COMPLETE — entire B4 backlog closed
- W8 done (688s, 18 new tests): upload POST flow (io upload_image via curl
  -F file=@-, http(s) guard before spawn, 30s bounded wait+kill, 1MiB cap,
  threaded pipes no deadlock, link parse+revalidate; engine upload.send
  native, registry 42→43; ui-egui File→Upload refuse-no-image/endpoint,
  copy link+status), Ctrl+. stop, compact countdown HUD (CAPTURE IN + number,
  capture_delay_secs driven). Real TcpListener mocks assert POST/multipart/
  PNG magic/link round-trip; hostile file://javascript: rejected no-spawn.
- PARENT parity fix: child wired Ctrl+.→record.stop, but live (app.ts:5181)
  stops INTERVAL capture (timelapse). Corrected handler→app.timelapse.running
  →stop_timelapse + rewrote the 2 kittest (7/7 green). Honest parity.
- Parent bumped automation drift-guard 42→43 (parent-owned) + fmt.
- Parent-verified: workspace 424 passed / 0 failed / 5 ignored, clippy/fmt/
  layers/web clean. Snapshots reviewed (countdown HUD clean; File button
  neatly inserted, no clipping; widget-in-harness popover = known artifact).
- Parity B4#4–9 ticked W8 (7 rows); gaps.md ALL 9 B4 rows CLOSED; README 43.
- B4 backlog fully closed. Remaining parity pending rows are other features
  (recent rail, W2 residuals etc.) + rostad gates (KDE pass/trim/relicense/
  cutover). NO releases published.

### 2026-11-17 — wake #84: W9 COMPLETE (save & recent UX)
- W9 done: io build_filename ({date}/{time}/{datetime} via chrono, fs-safe) +
  default_save_dir (HOME/Pictures/SnagCraft) + scan_recent (newest-first cap
  12); engine doc.save uses pattern + doc.save_as (registry 43→44); ui-egui
  Save As (Ctrl+Shift+S + File menu), settings save-location Change/Reset,
  Recent filmstrip (toggle chip + count + Hide, in-session + folder scan,
  click-to-open). Recording thumb = film-icon placeholder.
- Child ran way over budget (140 calls, 15min, never reported despite 2
  wrap-up steers) → PARENT STOPPED it + integrated directly from on-disk
  artifacts. Parent bumped automation drift-guard 43→44 (verified 13/13).
- Parent-verified: workspace 451 passed / 0 failed / 5 ignored, clippy/fmt/
  layers/wasm (10 web crates) clean. Recent strip snapshot reviewed: chip
  "Recent (3)" + filmstrip 3 labeled thumbs + Hide, counts agree, clean.
  (Pre-existing narrow-width "Sha…" toolbar overflow noted, not W9.)
- Parity 5 rows W9 (Save As, filename pattern, default dir, Recent strip,
  recording-thumb placeholder); README 44; gaps W9 note. Still pending:
  separate recording folder, New-canvas presets, Resize dialog, settings
  filename live-preview + other long-tail + rostad gates.

### 2026-11-17 — wake #90: W10 COMPLETE (canvas presets + Resize)
- W10 done: New-canvas dialog (Ctrl+N + File→New, 8 presets Default 1280×800
  … Small 800×600 + custom W/H + Create, dirty-guard); engine doc.resize
  (scales base + annotation geometry, clamp 1..=16384, registry 44→45) +
  ui-egui Resize… dialog (prefilled dims, Esc closes). kittest w10_canvas.rs.
- Child hit 70-call budget chasing snapshot shifts → PARENT STOPPED +
  integrated. New toolbar buttons shifted 7 existing snapshots; re-blessed
  with UPDATE_SNAPSHOTS=1 --no-fail-fast (default stops at first fail before
  reaching ui-egui — must use --no-fail-fast to bless all).
- Root-caused a residual 17px clean-run diff: a blinking text caret/focus
  speck (time-based render non-determinism, top-center toolbar), NOT a
  regression. Added rust/kittest.toml max_failed_pixels=64 (~0.008% of frame;
  real regressions change hundreds+ px and still fail). Stable across 2 runs.
- Parent bumped automation drift-guard 44→45 (13/13). Parent-verified:
  workspace 459 passed / 0 failed / 5 ignored, clippy/fmt/layers/wasm clean.
  New-canvas snapshot reviewed: 8-preset grid + custom fields + Create/Cancel,
  clean. Parity 94/95 W10; README 45; gaps W10 note.
- Remaining pending ~28 rows: settings rows (hotkeys enable/register, mic/
  system audio, recording format, filename live-preview), separate recording
  folder, copy diagnostics, PrintScreen fallback, text-caption settle,
  dirty-state, editor decorations + rostad gates.

### 2026-11-17 — wake #97: W11 COMPLETE (settings + diagnostics + PrintScreen)
- W11 done: settings cluster (interval period, mic source dropdown from
  audio.sources, system audio toggle, enable-global-hotkeys + Register button,
  recording format MP4/GIF, separate recording folder + Change/Reset, filename
  live-preview via build_filename); engine diagnostics.copy (portal log +
  .rec.log tail + version/OS → clipboard) + Troubleshooting button;
  PrintScreen/Shift+PrintScreen in-app fallback. Settings window vscroll=true
  (panel outgrew short screens). Registry 45→46.
- Child over-ran budget (83 calls) stuck on a kittest settings test → PARENT
  STOPPED + integrated. Root cause: W11's new rows pushed the Theme radio below
  the scroll fold; coordinate .click() missed it. FIX: click_accesskit() (clicks
  off-screen widgets) in theme_swap + settings_change tests; re-blessed
  w3_shell_dark (stale 760→1400 size). scroll_to_me alone didn't work.
- Parent bumped automation 45→46; cargo fmt --all (child left unformatted).
- Parent-verified: workspace 468 passed / 0 failed / 5 ignored, clippy/fmt/
  layers/wasm clean. Settings snapshot reviewed: General/Recording audio/Export
  clean; lower sections below scroll fold (reachable, tests confirm).
- Parity 14 rows W11; README 46; gaps W11 note. Remaining pending ~14 rows:
  editor residuals (text-caption settle, dirty-state, crop decorations) +
  recording thumbnails (real video) + rostad gates.

### 2026-11-17 — wake #100: W12 COMPLETE — FULL PARITY REACHED
- W12 done (65 calls, within budget): shortcuts overlay (? + File→Keyboard
  shortcuts, real-bindings SHORTCUTS const next to handle_keys; newly wired
  Ctrl+O + Ctrl+scroll zoom 0.25–4.0, default 1.0 byte-identical), About modal
  (File→About SnagCraft, accent mark + name + version 0.1.0 + tagline + repo,
  Esc/OK), derived dirty-state (engine saved_depth + dirty()/history_depth();
  save point moves on save/save_as/export, resets on new/open/capture; undo to
  save point clean again — sticky flag couldn't), ffmpeg-absent actionable
  message (Encoder::spawn + assemble). Editor residuals VERIFIED not rebuilt
  (caption settles on lost_focus; crop dim/frame painter-only, no doc write).
- Registry UNCHANGED (only session.info fields) → no automation bump.
- Parent polish: shortcuts ScrollArea scrollbar AlwaysVisible (vision flagged
  bottom rows "clipped" — actually below scroll fold; made affordance obvious),
  re-blessed w12_shortcuts. About snapshot reviewed: perfect.
- Parent-verified: workspace 474 passed / 0 failed / 5 ignored, clippy/fmt/
  layers/wasm clean. Parity 13 rows W12 → **pending = 1 (Trim, rostad-gated)**.
- Delivered fresh AppImage (13.9MB, launch-tested exit 124, CLI 46 commands) to
  rostad on his "När finns en version redo att testa?" — full feature set.
- MISSION: W0–W12 built, full parity except gated Trim. Awaiting rostad KDE
  live pass + gates (trim/relicense/cutover/external post). No releases.

### 2026-11-17 — wake #101: published Rust-edition pre-release (rostad authorized)
- rostad: "släng upp appen som en uppdatering i repot" → explicit publish OK.
- Existing v0.1.0 tag was the OLD Capicorn first-alpha (2026-11-14) — left
  untouched (no destructive overwrite). Used distinct tag **rust-v0.1.0**.
- Created GitHub pre-release rust-v0.1.0 (--prerelease --latest=false): assets
  SnagCraft-x86_64.AppImage (13.95MB) + snagcraft_0.1.0_amd64.deb (10.34MB),
  both verified uploaded. v1.5.3 remains stable "Latest" (no cutover).
- URL: https://github.com/rostaddotcc/snagcraft/releases/tag/rust-v0.1.0
- Still gated: cutover (promote Rust to stable / retire Tauri), relicense,
  trim, KDE live pass. This is a pre-release for testing, not a cutover.

### 2026-11-17 — wake #102: W13 audio mixing (B1 closed) — last functional gap
- Fresh re-check first: tree clean, no TODO/FIXME in app code, rust-v0.1.0
  pre-release still live (both assets, v1.5.3 still "Latest"), workspace
  474/0/5 + clippy/fmt clean. Parity pending = 1 (Trim, rostad-gated). The one
  remaining NON-gated functional gap was gaps.md **B1** (mic + system-audio
  mixing) — the W11 settings toggle existed but only one source ever recorded.
- W13 (parent-built, small): `AudioConfig` gains `system_audio` + `input_count()`;
  encoder opens one `-f pulse` input per source (mic first, then
  `@DEFAULT_MONITOR@`), two inputs mixed via `amix=inputs=2:duration=longest`
  into ONE aac track — the live `audio_output_args` recipe ported verbatim
  (aac 160k, `-shortest`). GIF stays silent; 0 sources = `-an`, never an error.
- `record.start` accepts `system_audio`; both audio params fall back to the
  persisted settings (W11 format precedent) via a pure `record_audio_config`
  helper. UI now passes the settings toggle. Capture thread carries the full
  `AudioConfig`; `.rec.log` line reports mic/system.
- Registry UNCHANGED (46) → no automation bump. 7 new tests (input_count, 4
  arg-shape, settings-fallback, + a LIVE end-to-end mix).
- **Live-verified on pn53 (pipewire-pulse present):** the encoder child mixed
  two real pulse sources into exactly ONE ffprobe-confirmed aac stream beside
  h264 (`the_encoder_mixes_two_pulse_sources_into_one_aac_track ... ok`). The
  probe self-skips on headless CI (no pulse socket), so it stays green there.
- Parent-verified: workspace **481 passed / 0 failed / 5 ignored**, clippy 0,
  fmt clean, layers legal (23 crates), wasm all 10 web crates compile.
- gaps.md B1 → CLOSED; parity audio row → W4 + W13. Remaining pending = 1
  (Trim, rostad-gated). No release re-publish (pre-release assets are W12-era;
  a refresh is rostad's call). Awaiting KDE live pass + gates.
- Fresh W13 artifacts built from the committed state (df3b20e, pushed):
  AppImage 13.9MB launch-tested (exit 124, benign locked-GNOME noise only) +
  deb 10.3MB, in rust/dist/w13/. CLI smoke: 46 commands, record.start doc
  shows system_audio. Delivered the AppImage to rostad.
- **B5 audit (same wake):** item 1 (crop re-grab) is NOT a gap — the live crop
  overlay is painter-only (selectable/evented false), the "handles" rail hint
  is prose. Items 2–3 CONFIRMED: no double-click text edit / select-mode resize
  (live fabric Textbox), no nib preview circle (live app.ts:433/993/3163 —
  parity row 52 W2 tick overclaimed, corrected). New nuance: live plain-click
  (<8px MIN_DRAG) resets the crop; rebuild leaves a 0×0 frame. **W14 scope:
  nib preview + double-click text edit (+resize) + crop click semantics.**
  The loop continues — editor interaction parity is the last open cluster.

### 2026-11-17 — wake #103: W14 editor interaction parity — B5 ALL CLOSED
- Fresh re-check: tree clean at 11a9b1e (W13 + promo renderer), suite green.
  W14 = the B5 cluster from the W13 audit, built parent-direct.
- **engine**: `edit.set_text {id,text}` + `edit.resize {id,rect}` (history-backed;
  wrong kinds report false, degenerate rects rejected). doc gains set_text/
  set_rect. Registry 46→48; automation guard + engine registry test bumped.
- **ui**: Select mode gets corner resize handles (7px accent squares, diagonal
  resize cursors, anchor-opposite-corner live preview) + double-click inline
  re-edit of Text/Callout (loads the object's own style; commit → set_text;
  emptying removes the object — live settle parity; quick-font bar hidden for
  re-edits). Brush rail gets the live nib preview (48px box, ink dot at 1:1
  diameter, value label — app.ts:433/993/3163). Crop: <8px drag/click resets
  the pending frame, Enter needs ≥8px (MIN_DRAG/MIN_SIZE parity).
- **Gates (kittest pointer paths, 60fps step_dt):** resize+undo, text
  re-edit/append/empty-remove, callout re-edit, crop click reset, nib snapshot
  (vision-verified). Two harness lessons: the properties rail height differs
  per tool → calibrate the image→screen mapping UNDER the exercised tool (a
  resize to a known screen point yields the absolute offset; a move only
  yields scale); kittest's default 250ms step_dt puts consecutive clicks
  outside egui's double-click window → with_step_dt(1/60), and three clicks
  inside 300ms count as TRIPLE (pause between pairs).
- Parent-verified: workspace **487 passed / 0 failed / 11 ignored**, no
  snapshot drift. clippy/fmt/layers/wasm clean (below).
- gaps.md B5 → ALL CLOSED; parity select/crop/brush rows → W2 + W14; README
  48 commands. **Pending parity = 1 (Trim, rostad-gated). No non-gated
  functional gaps remain.** Awaiting rostad: KDE live pass, trim decision,
  relicense, cutover, release-asset refresh. Promo film storyboard still
  awaiting his go (separate track).
- Fresh W14 artifacts from committed dfca838: AppImage 13.95MB launch-tested
  (exit 124) in rust/dist/w14/, CLI smoke 48 commands incl. edit.set_text +
  edit.resize. Delivered to rostad for the KDE live pass.
- **LOOP BACKLOG EMPTY**: every non-gated parity row and gap register entry is
  closed. Further iteration needs rostad's gates/feedback → loop completed.

### 2026-11-18 — wake #104: W15 timelapse stop→MP4 — a real gap behind a green checklist
- Fresh re-check: tree clean at 6869563, suite green (487/0/11). The W14 close
  said "backlog empty", but a code-level re-audit of the timelapse flow found a
  genuine functional gap: **the rebuild never assembled the video**. The live
  app auto-calls `make_timelapse` when a run stops (`app.ts:3019-3047`, 10 fps,
  `SnagCraft-timelapse-{date}-{time}.mp4` in the run folder); the rebuild's
  `poll_timelapse` "done" arm only reported the frame count. `timelapse.assemble`
  existed as a command (W7) but nothing in the UI flow called it. Parity row 191
  ("stop converts the run to MP4") was ticked W4 — an overclaim: the loop shipped,
  the conversion did not. gaps.md's "no Make video button" framing was also wrong
  (the live app has no button — it auto-assembles).
- **W15 fix (parent-direct):** `finish_timelapse(frames)` — zero shots → live
  "nothing was saved" wording; shots > 0 → "Building a timelapse from N shots…"
  + off-thread `assemble_timelapse` (pure L4 call, **no session lock** — the
  editor keeps painting during the encode, unlike the OCR worker). Result poll
  in `poll_media` (runs hidden too): "Timelapse saved: path (N shots, 10 fps)"
  + push to the Recent rail; failure → "Timelapse failed: err — the N shots are
  still in dir" (shots never lost); disconnected worker cleared likewise.
  Failed capture runs with shots on disk get the same shots-kept note. Run
  folder tracked in `TimelapseState.dir`; `assembling` joins the 250 ms repaint
  heartbeat. Dialog subtitle now says what stopping does ("…stopping builds a
  10 fps video."). ui-egui gains an optional native `snagcraft-recording` dep
  (L4→L6, layers legal).
- **Gates:** `tests/w15_timelapse.rs` — real ffprobe-verified h264 encode
  (3 frames, 34×22 padded) + recent-rail pickup + live wording; empty-run
  wording; NoFrames failure clears the flag and a second run assembles again
  (runs even without ffmpeg — NoFrames precedes the spawn). Dialog snapshot
  re-blessed (447 px, vision-confirmed subtitle-only) and vision-verified.
- Parent-verified: workspace **490 passed / 0 failed / 11 ignored**, clippy/fmt
  clean, layers 23 crates legal, wasm 10/10. No new engine commands → automation
  guard untouched (48).
- Docs: parity row 191 split (loop W4 / stop→MP4 W15), stale W6b summary table
  recounted row-by-row (claimed 45 pending; actual 1 — Trim, rostad-gated;
  151 rows total). gaps.md B4 row 3 residual closed with the corrected framing.
- **Pending parity = 1 (Trim, gated). No non-gated functional gaps remain —
  again, but this time the timelapse flow was audited end-to-end against the
  live anchor, not just the checklist.** Awaiting rostad: KDE live pass, trim
  decision, relicense, cutover. Web C2/C3 polish + design pass = next wakes.

## Appendix — measured reality

| Claim | Measured | What changed |
|---|---|---|
| Parity row 191 ticked W4 ("stop converts the run to MP4") + W14 "backlog empty" | The UI's `poll_timelapse` done-arm only reported the frame count — nothing ever called `timelapse.assemble`. The live app auto-assembles on stop (`app.ts:3019-3047`); it has no "Make video" button (the gaps.md framing was wrong too) | W15 wires stop→MP4 off-thread (no session lock). Row split: loop W4 / conversion W15. Lesson: a closed checklist is not an audit — re-read the live anchors of headline flows end-to-end, not just the row text |
| W1 drag-create worked (spike pattern) | egui's hover_pos on the first drag frame is already the MOVED point → every drag-created object was 0px | UI child's parity.rs drag gates caught it; fixed with pointer.press_origin(). Lesson: seeded-command snapshots cannot catch pointer-path bugs — drag gates are mandatory |
| ab_glyph `ascent_unscaled() * scale` gives pixel ascent (W0 spike pattern) | Returns RAW font units (Ubuntu-Light ascent=932, upem 1000) → baseline 26 000 px off-canvas → silent text loss in CPU composite | compose uses `font.as_scaled(scale).ascent()` / `.h_advance(gid)` (ScaleFont trait); W0 spike export had the same latent bug (its on-screen text came from egui painter, which masked it) |
| egui = "0.37" bundles fonts by default | egui's own `default = []`; `default_fonts` is enabled via eframe — CLI (egui-only) got EMPTY FontDefinitions | workspace egui enables `default_fonts` explicitly; CLI text export verified pixel-wise |
| Child "tests green" claims imply visual correctness | compose passed 7 unit tests yet rendered no text (no font-present pixel test existed) | parent vision + pixel probes are mandatory on render paths; added render_probe regression tests |
| Blur visible in seeded snapshot | Gaussian blur on a pure-white base is invisible | seeded() loads a synthetic gradient base (load_surface) |
| photocraft layering forbids ALL same-layer deps | L0 foundation (raster→geom/color) needs same-layer edges | xtask layers allows same-layer edges but rejects cycles (DFS) + any upward edge; UI ban below L6 unchanged |
| egui main-branch docs match crates.io 0.37 | 0.37 renamed APIs: `Panel::top/bottom` (unified, `.show(ui,…)` not `.show(ctx,…)`), `eframe::App::ui` replaces `update`, `Ui::id`→`scope_id`, `Id::new` deprecated, `DroppedFile` is a trait (`.path()`), `FontData.font` is a `Blob`, `TextEdit::frame` takes `Frame` not bool, ab_glyph `Scale`→`PxScale`, `ascent_unscaled()` takes no args | All fixed against the vendored 0.37 registry source; briefs for W1+ must cite 0.37, not main |
| `EFRAME_SCREENSHOT_TO` usable for headless shots | Asserts "not yet implemented for wgpu backend" (wgpu_integration.rs:143) | Use `ctx.request_screenshot(cb)` (wgpu-supported) or egui_kittest offscreen |
| `--selftest` screenshots on pn53 | Locked GNOME session delivers no frame callbacks → frame counter never advances → no shot, no auto-exit (timeout 124) | kittest headless snapshot is the pn53 visual gate; selftest expected to work on unlocked KDE |
| kittest `Harness::new_egui` | Does not exist in 0.37; use `Harness::builder().with_size(…).build_ui_state(\|ui,state\|…, state)` | Test written against the real API |
