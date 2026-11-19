// Tests are exempt from the no-unwrap/no-expect house rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! W27 parity: Settings → Capture matches the live app — a separate Capture
//! section (delay + interval) and the capture delay as a TIMER_CHOICES pill
//! radio group [Off, 3 s, 5 s, 10 s], not a continuous slider.

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w27-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn harness(dir: &std::path::Path) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    };
    app.capture_enabled = false;
    Harness::builder()
        .with_size(egui::vec2(1000.0, 1700.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn open_settings(h: &mut Harness<'static, SnagCraftApp>) {
    h.get_by_label("Settings (Ctrl+,)").click();
    h.run();
}

#[test]
fn capture_delay_is_a_pill_group_matching_timer_choices() {
    let dir = scratch_dir("pills");
    let mut h = harness(&dir);
    h.run();
    open_settings(&mut h);

    // The live TIMER_CHOICES = [0, 3, 5, 10] render as Off / 3 s / 5 s / 10 s.
    h.get_by_label("Capture delay");
    for label in ["Off", "3 s", "5 s", "10 s"] {
        h.get_by_label(label);
    }
    // Default is Off (0 s).
    assert_eq!(h.state().settings.capture_delay_secs, 0);
    h.snapshot("w27_settings_capture");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn picking_a_delay_pill_persists_through_the_engine() {
    let dir = scratch_dir("persist");
    let mut h = harness(&dir);
    h.run();
    open_settings(&mut h);
    assert_eq!(h.state().settings.capture_delay_secs, 0);

    h.get_by_label("5 s").click();
    h.run();

    assert_eq!(h.state().settings.capture_delay_secs, 5);
    let session = h.state().session_arc();
    let got = session
        .lock()
        .unwrap()
        .dispatch("settings.get", serde_json::json!({}))
        .unwrap();
    assert_eq!(got["capture_delay_secs"], 5, "engine saw the change");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_section_groups_delay_and_interval() {
    let dir = scratch_dir("section");
    let mut h = harness(&dir);
    h.run();
    open_settings(&mut h);

    // The live app splits General (switches) from Capture (delay + interval).
    // The help line is unique to the Capture section ("Capture" alone is also
    // the toolbar menu button, so it is not a unique label).
    h.get_by_label_contains("Wait before grabbing the screen");
    h.get_by_label("Capture delay");
    h.get_by_label("Interval capture");
    let _ = std::fs::remove_dir_all(&dir);
}
