//! W3b shell gates: settings panel (button + Ctrl+,), theme swap pixels,
//! close-guard modal on a dirty document, and the floating widget viewport.
//!
//! The host settings file must never leak into these tests, so every harness
//! starts from [`SettingsView::default`].

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_platform::{HotkeyEvent, PlatformError, TrayEvent};
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, Tool, app_ui, apply_theme};

fn harness(show_widget: bool) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget,
        ..SettingsView::default()
    };
    app.widget_expanded = true;
    Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

/// The settings panel is tall (W11 added a row per section): these tests need a
/// viewport that shows every row, so a click lands on the widget it names.
fn harness_tall(show_widget: bool) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget,
        ..SettingsView::default()
    };
    app.widget_expanded = true;
    Harness::builder()
        .with_size(egui::vec2(1100.0, 1_400.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

/// Inject the eframe close request (`ViewportEvent::Close`, viewport_info.rs:35).
/// The event is consumed by the next frame, so the caller runs one step before
/// reading the emitted viewport commands.
fn request_close(h: &mut Harness<'static, SnagCraftApp>) {
    if let Some(info) = h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT) {
        info.events.push(egui::ViewportEvent::Close);
    }
}

/// Root-viewport commands emitted by the frame just rendered.
fn root_commands(h: &Harness<'static, SnagCraftApp>) -> Vec<egui::ViewportCommand> {
    h.output()
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|out| out.commands.clone())
        .unwrap_or_default()
}

#[test]
fn settings_window_opens_from_the_gear_button() {
    let mut h = harness_tall(false);
    h.run_steps(3);
    h.get_by_label("Settings (Ctrl+,)").click();
    h.run_steps(2);
    // Sections and the first general toggle are in the accessibility tree.
    for label in [
        "General",
        "Minimize to tray instead of closing",
        "Auto-save captures",
        "Theme",
    ] {
        let _ = h.get_by_label(label);
    }
    h.snapshot("w3_settings");
}

#[test]
fn settings_window_opens_with_ctrl_comma() {
    let mut h = harness_tall(false);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Comma);
    h.run_steps(2);
    let _ = h.get_by_label("Auto-save captures");
}

#[test]
fn theme_swap_changes_pixels_and_state() {
    let mut h = harness_tall(false);
    h.run_steps(3);
    let light = h.render().expect("render light");
    h.get_by_label("Settings (Ctrl+,)").click();
    h.run_steps(2);
    // W11 added settings rows above Theme, pushing it below the scroll fold;
    // click via AccessKit so the off-screen radio is still activated.
    h.get_by_label("Dark").click_accesskit();
    h.run_steps(3);

    assert_eq!(h.state().settings.theme, Theme::Dark);
    // The engine owns the value: settings.set must have applied it too.
    let applied = h
        .state_mut()
        .sess()
        .dispatch("settings.get", serde_json::json!({}))
        .expect("settings.get");
    assert_eq!(applied["theme"], "Dark");

    let dark = h.render().expect("render dark");
    assert_eq!(light.dimensions(), dark.dimensions());
    // Compare the chrome strip only: the canvas shows the same image in both
    // themes, while every panel/button must repaint.
    let width = light.width();
    let strip_rows = 120;
    let mut changed = 0usize;
    let mut total = 0usize;
    for y in 0..strip_rows.min(light.height()) {
        for x in 0..width {
            total += 1;
            if light.get_pixel(x, y) != dark.get_pixel(x, y) {
                changed += 1;
            }
        }
    }
    assert!(
        changed * 4 > total * 3,
        "theme swap should repaint the toolbar: {changed}/{total} pixels differ"
    );
    h.snapshot("w3_shell_dark");
}

#[test]
fn settings_change_marks_persistence_dirty() {
    let mut h = harness_tall(false);
    h.run_steps(2);
    assert!(!h.state_mut().take_settings_dirty(), "clean at startup");
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Comma);
    h.run_steps(2);
    // W11 added settings rows above Theme, pushing it below the scroll fold;
    // click via AccessKit so the off-screen radio is still activated.
    h.get_by_label("Dark").click_accesskit();
    h.run_steps(2);
    assert!(
        h.state_mut().take_settings_dirty(),
        "a settings change must ask the app layer to save"
    );
}

