# SnagCraft — ArtCraft suite candidacy dossier

**INTERNAL.** Planning document for rostad. Nothing here may be posted externally —
no issues, PRs, comments, Discord messages or release notes in `storytold/*` or
anywhere else until rostad says so (see Gates). Do not paste this file into a
craft-contrib issue.

Status: W6b, 2026-11-17. Evidence base: the rebuild workspace `rust/`, the status
log (`docs/plans/rust-rebuild-status.md`) and the gap register (`docs/gaps.md`).

---

## 1. The proposal in one paragraph

SnagCraft is a screenshot/annotation/screen-recording app — the Snagit workflow —
written as a native Rust app in the suite's architecture: sans-I/O engine, enforced
L0–L6 layers, one command registry driving UI, CLI, TCP control and MCP, and the same
crates compiled to WASM for the web build. It is the best-fit candidate for a
**capture** niche in the Craft App Suite, which today covers photo, vector, film,
audio, documents and CAD but has no screen-capture/annotation app.

## 2. The niche — ScreenCraft (#2148/#2337)

The rebuild spec cites the niche as **ScreenCraft, #2148/#2337**
(`docs/specs/2026-11-16-snagcraft-rust-rebuild.md:131`, repeated in
`docs/plans/2026-11-17-w6-packaging-docs.md:56`).

