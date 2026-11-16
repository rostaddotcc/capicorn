# W3 — Shell Parity (tray, hotkeys, widget, settings, close-guard, integration)

> **For Hermes:** W3a = platform crate + settings/engine tracks (dispatched);
> W3b = UI (settings panel, themes, close-guard, widget viewport) + app wiring
> (parent, after W3a lands). Parity source: rust-rebuild-parity.md §5/§11/§13.

**Goal:** system-app behaviour — tray click-to-restore, GlobalShortcuts portal
hotkeys (Print / Shift+Print), floating widget (58×74 tab ↔ 292×50 pill),
settings panel + persistence, light/dark themes, close-guard decision tree,
single-instance argv forwarding, autostart, file associations, --capture/--region CLI.

**Verified inputs (wake #9):** live tray.rs (ksni backend load-bearing: left=restore,
middle=widget, right=menu Show/Capture/Widget/Quit; TRAY_READY gates hide-to-tray),
hotkeys.rs (GlobalShortcuts portal, one BindShortcuts per session, xdg_trigger
conversion, defaults Print/SHIFT+Print from settings.ts:66-67), close-guard tree
(windows.rs:96-140: force-quit→exit; dirty→save modal; widget visible→hide main;
minimize-to-tray+tray present→hide; else exit), widget sizes (windows.rs:205-213).

## W3a LOCKED CONTRACTS

```rust
// NEW crate snagcraft-platform (L6, toolkit-free, channel-based)
pub enum TrayEvent { Show, Capture, ToggleWidget, Quit }
pub fn spawn_tray(icon_png: &[u8], events: std::sync::mpsc::Sender<TrayEvent>)
    -> Result<TrayHandle, PlatformError>;
// tray-icon 0.25 default-features=false features=["ksni"]; own thread;
// left/double → Show, middle → ToggleWidget, menu: Show SnagCraft / Capture /
// Floating widget / ─ / Quit. TrayHandle::is_available() (TRAY_READY pattern).
pub struct HotkeyDef { pub id: String, pub trigger: String }
pub enum HotkeyEvent { Activated { id: String }, Bound { granted: Vec<(String,String)> }, Failed(String) }
pub fn spawn_hotkeys(defs: Vec<HotkeyDef>, events: Sender<HotkeyEvent>)
    -> Result<HotkeyHandle, PlatformError>;
// ashpd 0.14 GlobalShortcuts (feature global_shortcuts): create_session →
// bind_shortcuts (MAY BLOCK for user consent → own thread + tokio runtime);
// one BindShortcuts per session; Activated events carry the def id.
pub fn xdg_trigger(input: &str) -> String; // port from live hotkeys.rs:156
pub fn set_autostart(enabled: bool) -> Result<(), PlatformError>; // XDG autostart .desktop
pub fn autostart_enabled() -> bool;
pub enum Instance { Primary { rx: Receiver<Vec<String>> }, Secondary }
pub fn acquire_or_forward(args: &[String]) -> Result<Instance, PlatformError>;
// local socket $XDG_RUNTIME_DIR/snagcraft.sock (unix): Primary binds + listens
// (thread), Secondary connects + sends argv JSON + exits. Stale socket → reclaim.

// io::settings (L4) — data + persistence
#[derive(Clone,Debug,Serialize,Deserialize)] #[serde(default)]
pub struct Settings {
  minimize_to_tray: bool=false, start_at_login: bool=false, show_widget: bool=true,
  autosave_captures: bool=true, capture_delay_secs: u32=0,
  theme: Theme=Light, default_format: String="png", default_quality: u8=90,
  save_dir: Option<String>=None, filename_pattern: String="SnagCraft-{date}-{time}",
  hotkey_full: String="Print", hotkey_region: String="SHIFT+Print",
}
pub enum Theme { Light, Dark }
pub fn path() -> PathBuf; // $XDG_CONFIG_HOME/snagcraft/settings.json (fallback ~/.config)
pub fn load() -> Settings; // missing/corrupt → defaults, never panic
pub fn save(s: &Settings) -> Result<(), IoError>; // atomic: temp+rename

// engine (L5): Session.settings: Settings (loaded via io::settings::load in new());
// commands settings.get {} → full JSON; settings.set {settings:{...}} → replace
// (serde validates), return applied JSON. Persistence is the APP's job after set
// (engine stays sans-I/O). Session::settings()/settings_mut().
```

## Tracks
- **T1 — platform crate:** tray/hotkeys/autostart/single-instance per contracts,
  port patterns from live tray.rs/hotkeys.rs (strip tauri::), offline tests
  (xdg_trigger cases, autostart with XDG_CONFIG_HOME override, instance
  primary/secondary over a temp socket, tray/hotkey spawn graceful-headless).
- **T2 — io::settings + engine commands:** per contracts, round-trip/corrupt/
  atomic-save tests + settings.get/set engine tests incl. adversarial params.
