# Attribution — SnagCraft Rust rebuild (`rust/`)

## Assets

| Path | Title | Author | Source | License |
|---|---|---|---|---|
| `packaging/snagcraft.png` | SnagCraft app icon (256px) | rostad | Original artwork, resized from repo-root `app-icon.png` (1024px original) | GPL-3.0-or-later + commercial (project dual license) |
| `crates/ui-egui/tests/snapshots/*.png` | Headless test snapshots | Hermes (generated) | Rendered by egui_kittest from synthetic scenes | GPL-3.0-or-later + commercial |

## Fonts

No font files are committed. Text raster reuses egui's bundled **Ubuntu-Light**
face at runtime (extracted from `egui::FontDefinitions::default()` in the app
layer; the engine receives bytes via `Session::set_font`). Ubuntu-Light is
© Canonical Ltd, licensed under the Ubuntu Font Licence 1.0 — see the
`epaint_default_fonts` crate for the upstream license text. The suite allowlist
includes `Ubuntu-font-1.0`.

## Dependencies (SPDX allowlist check, W1)

All workspace dependencies resolve to allowlisted licenses:
egui/eframe/epaint (MIT OR Apache-2.0), image (MIT OR Apache-2.0),
ab_glyph (Apache-2.0), rfd (MIT), ashpd (MIT), xcap (MIT OR Apache-2.0),
tokio (MIT), serde/serde_json (MIT OR Apache-2.0), thiserror (MIT OR Apache-2.0),
base64 (MIT OR Apache-2.0), clap (MIT OR Apache-2.0),
cargo_metadata (MIT OR Apache-2.0), tracing/tracing-subscriber (MIT),
egui_kittest (MIT OR Apache-2.0). No GPL dependencies. Verify with
`cargo deny` / `cargo about` before each release (W6 packaging wave).