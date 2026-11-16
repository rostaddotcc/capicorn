#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! SnagCraft desktop (Rust edition) — eframe/wgpu shell over the engine.
//!
//! W3b: single instance (`acquire_or_forward`), tray + global hotkeys with
//! graceful headless degradation, `--capture` / `--region` / image-path argv,
//! and settings persistence (`io::settings::save`) drained after each frame.
//!
//! Usage: snagcraft [--capture] [--region] [image] [--selftest --shot-out out.png]

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
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--selftest" => selftest = true,
            "--shot-out" => {
                if let Some(p) = args.next() {
                    shot_out = PathBuf::from(p);
                }
            }
            "--help" | "-h" => {
                tracing::info!(
                    "snagcraft [--capture] [--region] [image] [--selftest --shot-out out.png]"
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
    eframe::run_native(
        "snagcraft",
        options,
        Box::new(|cc| {
            snagcraft_ui_egui::apply_theme(&cc.egui_ctx, app.settings.theme);
            Ok(Box::new(Shell { app }))
        }),
    )
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
    let settings = app.session.settings().clone();
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
