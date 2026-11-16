//! Headless visual gate for the W2b 13-tool parity shell.

use egui_kittest::Harness;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

/// Seeded shell with deterministic settings (the host config file must not
/// leak into a snapshot) and the floating widget off — it has its own test.
fn harness(size: egui::Vec2) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        ..SettingsView::default()
    };
    Harness::builder()
        .with_size(size)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
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
