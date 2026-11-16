//! Headless visual gate for the W2b 13-tool parity shell.

use egui_kittest::Harness;
use snagcraft_ui_egui::{SnagCraftApp, app_ui, apply_theme};

fn harness(size: egui::Vec2) -> Harness<'static, SnagCraftApp> {
    let app = SnagCraftApp::seeded();
    Harness::builder()
        .with_size(size)
        .with_accessibility_check(false)
        .with_context_setup(apply_theme)
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

#[test]
fn w2_shell_visual() {
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(5);
    h.snapshot("w2_shell");
}

/// Narrow window: the toolbar must stay ONE row (horizontal scroll, never wrap).
#[test]
fn w2_shell_narrow_visual() {
    let mut h = harness(egui::vec2(760.0, 620.0));
    h.run_steps(5);
    h.snapshot("w2_shell_narrow");
}
