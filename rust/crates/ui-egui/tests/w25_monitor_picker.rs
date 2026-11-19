//! W25 gates: the in-app monitor picker (issue #1) — seeded monitor list,
//! click-to-capture dispatch, Esc/Cancel, the Linux empty-list fallback, and
//! the engine capture.monitors / capture.monitor commands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_capture::MonitorInfo;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w25-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn monitor(index: usize, name: &str, w: u32, h: u32, primary: bool) -> MonitorInfo {
    MonitorInfo {
        index,
        name: name.to_string(),
        x: 0,
        y: 0,
        width: w,
        height: h,
        is_primary: primary,
        scale_factor: 1.0,
    }
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

fn seeded_picker(h: &mut Harness<'static, SnagCraftApp>) {
    let app = h.state_mut();
    app.monitors = vec![
        monitor(0, "DELL U2720Q", 3840, 2160, true),
        monitor(1, "HP E243", 1920, 1080, false),
    ];
    app.monitor_picker_open = true;
}

#[test]
fn picker_lists_monitors_with_resolution_and_primary_badge() {
    let dir = scratch("list");
    let mut h = harness(&dir);
    seeded_picker(&mut h);
    h.run_steps(2);
    let _ = h.get_by_label("Choose a screen");
    let _ = h.get_by_label_contains("DELL U2720Q 3840 × 2160");
    let _ = h.get_by_label_contains("HP E243 1920 × 1080");
    h.snapshot("w25_monitor_picker");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clicking_a_monitor_closes_the_picker_and_starts_a_capture() {
    let dir = scratch("pick");
    let mut h = harness(&dir);
    seeded_picker(&mut h);
    h.run_steps(2);
    h.get_by_label_contains("HP E243").click();
    h.run_steps(1);
    assert!(
        !h.state().monitor_picker_open,
        "the picker closes on choice"
    );
    // The off-thread capture pipeline was armed. On Linux the backend reports
    // unsupported (no Windows runtime here), which poll_capture surfaces — so
    // the status shows either the in-flight grab or its unsupported result.
    // Both prove the click dispatched capture.monitor for the chosen index.
    let status = h.state().status.clone();
    assert!(
        h.state().capture_in_flight()
            || status.contains("Capturing the selected monitor")
            || status.contains("Capture failed"),
        "a capture was dispatched: {status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn esc_and_cancel_close_the_picker() {
    let dir = scratch("close");
    let mut h = harness(&dir);
    seeded_picker(&mut h);
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    assert!(!h.state().monitor_picker_open, "Esc closes the picker");

    seeded_picker(&mut h);
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(1);
    assert!(!h.state().monitor_picker_open, "Cancel closes the picker");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_monitor_list_opens_the_system_picker_capture() {
    let dir = scratch("empty");
    let mut h = harness(&dir);
    // On Linux capture.monitors is genuinely empty (the portal is the picker).
    // W41: this now OPENS the ScreenCast system-picker capture instead of the
    // old dead-end "start a recording" status note.
    h.state_mut().open_monitor_picker();
    h.run_steps(1);
    assert!(
        !h.state().monitor_picker_open,
        "no in-app picker without monitors"
    );
    assert!(
        h.state().capture_in_flight(),
        "the system-picker capture was armed"
    );
    assert!(
        h.state().status.contains("Pick a screen"),
        "the picker status replaces the dead-end note: {}",
        h.state().status
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn engine_capture_monitors_and_monitor_commands() {
    let mut s = snagcraft_engine::Session::new();
    // capture.monitors answers (empty on Linux, populated on Windows/macOS).
    let v = s
        .dispatch("capture.monitors", serde_json::json!({}))
        .unwrap();
    assert!(v["monitors"].is_array());
    // capture.monitor requires an index.
    assert!(
        s.dispatch("capture.monitor", serde_json::json!({}))
            .is_err(),
        "missing index is rejected"
    );
    // On Linux a direct index is unsupported (the portal picker is the path).
    #[cfg(target_os = "linux")]
    {
        let err = s
            .dispatch("capture.monitor", serde_json::json!({"index": 0}))
            .unwrap_err();
        assert!(
            err.to_string().contains("portal picker"),
            "Linux routes to the portal picker: {err}"
        );
    }
}