#[test]
fn close_request_on_dirty_document_shows_the_guard() {
    let mut h = harness(false);
    h.run_steps(3);
    assert!(h.state().doc_dirty, "the seeded scene is unsaved");
    request_close(&mut h);
    h.run_steps(1);
    // The frame that saw the close request must answer with CancelClose.
    assert!(
        root_commands(&h)
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::CancelClose)),
        "the close must be cancelled while the modal is open"
    );
    h.run_steps(2);
    let _ = h.get_by_label("Unsaved changes");
    let _ = h.get_by_label("You have unsaved annotations");
    // "Save" also exists in the toolbar, so the modal's copy is the second one.
    assert!(h.query_all_by_label("Save").count() >= 2);
    for label in ["Discard", "Cancel"] {
        let _ = h.get_by_label(label);
    }
    assert!(h.state().confirm_close, "modal stays up until answered");
    h.snapshot("w3_close_guard");
    // Cancel dismisses without closing.
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(!h.state().confirm_close);
}

#[test]
fn close_request_on_clean_document_is_allowed_through() {
    let mut h = harness(false);
    h.state_mut().doc_dirty = false;
    h.run_steps(3);
    request_close(&mut h);
    h.run_steps(1);
    assert!(!h.state().confirm_close);
    assert!(
        !root_commands(&h)
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::CancelClose)),
        "a clean document closes without a guard"
    );
}

#[test]
fn floating_widget_renders_both_states() {
    let mut h = harness(true);
    h.run_steps(3);
    // Expanded pill: capture buttons plus the disabled Rec affordance.
    for label in ["Full", "Region", "Rec"] {
        let _ = h.get_by_label(label);
    }
    h.snapshot("w3_widget_expanded");

    // Fold to the 58×74 tab, then re-expand with Enter (a11y path).
    h.get_by_label("»").click();
    h.run_steps(2);
    assert!(!h.state().widget_expanded);
    h.snapshot("w3_widget_folded");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().widget_expanded);
}

#[test]
fn widget_close_hides_it_and_persists_the_setting() {
    let mut h = harness(true);
    h.run_steps(3);
    h.get_by_label("×").click();
    h.run_steps(2);
    assert!(!h.state().settings.show_widget);
    assert!(h.state_mut().take_settings_dirty());
}

#[test]
fn close_request_hides_to_the_tray_when_configured() {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        minimize_to_tray: true,
        ..SettingsView::default()
    };
    app.doc_dirty = false;
    app.tray_ok = true;
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app);
    h.run_steps(2);
    request_close(&mut h);
    h.run_steps(1);
    let commands = root_commands(&h);
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::Visible(false))),
        "a tray-ready shell hides instead of quitting"
    );
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::CancelClose)),
        "hiding must cancel the close"
    );
    assert!(!h.state().confirm_close);
}

#[test]
fn tray_and_hotkey_channels_drive_the_shell() {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        ..SettingsView::default()
    };
    let (tray_tx, tray_rx) = std::sync::mpsc::channel();
    app.attach_tray_events(tray_rx);
    let (hotkey_tx, hotkey_rx) = std::sync::mpsc::channel();
    app.attach_hotkeys(
        hotkey_tx.clone(),
        hotkey_rx,
        Err(PlatformError::Hotkeys("test host".to_string())),
    );
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app);
    h.run_steps(2);

    // Tray middle click / "Floating widget" toggles the widget setting.
    let _ = tray_tx.send(TrayEvent::ToggleWidget);
    h.run_steps(2);
    assert!(h.state().settings.show_widget);

    // Hotkey activation routes by portal id.
    let _ = hotkey_tx.send(HotkeyEvent::Activated {
        id: "capture-region".to_string(),
    });
    h.run_steps(1);
    assert_eq!(h.state().tool, Tool::Crop);

    // Tray Quit bypasses the close guard.
    let _ = tray_tx.send(TrayEvent::Quit);
    h.run_steps(1);
    assert!(h.state().force_quit);
}
