#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
// Tests may unwrap: a broken fixture should fail loudly, not be swallowed.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
//! SnagCraft desktop (Rust edition) — eframe/wgpu shell over the engine.
//!
//! W3b: single instance (`acquire_or_forward`), tray + global hotkeys with
//! graceful headless degradation, `--capture` / `--region` / image-path argv,
//! and settings persistence (`io::settings::save`) drained after each frame.
//!
//! W5b: `--control <port>` serves the automation control channel (W5a) on the
//! *same* session the UI edits, so an agent drives the document the user sees.
//! The token lives in `control.token` beside `settings.json` (0600) and is
//! reused across launches; absent flags mean no control server at all.
//!
//! Usage: snagcraft [--capture] [--region] [image] [--selftest --shot-out out.png]
//!                  [--control <port>] [--control-token-file <path>]

use std::path::PathBuf;
use std::sync::mpsc;

use snagcraft_platform::{HotkeyDef, HotkeyEvent, TrayEvent};
use snagcraft_ui_egui::SnagCraftApp;

/// Tray icon: the 32×32 RGBA PNG AppIndicator expects (ksni scales it).
const TRAY_ICON: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/tray-32.png"
));

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // W43: capture the D-Bus session environment + portal reachability into the
    // portal log at startup, so a wrong/unreachable bus is diagnosable.
    snagcraft_capture::diagnose_portal_at_startup();

    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Single instance first: a second launch only forwards its argv and exits.
    let instance = match snagcraft_platform::acquire_or_forward(&argv) {
        Ok(snagcraft_platform::Instance::Secondary) => {
            tracing::info!("another SnagCraft instance owns the session — argv forwarded");
            return Ok(());
        }
        Ok(snagcraft_platform::Instance::Primary { rx }) => Some(rx),
        Err(err) => {
            // No socket is not fatal: run without single-instance forwarding.
            tracing::warn!("single instance unavailable: {err}");
            None
        }
    };

    let mut selftest = false;
    let mut shot_out = PathBuf::from("snagcraft-selftest.png");
    let mut launch_args: Vec<String> = Vec::new();
    let mut control_port: Option<u16> = None;
    let mut control_token_file = snagcraft_io::settings::path().with_file_name("control.token");
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--selftest" => selftest = true,
            "--shot-out" => {
                if let Some(p) = args.next() {
                    shot_out = PathBuf::from(p);
                }
            }
            "--control" => match args.next().and_then(|p| p.parse::<u16>().ok()) {
                Some(port) => control_port = Some(port),
                None => return usage_error("--control needs a TCP port (0 = ephemeral)"),
            },
            "--control-token-file" => match args.next() {
                Some(path) => control_token_file = PathBuf::from(path),
                None => return usage_error("--control-token-file needs a path"),
            },
            "--help" | "-h" => {
                tracing::info!(
                    "snagcraft [--capture] [--region] [image] [--selftest --shot-out out.png] \
                     [--control <port>] [--control-token-file <path>]"
                );
                return Ok(());
            }
            other => launch_args.push(other.to_string()),
        }
    }

    let mut app = if selftest {
        SnagCraftApp::seeded()
    } else {
        SnagCraftApp::new()
    };
    if selftest {
        app.selftest = Some(snagcraft_ui_egui::SelfTest::new(shot_out));
    }
    // First-launch flags are honoured exactly like a forwarded second launch.
    if !launch_args.is_empty() {
        app.handle_launch_args(&launch_args);
    }
    if let Some(rx) = instance {
        app.attach_instance(rx);
    }

    // ---- control channel (W5b) ---------------------------------------------
    // Spawned before eframe starts so an agent never races the first frame, and
    // torn down after the UI loop returns (Drop would do it too — be explicit).
    let control = match control_port {
        Some(port) => match start_control(&app, port, &control_token_file) {
            Ok(handle) => Some(handle),
            Err(err) => {
                eprintln!("control: {err}");
                std::process::exit(1);
            }
        },
        None => None,
    };

    // ---- tray --------------------------------------------------------------
    let (tray_tx, tray_rx) = mpsc::channel::<TrayEvent>();
    match snagcraft_platform::spawn_tray(TRAY_ICON, tray_tx) {
        Ok(handle) => app.attach_tray(handle, tray_rx),
        Err(err) => {
            tracing::warn!("tray unavailable: {err}");
            app.tray_failed(&err);
        }
    }

    // ---- global hotkeys ----------------------------------------------------
    let settings = snagcraft_io::settings::load();
    let (hotkey_tx, hotkey_rx) = mpsc::channel::<HotkeyEvent>();
    let defs = vec![
        HotkeyDef::new("capture-full", settings.hotkey_full.clone()),
        HotkeyDef::new("capture-region", settings.hotkey_region.clone()),
    ];
    let hotkey_handle = snagcraft_platform::spawn_hotkeys(defs, hotkey_tx.clone());
    if let Err(err) = &hotkey_handle {
        tracing::warn!("global hotkeys unavailable: {err}");
    }
    app.attach_hotkeys(hotkey_tx, hotkey_rx, hotkey_handle);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_title("SnagCraft"),
        ..Default::default()
    };
    let result = eframe::run_native(
        "snagcraft",
        options,
        Box::new(|cc| {
            snagcraft_ui_egui::apply_theme(&cc.egui_ctx, app.settings.theme);
            Ok(Box::new(Shell { app }))
        }),
    );
    if let Some(handle) = &control {
        handle.shutdown();
    }
    result
}

