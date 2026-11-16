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

## Appendix — measured reality
(append rows as assumptions break)