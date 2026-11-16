//! Headless visual gate for the W1 engine-driven shell.

use egui_kittest::Harness;
use snagcraft_ui_egui::{SnagCraftApp, app_ui, apply_theme};

#[test]
fn w1_shell_visual() {
    let app = SnagCraftApp::seeded();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_accessibility_check(false)
        .with_context_setup(apply_theme)
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app);
    harness.run_steps(5);
    harness.snapshot("w1_shell");
}