/// Bad CLI usage: say what was wrong on stderr and stop with a non-zero code.
fn usage_error(message: &str) -> eframe::Result<()> {
    eprintln!("snagcraft: {message}");
    std::process::exit(2);
}

/// Token for the control channel: reuse the file when present (so a client's
/// stored token keeps working across launches), otherwise mint one and write it
/// `0600`.
fn control_token(path: &std::path::Path) -> Result<String, String> {
    if path.exists() {
        return snagcraft_automation::read_token_file(path).map_err(|e| e.to_string());
    }
    let token = snagcraft_automation::generate_token();
    snagcraft_automation::write_token_file(path, &token).map_err(|e| e.to_string())?;
    Ok(token)
}

/// Bind the loopback control channel to the app's own session and report where
/// it listens. The one line on stderr is the machine-readable handshake.
fn start_control(
    app: &SnagCraftApp,
    port: u16,
    token_file: &std::path::Path,
) -> Result<snagcraft_automation::ControlHandle, String> {
    let token = control_token(token_file)?;
    let handle = snagcraft_automation::spawn_control(app.session_arc(), port, token)
        .map_err(|e| e.to_string())?;
    let addr = handle.local_addr();
    eprintln!(
        "control: listening on {addr} (token file {})",
        token_file.display()
    );
    tracing::info!(%addr, token_file = %token_file.display(), "control channel ready");
    Ok(handle)
}

/// eframe shell wrapper: the one job the UI crate cannot do itself is
/// persisting settings (`io::settings::save` — it does not link io).
struct Shell {
    app: SnagCraftApp,
}

/// The one job the UI crate cannot do itself: write the settings file.
fn persist_settings(app: &mut SnagCraftApp) {
    if !app.take_settings_dirty() {
        return;
    }
    let settings = app.sess().settings().clone();
    if let Err(err) = snagcraft_io::settings::save(&settings) {
        tracing::warn!("settings save failed: {err}");
        app.status = format!("Could not save settings: {err}");
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        snagcraft_ui_egui::app_ui(ui, &mut self.app);
        persist_settings(&mut self.app);
    }

    /// Hidden window (tray/widget): eframe skips `ui`, so keep the shell's
    /// channels pumped and flush any pending settings write.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        snagcraft_ui_egui::SnagCraftApp::logic(&mut self.app, ctx);
        persist_settings(&mut self.app);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        eframe::App::clear_color(&self.app, visuals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token file in a per-test scratch directory.
    fn token_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snagcraft-w5b-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("control.token")
    }

    #[test]
    fn token_is_minted_once_then_reused() {
        let path = token_path("reuse");
        let first = control_token(&path).expect("mint a token");
        assert!(snagcraft_automation::is_valid_token(&first));
        // The 0600 privacy guarantee is a Unix mode-bit concept; on Windows the
        // token lives in user-scoped app data (ACL-protected) instead.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("stat token")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "token file must be private");
        }
        // A second launch keeps the token: a client's stored copy stays valid.
        assert_eq!(control_token(&path).expect("reuse the token"), first);
    }

    #[test]
    fn corrupt_token_file_fails_loudly() {
        let path = token_path("corrupt");
        std::fs::write(&path, "not-a-token").expect("write fixture");
        assert!(control_token(&path).is_err(), "must not silently rotate");
    }

    #[test]
    fn start_control_serves_the_app_session() {
        let app = SnagCraftApp::new();
        let path = token_path("live");
        let handle = start_control(&app, 0, &path).expect("start the control channel");
        assert!(handle.local_addr().ip().is_loopback(), "loopback only");
        assert!(path.exists(), "the token file is written for clients");
        handle.shutdown();
    }
}