- **Parent W3b (after landing):** ui-egui settings panel (General/Capture/Export/
  Hotkeys/Save/Theme sections), dark theme (#3d7ec2 accent), close-guard modal
  (plain copy + picture icon, Save/Discard/Cancel), dirty tracking, widget
  immediate viewport (transparent, two states, drag, Capture/Rec buttons — Rec
  disabled until W4), apps/snagcraft wiring (tray/hotkeys channels → commands,
  --capture/--region/path CLI + launch_request poll, single-instance, autostart
  toggle, file-association .desktop with %F + MIME), gates, AppImage, parity ticks.

## Gates
1. fmt/clippy/tests/layers green (platform registered L6; no egui dep in platform).
2. Tray + hotkeys: graceful on headless/locked host (Err or no-op, never panic);
   live validation = rostad's KDE (tray click, Print/Shift+Print consent dialog).
3. Settings persist across restarts (test with temp XDG_CONFIG_HOME).
4. Single instance: second launch forwards argv (integration test over temp socket).
5. Widget: transparent, folds/expands, drags, restores main (kittest + KDE pass).
6. Close-guard: dirty → modal; clean+tray → hide; force-quit bypasses.
7. AppImage launch + rostad KDE pass.

## W3b design (parent, after W3a lands) — mechanisms VERIFIED from vendored 0.37 source

- **Widget viewport:** `ctx.show_viewport_immediate(ViewportId::from_hash("widget"),
  ViewportBuilder::default().with_transparent(true).with_decorations(false)
  .with_inner_size([304,60]).with_resizable(false), |ui, _class| widget_ui(ui, app))`
  (context.rs:4662). Global `App::clear_color → [0,0,0,0]`; main window stays opaque
  because its panels paint full-size fills. Widget states: folded tab 58×74 ↔
  expanded pill 292×50 (window 304×60) via `ViewportCommand::InnerSize` +
  `OuterPosition` (park top-right from `i.viewport().monitor_size`). Drag:
  `StartDrag` on background drag (W0 probe proved it). Buttons: Full (capture.full),
  Region (capture.full + tool=Crop), Rec (DISABLED, tooltip "Recording lands in W4"),
  fold/unfold, close (hide widget + settings.show_widget=false). Enter/Space on the
  tab expands (a11y). V shortcut = record (W4) — inert until then.
- **Close guard (eframe 0.37 exact flow):** X-click pushes `ViewportEvent::Close`
  into `ctx.input(|i| i.viewport().events)` (viewport_info.rs:35, helper :115).
  App decision tree (live windows.rs:96-140 parity): force_quit → allow; dirty →
  show save modal + `ViewportCommand::CancelClose`; widget visible → hide main
  (`ViewportCommand::Visible(false)`) + CancelClose; minimize_to_tray && tray
  available → Visible(false) + CancelClose; else allow (no CancelClose → eframe
  closes, epi_integration.rs:428-435). Modal: plain copy "You have unsaved
  annotations" + picture icon, Save / Discard / Cancel (skill rule). Dirty flag
  app-level: set on successful edit.* dispatch, cleared on save/export/open/new/
  capture. Tray Quit sets force_quit.
- **Tray/hotkeys wiring (apps/snagcraft):** spawn_tray(icon_bytes, tx) +
  spawn_hotkeys([capture-full: settings.hotkey_full, capture-region:
  settings.hotkey_region], tx) at startup; drain channels each frame in app_ui →
  Show → Visible(true)+Focus; Capture → capture flow; ToggleWidget → widget
  toggle; Quit → force_quit+Close; HotkeyEvent::Activated{id} → capture full/region.
  Icon bytes: include_bytes! from packaging/snagcraft.png (256px — ksni scales).
  Headless/locked: spawn errors → status note, app continues (never panic).
- **Settings panel:** egui::Window "Settings" (toolbar ⚙ button + Ctrl+,): sections
  General (minimize-to-tray, start-at-login → platform::set_autostart, show-widget,
  autosave, capture delay), Export (default format combo, quality slider), Save
  location (dir picker via rfd, filename pattern), Hotkeys (two trigger text fields
  + "Rebind" → respawn hotkeys), Theme (Light/Dark radio). Every change →
  settings.set dispatch + io::settings::save + apply (theme visuals, autostart,
  widget visibility). Dark theme: Visuals::dark() + accent #3d7ec2 + panel fills
  (#1e2126-ish), same slider trailing fill.
- **Single instance + CLI:** acquire_or_forward(argv) at main() start; Secondary →
  exit 0; Primary → drain rx each frame: argv with --capture/--region → capture
  flow; image path → doc.open. First-launch --capture honored directly.
- **File associations:** packaging/snagcraft.desktop gains MimeType=image/png;
  image/jpeg;image/webp; + Exec=snagcraft %F (deb/AppImage desktop file).
- **Gates:** W3 gates per plan §Gates; kittest: settings window opens, theme swap
  changes pixels, close-guard modal on dirty (inject ViewportEvent::Close via
  input_mut), widget viewport renders in kittest (embedded mode note: kittest may
  embed viewports — verify snapshot shows pill). Live tray/hotkeys/widget-drag =
  rostad KDE pass.

## Appendix — measured reality
(append rows as assumptions break)