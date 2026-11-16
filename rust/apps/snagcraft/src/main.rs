#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! SnagCraft desktop (Rust edition) — eframe/wgpu shell over the engine.
//! Usage: snagcraft [--capture] [--selftest --shot-out out.png]

use std::path::PathBuf;

use snagcraft_ui_egui::{SnagCraftApp, apply_theme};

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut capture_on_start = false;
    let mut selftest = false;
    let mut shot_out = PathBuf::from("snagcraft-selftest.png");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--capture" => capture_on_start = true,
            "--selftest" => selftest = true,
            "--shot-out" => {
                if let Some(p) = args.next() {
                    shot_out = PathBuf::from(p);
                }
            }
            "--help" | "-h" => {
                tracing::info!("snagcraft [--capture] [--selftest --shot-out out.png]");
                return Ok(());
            }
            _ => {}
        }
    }

    let mut app = if selftest {
        SnagCraftApp::seeded()
    } else {
        SnagCraftApp::new()
    };
    if selftest {
        app.selftest = Some(snagcraft_ui_egui::SelfTest::new(shot_out));
    } else if capture_on_start {
        app.start_capture();
    }

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
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
