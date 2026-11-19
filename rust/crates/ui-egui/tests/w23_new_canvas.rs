//! W23 gates: the live New-canvas dialog — white/transparent background,
//! radio presets with the matching card highlighted, remembered size/bg.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w23-{tag}-{}-{:?}",
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
    app.push_settings(SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1000.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn base_alpha(app: &SnagCraftApp) -> u8 {
    app.sess()
        .doc()
        .map(|d| d.base.get_pixel(0, 0).0[3])
        .unwrap_or(255)
}

#[test]
fn transparent_canvas_has_a_zero_alpha_base() {
    let dir = scratch("transparent");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    app.create_new_canvas(64.0, 48.0, "transparent");
    assert_eq!(base_alpha(&app), 0, "the base must be fully transparent");
    assert_eq!(
        app.sess().doc().map(|d| (d.size.w, d.size.h)),
        Some((64, 48))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn white_stays_the_default_and_unknown_bg_falls_back() {
    let dir = scratch("white");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    app.create_new_canvas(32.0, 32.0, "white");
    assert_eq!(base_alpha(&app), 255);
    // A hostile bg value must never produce a transparent surprise.
    app.create_new_canvas(32.0, 32.0, "file:///etc/passwd");
    assert_eq!(base_alpha(&app), 255);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_dialog_remembers_size_and_background() {
    let dir = scratch("remember");
    let mut h = harness(&dir);
    h.state_mut().create_new_canvas(640.0, 360.0, "transparent");
    h.run_steps(1);
    {
        let app = h.state();
        assert_eq!(app.settings.new_canvas_w, 640);
        assert_eq!(app.settings.new_canvas_h, 360);
        assert_eq!(app.settings.new_canvas_bg, "transparent");
        // The engine owns the persisted values too.
        let got = app
            .sess()
            .dispatch("settings.get", serde_json::json!({}))
            .unwrap();
        assert_eq!(got["new_canvas_w"], 640);
        assert_eq!(got["new_canvas_bg"], "transparent");
    }
    // Reopening prefills from the remembered prefs (live readNewPrefs).
    h.state_mut().open_new_canvas();
    h.run_steps(1);
    assert_eq!(h.state().new_canvas_w, 640.0);
    assert_eq!(h.state().new_canvas_h, 360.0);
    assert_eq!(h.state().new_canvas_bg, "transparent");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preset_cards_highlight_the_matching_size() {
    let dir = scratch("highlight");
    let mut h = harness(&dir);
    h.state_mut().open_new_canvas();
    h.run_steps(2);
    // Default 1280×800 is preselected: its card carries the accent ring.
    h.snapshot("w23_new_canvas_default");
    // Clicking HD fills the fields and moves the highlight (radio semantics).
    h.get_by_label_contains("HD 1280").click();
    h.run_steps(2);
    assert_eq!(h.state().new_canvas_w, 1280.0);
    assert_eq!(h.state().new_canvas_h, 720.0);
    assert!(
        h.state().new_canvas_open,
        "no create until Create is pressed"
    );
    h.snapshot("w23_new_canvas_hd");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transparent_choice_flows_through_the_dialog() {
    let dir = scratch("bg-flow");
    let mut h = harness(&dir);
    h.state_mut().open_new_canvas();
    h.run_steps(2);
    h.get_by_label("Transparent").click();
    h.run_steps(1);
    assert_eq!(h.state().new_canvas_bg, "transparent");
    h.get_by_label("Create").click();
    h.run_steps(2);
    assert_eq!(
        base_alpha(h.state()),
        0,
        "the dialog's choice reached doc.new"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
