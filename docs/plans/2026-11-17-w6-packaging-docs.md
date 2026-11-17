# W6 — Packaging, docs, suite-candidacy dossier

> **For Hermes:** W6a = Linux packaging + CI artifacts; W6b = docs + dossier.
> GATES (never autonomous): relicense (GPL→MIT/Apache), ANY external post
> (issues/PRs/releases/Discord), cutover. W6 produces artifacts + docs only —
> NO GitHub release publish, NO external posting.

**Goal:** ship-ready Linux packaging (AppImage + .deb) + CI artifact builds,
honest docs (README/gaps/parity), internal suite-candidacy dossier.

## Scope reality (measured)
- Platform layer (capture/tray/hotkeys) is LINUX-ONLY (ashpd/ksni/PipeWire).
  Windows NSIS/MSI + macOS dmg from the spec are NOT feasible without porting
  the platform backends — documented as FUTURE in gaps.md, not built here.
- Cross-platform reach IS achieved via the WASM web build (any browser).
- AppImage already works (packaging/appimage.sh, 13.7 MB, launch-tested).
- dpkg-deb present → manual .deb script (no cargo-deb dependency).

## W6a — packaging (owns rust/packaging/, .github/workflows/rust-release.yml)
1. packaging/deb.sh: build a .deb from the release binary via dpkg-deb —
   DEBIAN/control (Package: snagcraft, Version from Cargo.toml, Architecture
   amd64, Depends: libwayland-client0, libxkbcommon0, libgl1, libegl1,
   libfontconfig1, libfreetype6, libdbus-1-3, libpipewire-0.3-0, ffmpeg,
   tesseract-ocr [Recommends]), usr/bin/snagcraft, usr/share/applications/
   snagcraft.desktop, usr/share/icons/hicolor/256x256/apps/snagcraft.png,
   usr/share/doc/snagcraft/{copyright,changelog}. dpkg-deb --build --root-owner-group.
2. AppImage polish: desktop-file-validate the .desktop (already passes);
   ensure icon/MIME correct; appimage.sh unchanged unless a real fix needed.
3. .github/workflows/rust-release.yml: on workflow_dispatch + tag push v*
   (tag push builds artifacts but DOES NOT publish a release — publishing is
   gated; use upload-artifact only, NO softprops/action-gh-release). Jobs:
   linux (build release binary → appimage.sh → deb.sh → upload AppImage + .deb),
   web (trunk build --release → upload dist). Reuse the rust.yml toolchain
   setup (rust-toolchain stable, Swatinem/rust-cache, Node24 actions).
   NO release publish step. Document the gate in a comment.
4. Verify: deb.sh produces a .deb that `dpkg-deb -I` + `dpkg-deb -c` list
   correctly; lintian if available (advisory). AppImage launch-test (exit 124
   on locked GNOME = alive). CI workflow YAML valid (actionlint if available).

## W6b — docs + dossier (owns rust/README.md, docs/gaps.md, docs/suite-candidacy.md, docs/plans/rust-rebuild-parity.md)
1. rust/README.md rewrite: what SnagCraft-Rust is, architecture (L0–L6 crate
   layers, sans-I/O engine, command registry drives UI/CLI/control/MCP),
   build (cargo build --release), run (native AppImage / cargo run), CLI
   (capture/convert/commands/run/mcp/control), control channel (TCP JSON,
   token-file 0600, auth-first no-ack, methods), MCP (5 tools), web
   (trunk serve/build), licence (GPL-3.0-or-later + commercial dual),
   gates (relicense/cutover/posting are rostad's). Ground every claim in the
   actual code/status log — no invented features.
2. docs/gaps.md: consolidate ALL logged gaps honestly — W3/W4 KDE live pass
   pending rostad; W4 trim (spec lists, live lacks — rostad decides), audio
   mic+monitor mixing, Recent-rail video thumbnails, copy-diagnostics button,
   the 9 reverted parity rows (OCR lang/busy modal, timelapse ffmpeg video
   assembly + Ctrl+. + shrunk countdown HUD, upload POST flow); W5 web image
   clipboard status-only, drag-drop not browser-exercised; Windows/macOS
   native FUTURE (platform backends needed; web build is cross-platform now).
3. docs/suite-candidacy.md: INTERNAL dossier — ScreenCraft niche (#2148/#2337
   in the ArtCraft suite), differentiation (Rust-only no webview, sans-I/O
   engine, L0–L6 layers, MCP+TCP automation, WASM web, dual licence),
   candidacy rationale. NO external posting (gate). Reference the suite
   house rules (tmp/research/suite/).
4. parity checklist: finalize docs/plans/rust-rebuild-parity.md — ensure every
   ticked row has evidence, pending rows honest. Do NOT overclaim.

## Appendix — measured reality
(append rows as assumptions break)