**Source check (W6b):** those numbers and the name "ScreenCraft" do **not** appear
anywhere in `tmp/research/suite/` (`house-rules-core.md`, `house-rules-rust.md`,
`craft-launcher-AGENTS.md`). They come only from the spec, so treat them as
**unverified references** — confirm the numbering against the actual suite tracker
(wherever #2148/#2337 live) before they are used in any pitch. This dossier does not
re-derive them.

What the launcher's own `AGENTS.md` does establish: the suite is *PhotoCraft,
LightCraft, FilmCraft, DesignCraft, VectorCraft, PdfCraft, EffectCraft, WordCraft,
DeckCraft, GridCraft, CADCraft, SoundCraft, FigCraft, TextureCraft, MaterialCraft,
Craft Anything*, plus approved community apps (SolidCraft, MailCraft). **There is no
capture app in that list** — so candidacy means a new suite app, not a replacement.

Open naming question: the repository and product are **SnagCraft**; the spec's niche
name is **ScreenCraft**. Either the app is renamed on acceptance or the niche keeps
the SnagCraft name — rostad's call, and it affects repo, binary, app-id, desktop
entry and the `.snagcraft` format name.

## 3. House-rule fit (evidence-based)

Checked against `tmp/research/suite/house-rules-core.md` (15 rules) and
`house-rules-rust.md` (6 rules + licence allowlist). Only the rules where SnagCraft
has something to show or something to fix are listed.

| Rule | Fit | Evidence / action |
|------|-----|-------------------|
| Rust #1 never crash, no `unsafe` | **met** | `unsafe_code = "forbid"` workspace-wide; every crate root carries the `deny(unwrap_used, expect_used, panic, unimplemented, todo, unreachable)` attribute; hostile-input tests (format tamper rejection, hostile object seeds, bounded frame size, sanitised params) |
| Rust #2 clean room | **met** | rebuild written from the behaviour of rostad's own v1.5.x app plus public specs; assets original; `rust/ATTRIBUTION.md` records icon and fonts; workspace has a dependency-allowlist check |
| Rust #3 assets + fonts in `craft-fonts` | **needs work** | 14 OFL faces live in `rust/assets/fonts/` (embedded with `include_bytes!`, so no runtime file access). The rule puts font files in `storytold/craft-fonts`. Either an exception or a build-time fetch is needed before candidacy; the OFL texts are already kept |
| Rust #4 Rust only, no webview, no Tauri | **met, and it is the strongest point** | native egui/eframe; the web build is the same Rust compiled to WASM via trunk — the launcher's `AGENTS.md` describes exactly this arrangement for itself ("no Tauri, Electron or webview shells, ever"). No handwritten JS/TS: `index.html` is markup and CSS only |
| Rust #5 time through `chrono` | **currently compliant in intent** | no calendar conversion anywhere: file stamps are epoch seconds (`epoch_secs`, "file-name stamps only, no calendar math") and the HUD clock is integer division. When `{date}`/`{time}` pattern expansion lands (parity row pending), it must use `chrono` |
| Rust #6 contributor credits | n/a yet | no `contributors/people.toml` in this repo |
| Core #8 `main` via PR | n/a now | applies once the repo is a suite repo |
| Core #11 CI calls `storytold/craft-common-actions` | **needs work** | `.github/workflows/rust.yml` is self-contained (gates + AppImage + kittest snapshots on lavapipe). Joining the suite means moving to the shared actions |
| Core #9 stop on licensing outside the allowlist | **blocking** | see §6.1: `GPL-3.0-or-later` is not in the allowlist |

## 4. Differentiation

1. **Rust only, no webview.** A native egui/eframe app with a ~13.7 MB AppImage,
   against ~80–90 MB for the Tauri build of the same product. The editor, the CPU
   compositor and the export path are all Rust — no canvas API, no JS runtime.
2. **Sans-I/O engine with enforced layers.** `snagcraft-engine` never touches a
   file, socket or font path; `cargo xtask layers` proves the layering (23 workspace
   crates, cycle detection, no UI toolkit below L6) instead of merely documenting it.
3. **One registry, four surfaces.** 41 native commands (21 in the web build) with a
   stable id and JSON params drive the UI, the CLI, the TCP control channel and the
   MCP server. Automation is a first-class surface, not a bolted-on CLI.
4. **PhotoCraft-model automation.** Loopback TCP JSON with a 0600 token file,
   auth-first-no-ack, and registry-driven methods — the same protocol shape as
   PhotoCraft, so suite agents can drive SnagCraft the way they already drive
   PhotoCraft. Plus an MCP stdio server (5 tools) for agent frameworks.
5. **Same code in a browser.** The WASM build is not a demo: the full annotation
   toolset, document open/save and PNG export-download were browser-verified.
   Cross-platform reach today without porting native backends.
6. **Testing discipline the suite can trust.** kittest headless snapshots per wave
   (`visual`, `w2_shell`, `w3_shell`, `w4_live`, `parity`, `context_menu`), pixel
   probes on render paths (a snapshot has been wrong before), adversarial param tests,
   392 tests green. CI runs with `UPDATE_SNAPSHOTS=true` so snapshot drift never
   masks a functional failure.
7. **Dual licence.** GPL-3.0-or-later with a commercial option — see §6.1, where this
   cuts both ways.

## 5. What is proven vs pending

**Proven (verifiable artefacts, not self-reports):**

- 392 workspace tests, 0 failures; `cargo xtask layers` legal (23 crates); clippy
  `-D warnings`, fmt clean.
- Live control round-trip on pn53: app `--control` ↔ CLI → `commands.list` = 41,
  `doc.new` 64×48, `ui.screenshot` a valid PNG; wrong token rejected; token file
  `0600`.
- WASM web app browser-verified: boot, full editor render, 14118-byte PNG export.
- 13-tool annotation parity, undo/redo (80), PNG/JPEG/WebP export, native format,
  recording (MP4/GIF, ffprobe-verified offline), scrolling stitch, OCR, timelapse
  frames, watermark.
- AppImage and `.deb` packaging scripts; CI artifacts.

**Pending before any candidacy conversation:**

- KDE live pass for tray/hotkeys/recording (portal consent needs an unlocked
  session) — `docs/gaps.md` §A1.
- The nine reverted parity rows (OCR language/busy UI, timelapse video assembly,
  `Ctrl+.`, shrunk countdown HUD, upload POST flow) — §B4.
- Trim decision (spec lists it, live lacks it) — §A2.
- Web MVP limits (clipboard image write, drag-drop unexercised) — §C.

## 6. Blockers and risks

### 6.1 Licence is the hard blocker

The suite's licence allowlist contains MIT, Apache-2.0, BSD, ISC, OFL, CC and
friends — **not GPL**. Core rule 9 makes licensing outside the allowlist a
stop-and-ask item, and "a repository may not add an identifier here by editing its
own copy". A GPL-licensed app therefore cannot simply be dropped into the suite:
**relicensing to MIT/Apache-2.0 (or getting an explicit maintainer exception) is a
precondition**, and it is rostad's decision (gate A3). The dual licence is a genuine
commercial asset for SnagCraft on its own; for suite candidacy it is a conflict to
resolve, not a selling point.

### 6.2 Platform coverage

An official suite app ships per-platform releases (the launcher downloads the right
asset and verifies `SHA256SUMS.txt`). SnagCraft's platform layer is Linux-only:
ashpd portals, `ksni` tray, PipeWire recording, XDG paths. Capture has an `xcap`
fallback for macOS/Windows, but tray, global hotkeys and recording do not. Official
suite membership would require Windows/macOS backends and signed builds — a porting
project (`docs/gaps.md` §D1).

### 6.3 Repository shape

The repo still contains the Tauri v1.5.x app at the root and the Rust workspace
under `rust/`. A suite reviewer will read that as two products in one repo. Cutover
(gate A4) — moving the workspace to the root and retiring the Tauri tree — is a
prerequisite for a clean candidacy.

### 6.4 Other honest weaknesses

- `snagcraft-vector` is a compiling placeholder, not a vector engine — fine for
  screenshots, but do not present the layer table as "all crates implemented".
- Upload is "open the share page" only; the live app's POST-to-endpoint flow is not
  rebuilt (`docs/gaps.md` §B4).
- No macOS/Windows CI, signing or notarisation.
- The niche numbering (#2148/#2337) is unverified (§2).

## 7. Suggested path (all gated on rostad)

1. **Decide licence** (relicense vs exception) — the one item that gates everything.
2. **Cutover** the Rust build to the repo root; keep the Tauri tree only if it is
   still needed for the release the users have.
3. **KDE live pass** on the W4 AppImage; close the nine reverted rows that matter.
4. **Move fonts** to `craft-fonts` or obtain an exception; adopt
   `craft-common-actions` in CI.
5. Only then: ask the suite maintainers about the ScreenCraft niche (#2148/#2337)
   and the naming question — as a person, through the normal channel, not via an
   agent posting.

Nothing above happens without an explicit go-ahead. Until then this file stays
internal.

## Gates

| Gate | Owner | Status |
|------|-------|--------|
| Relicense GPL-3.0-or-later → MIT/Apache-2.0 | rostad | open |
| Any contact with `storytold/*` (issue, PR, comment, Discord, email) | rostad only | forbidden |
| Cutover from Tauri v1.5.x | rostad | open |
| Naming: SnagCraft vs ScreenCraft | rostad | open |
| KDE live acceptance pass | rostad | open |