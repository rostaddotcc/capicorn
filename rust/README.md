# SnagCraft (Rust edition)

The native-Rust rebuild of SnagCraft: **capture → annotate → share**, with screen
recording, scrolling capture, OCR and timelapse on Linux (Wayland-first). It is a
clean-room, FOSS alternative to Snagit — no subscription, no webview.

This directory (`rust/`) is the rebuild workspace. The original Tauri v1.5.x app
lives in the repository root (`src-tauri/`, `src/`) and is still the shipped
build; switching to this one is a **gate owned by rostad** (see
[Gates](#gates)). Parity tracking against v1.5.3 lives in
[`docs/plans/rust-rebuild-parity.md`](../docs/plans/rust-rebuild-parity.md),
what is still missing in [`docs/gaps.md`](../docs/gaps.md).

Status: W0–W5 complete on Linux, W6 lands packaging and these docs — native egui/eframe
app (AppImage + `.deb`), headless CLI, TCP JSON control channel, MCP server, and a WASM
web build of the same editor.
392 workspace tests green at W5 close; the interactive portal legs (recording,
global hotkeys, tray) still need rostad's unlocked KDE session — see
`docs/gaps.md` §A1.

---

## Architecture

Three ideas carry the whole design:

1. **A sans-I/O engine.** `snagcraft-engine` owns the document, history and every
   user-visible action. It never opens a socket, dialog or font file: paths come in
   as params, bytes come in as surfaces, and fonts are compiled into the binary.
   The same engine therefore runs in a native window, a terminal, an automation
   server and a browser tab.
2. **One command registry, four surfaces.** `snagcraft_engine::registry()` is the
   single list of commands (stable id + JSON params). The egui UI, the CLI, the TCP
   control channel and the MCP server all dispatch through it. Adding a command to
   the registry makes it available to all four with no extra glue.
3. **Layers enforced by a build task.** `cargo xtask layers` reads the workspace
   metadata and rejects any dependency edge that points upward; nothing below L6 may
   link a UI toolkit (`egui`, `eframe`, `winit`, `rfd`, `tray-icon`, …).

### Layers (from `rust/xtask/src/layers.rs`)

| Layer | Crates | Responsibility |
|-------|--------|----------------|
| L0 | `geom`, `color`, `raster` | points/rects, RGBA blending, the surface type |
| L1 | `doc` | document model: base image + 10 annotation object kinds, serde |
| L2 | `ops`, `algo`, `text`, `vector` | bounded history (80 states), blur/pixelate/blackout/flood-fill/resample, bundled-font resolution; `vector` is currently a compiling placeholder |
| L3 | `compose`, `format` | CPU compositor (styled objects → pixels), native `.snagcraft` format |
| L4 | `io`, `capture`, `recording` | codecs + clipboard + settings + OCR + upload, portal screenshot + scrolling/interval capture, ScreenCast → PipeWire → ffmpeg |
| L5 | `engine` | `Session` + command registry |
| L6 | `platform`, `ui-egui`, `automation` | tray/hotkeys/autostart/single-instance (toolkit-free), egui shell, TCP control + MCP |
| L7 | `apps/snagcraft`, `apps/snagcraft-cli`, `apps/snagcraft-web`, `xtask` | binaries and build tooling |

`probes/` (the W0 spike and the Wayland viewport probe) are registered at L6 as
egui shells and are not part of the product build.

Same-layer edges are allowed (with cycle detection); upward edges are not. The
workspace forbids `unsafe`, and denies `unwrap`/`expect`/`panic`/`todo`/`unreachable`
in non-test code at the crate roots.

### The same Rust in a browser

`io` and `engine` have a `native` / `web` feature split. The web build drops the
platform-bound code (arboard clipboard, portal capture, tesseract, PipeWire) and
keeps the document, compositor, tools and export. `cargo xtask wasm` compiles the
web crate set for `wasm32-unknown-unknown` to prove the split still holds. Of the
43 native commands, **21** are registered in the web build.

---

## Build

Requirements: stable Rust (workspace `rust-version` 1.95, edition 2024). Linux needs
`libpipewire-0.3`/`libspa-0.2` dev headers, `clang`, `ffmpeg` and `tesseract-ocr`
at runtime for recording and OCR.

```sh
cd rust

# desktop app + headless CLI
cargo build --release -p snagcraft -p snagcraft-cli

# gates: fmt + clippy (-D warnings) + tests + layer check
cargo xtask ci

# individual gates
cargo xtask layers     # L0–L6 edges legal, no UI below L6
cargo xtask wasm       # web crate set compiles for wasm32

# WASM web app (needs trunk + wasm-bindgen-cli)
cargo xtask wasm
cd apps/snagcraft-web && trunk build --release
```

`cargo xtask` is an alias (`rust/.cargo/config.toml`) for `run -p xtask --`.

### Packaging

```sh
# AppImage (linuxdeploy + linuxdeploy-plugin-appimage on PATH)
rust/packaging/appimage.sh rust/target/release/snagcraft dist/

# .deb (dpkg-deb; no cargo-deb)
rust/packaging/deb.sh rust/target/release/snagcraft dist/
```

`.github/workflows/rust-release.yml` builds and uploads the AppImage, `.deb` and web
`dist` as workflow artifacts on `workflow_dispatch` and `v*` tags. It deliberately
does **not** publish a release — that is a gate.

---

## Run

```sh
# AppImage
./SnagCraft-x86_64.AppImage

# straight from the workspace
cargo run -p snagcraft                      # editor window
cargo run -p snagcraft -- --capture         # capture, then open the editor
cargo run -p snagcraft -- --region          # region capture
cargo run -p snagcraft -- shot.png          # open an image (file associations pass this)
cargo run -p snagcraft -- --control 0       # serve the control channel, ephemeral port
cargo run -p snagcraft -- --selftest --shot-out /tmp/shot.png   # headless self-test
```

A second launch forwards its argv to the running instance over a unix socket and
exits, so captures always land in the window that is already open. `--selftest`
needs an **unlocked** session: on a locked GNOME seat no frame callbacks arrive and
it will simply sit there (headless visual gating is done with kittest snapshots
instead).

---

## CLI

`snagcraft-cli` drives every registry command without a display. Results go to
stdout as JSON; logs go to stderr.

```sh
snagcraft-cli capture out.png [--quality 90]     # full-screen capture → file
snagcraft-cli convert in.png out.jpg [--format jpeg] [--quality 90]
snagcraft-cli commands                           # the registry, with params docs
snagcraft-cli run doc.new --params '{"w":640,"h":480}'
snagcraft-cli run edit.add --input in.png --params '{"object":{...}}' --output out.png
snagcraft-cli mcp                                # MCP server on stdio
snagcraft-cli control --port 41234 --method commands.list
```

`run` takes any command id from `commands`, optional `--input` (open an image
first) and optional `--output` (export afterwards), so an entire edit is scriptable.

---

## Control channel

`snagcraft --control <port>` (0 = ephemeral, the port is printed) serves
line-delimited JSON over **loopback TCP only** — the PhotoCraft protocol.

- **Token:** 64 hex chars (256 bits), written to `control.token` beside
  `settings.json` (mode `0600`), overridable with `--control-token-file`.
- **Auth first, no acknowledgement:** the client sends
  `{"id":1,"method":"auth","params":{"token":"…"}}`; on success the server sends
  *nothing*, so the next frame the client reads is the answer to its own request. A
  bad token yields exactly one error frame.
- **Methods:** `commands.list`, `command.run` (`{id, params}`), `session.info`,
  `doc.inspect`, `ui.screenshot` (PNG, base64).
- **Hostile input:** frames are capped at 1 MiB, idle connections time out after
  60 s, tokens are validated as 64 hex chars on both write and read, and params go
  through the engine's own validation.

```sh
snagcraft --control 0 &          # prints: control: 127.0.0.1:PORT
snagcraft-cli control --port PORT --method command.run \
  --params '{"id":"doc.new","params":{"w":320,"h":200}}'
```

## MCP

`snagcraft-cli mcp` (or `snagcraft_automation::serve_mcp_stdio`) exposes the engine
over MCP on stdio (rmcp 3.10), with five tools:

| Tool | Purpose |
|------|---------|
| `command_list` | every command id + label + params doc |
| `command_run` | run any command by id with a params object |
| `session_info` | has_doc, size, object count, undo/redo availability |
| `doc_inspect` | size, next id, all annotation objects |
| `doc_export` | export to `{path, format, quality}` |

`command_run` is the reason the server stays small: new registry commands need no
new MCP code.

## Web build

`apps/snagcraft-web` is an eframe `WebRunner` compiled to WASM — the same editor
core in a browser tab, not a separate implementation.

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk wasm-bindgen-cli
cd rust/apps/snagcraft-web
trunk serve          # http://127.0.0.1:8080
trunk build --release
```

The web build opens/saves images, runs the full annotation toolset and exports PNG
by download. Desktop-only surfaces (capture, OCR, recording, tray, widget, share
page) are hidden or reported as desktop-only. Limits: image copy is status-only
(no `ClipboardItem` write) and drag-drop open is code-complete but not
browser-exercised — see `docs/gaps.md` §C.

## Testing

```sh
cargo test --workspace
cargo xtask ci
```

- **392 tests** at W5 close, 0 failures. A handful are `#[ignore]`d because they
  need an unlocked session, a live portal or a real DBus service.
- UI shells are gated by **kittest** headless snapshots (`ui-egui/tests/`: `visual`,
  `w2_shell`, `w3_shell`, `w4_live`, `parity`, `context_menu`). Snapshots are
  re-blessed deliberately, and CI runs with `UPDATE_SNAPSHOTS=true` so the runner
  blesses into a scratch dir instead of failing on font/GPU drift — the functional
  assertions still gate.
- Render paths get pixel probes (text actually drawn, blur actually smoothed); a
  snapshot alone has been wrong before (see the status log's measured-reality
  appendix).

## Licence

**GPL-3.0-or-later**, with a commercial licence available. Using SnagCraft as a
GPL program is free; embedding it in a closed-source product needs a commercial
licence from rostad. See `LICENSE` at the repository root and
`rust/ATTRIBUTION.md` for assets and fonts.

## Gates

These are rostad's decisions and are never taken autonomously:

| Gate | State |
|------|-------|
| Relicense GPL-3.0-or-later → MIT/Apache-2.0 | open |
| Cutover: make the Rust build the shipped app | open |
| Any external posting (issues/PRs/comments, Discord, publishing a release) | forbidden without rostad's go-ahead |
| KDE live acceptance pass (tray/hotkeys/recording) | open — see `docs/gaps.md` §A1 |
| Recording trim: build it or drop it from the spec | open — see `docs/gaps.md` §A2 |

## Where things live

| Path | What |
|------|------|
| `crates/` | L0–L6 libraries |
| `apps/snagcraft` | desktop app (eframe) |
| `apps/snagcraft-cli` | headless CLI |
| `apps/snagcraft-web` | trunk/WASM web app |
| `xtask/` | `layers`, `wasm`, `ci` gates |
| `packaging/` | AppImage + `.deb` scripts, desktop entry, icon |
| `probes/` | W0 spike + Wayland viewport probe (kept for reference) |
| `assets/fonts/` | 14 bundled OFL faces |
| `../docs/plans/rust-rebuild-status.md` | wake log + measured reality |
| `../docs/plans/rust-rebuild-parity.md` | v1.5.3 parity checklist |
| `../docs/gaps.md` | consolidated gap register |
| `../docs/suite-candidacy.md` | internal ArtCraft suite dossier |