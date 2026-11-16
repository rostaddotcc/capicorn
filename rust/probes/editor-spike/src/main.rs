//! W0 editor spike binary. Usage:
//!   editor-spike [image.png]
//!   editor-spike --selftest [--shot-out out.png]

use std::path::PathBuf;

use editor_spike::app::{SpikeApp, apply_theme};

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut path: Option<PathBuf> = None;
    let mut selftest = false;
    let mut shot_out = PathBuf::from("spike-selftest.png");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--selftest" => selftest = true,
            "--shot-out" => {
                if let Some(p) = args.next() {
                    shot_out = PathBuf::from(p);
                }
            }
            "--help" | "-h" => {
                tracing::info!("editor-spike [image.png] [--selftest --shot-out out.png]");
                return Ok(());
            }
            other => path = Some(PathBuf::from(other)),
        }
    }

    let mut app = if selftest {
        SpikeApp::seeded()
    } else {
        match &path {
            Some(p) => {
                let mut a = SpikeApp::new(editor_spike::scene::synthetic_image(1000, 640));
                a.load_path(p);
                a
            }
            None => SpikeApp::new(editor_spike::scene::synthetic_image(1000, 640)),
        }
    };
    if selftest {
        app.selftest = Some(editor_spike::app::SelfTest::new(shot_out));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_title("SnagCraft — Rust W0 editor spike"),
        ..Default::default()
    };
    eframe::run_native(
        "snagcraft-editor-spike",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
