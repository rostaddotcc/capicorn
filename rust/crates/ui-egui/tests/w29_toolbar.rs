//! W29 design gates: the toolbar must show ALL 13 tools at common desktop
//! widths (the old 32px buttons + Open/Export/Resize row buttons pushed the
//! last tools into an invisible horizontal scroll), and the custom-color
//! slot must match the swatch footprint.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, Tool, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w29-toolbar-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn harness(dir: &std::path::Path, width: f32) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    Harness::builder()
        .with_size(egui::vec2(width, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

/// Every tool button's rect must sit inside the window — a tool pushed into
/// the scroll overflow is a tool the user cannot see (rostad's design flag).
#[test]
fn all_thirteen_tools_fit_the_toolbar_at_1100px() {
    let dir = scratch("fit");
    let mut h = harness(&dir, 1100.0);
    h.run_steps(2);
    for tool in Tool::ALL {
        let node = h.get_by_label(tool.label());
        let rect = node.rect();
        assert!(
            rect.max.x <= 1100.0 && rect.min.x >= 0.0 && rect.is_positive(),
            "{tool:?} must be fully visible in the toolbar, got {rect:?}"
        );
    }
    // The action side survived the compaction too.
    let _ = h.get_by_label("Capture");
    let _ = h.get_by_label("File");
    let _ = h.get_by_label("OCR");
    let _ = h.get_by_label("Save");
    let _ = h.get_by_label("Copy");
    h.snapshot("w29_toolbar_fit");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Open/Export/Resize left the row for the File menu — the menu must carry
/// them (no feature lost in the compaction).
#[test]
fn file_menu_carries_the_moved_document_ops() {
    let dir = scratch("menu");
    let mut h = harness(&dir, 1100.0);
    h.run_steps(2);
    h.get_by_label("File").click();
    h.run_steps(3);
    let _ = h.get_by_label("Open…");
    let _ = h.get_by_label("Export…");
    let _ = h.get_by_label("Resize…");
    let _ = std::fs::remove_dir_all(&dir);
}
