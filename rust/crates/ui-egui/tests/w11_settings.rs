// Tests are exempt from the no-unwrap/no-expect house rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! W11 parity long-tail: the settings cluster (interval, mic, system audio,
//! hotkeys switch + Register, recording format, split folders, filename
//! preview), Troubleshooting → Copy diagnostics, and the PrintScreen in-app
//! fallback.

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{
    CaptureKind, SettingsView, SnagCraftApp, Theme, Tool, app_ui, apply_theme,
};

fn scratch_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("snagcraft-w11-tests-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Deterministic shell: no host settings file, no floating widget, and a temp
/// save folder. `capture_enabled` is off so the PrintScreen dispatch is
/// asserted without opening the portal picker.
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

/// Every W11 row is on the panel, and the preview goes through
/// `io::files::build_filename` with the fixed example stamp.
#[test]
fn settings_panel_renders_the_w11_rows() {
    let dir = scratch_dir("rows");
    let mut h = harness(&dir);
    h.run();
    open_settings(&mut h);

    h.get_by_label("Interval capture");
    h.get_by_value("every 30 s");
    h.get_by_label_contains("Interval capture shoots this often");
    h.get_by_label("Microphone");
    h.get_by_label("System audio (desktop output)");
    h.get_by_label("Enable global hotkeys");
    h.get_by_label("Register hotkeys");
    h.get_by_label("Recording format");
    h.get_by_value("mp4");
    h.get_by_label("Separate recording folder");
    h.get_by_label("Same as the save folder");
    h.get_by_label("Copy diagnostics");
    h.get_by_label_contains("Preview: SnagCraft-2026-11-14-19-45-12.png");
    h.get_by_label_contains("{datetime} both");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The interval dropdown writes through `settings.set` (the engine owns it).
#[test]
fn interval_choice_is_persisted_through_the_engine() {
    let dir = scratch_dir("interval");
    let mut h = harness(&dir);
    h.run();
    open_settings(&mut h);
    assert_eq!(h.state().settings.interval_secs, 30);

    h.get_by_value("every 30 s").click();
    h.run();
    h.get_by_label("every 5 min").click();
    h.run();

    assert_eq!(h.state().settings.interval_secs, 300);
    let session = h.state().session_arc();
    let got = session
        .lock()
        .unwrap()
        .dispatch("settings.get", serde_json::json!({}))
        .unwrap();
    assert_eq!(got["interval_secs"], 300, "engine saw the change");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `audio.sources` feeds the mic dropdown, and a pick sticks.
#[test]
fn mic_dropdown_lists_sources_and_sets_the_choice() {
    let dir = scratch_dir("mic");
    let mut h = harness(&dir);
    h.state_mut().rec.sources = vec![(
        "alsa_input.pci-test".to_string(),
        "Test Microphone".to_string(),
    )];
    h.run();
    open_settings(&mut h);

    h.get_by_value("None").click();
    h.run();
    h.get_by_label("Test Microphone").click();
    h.run();

    assert_eq!(
        h.state().settings.mic_source.as_deref(),
        Some("alsa_input.pci-test")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Troubleshooting → Copy diagnostics always reports a status: the bytes on
/// success, the reason when the session has no clipboard (headless CI).
#[test]
fn copy_diagnostics_reports_a_status() {
    let dir = scratch_dir("diag");
    let mut h = harness(&dir);
    h.run();
    open_settings(&mut h);

    // The panel row exists (asserted above); the command path itself is driven
    // directly here because the Troubleshooting row sits at the bottom of a
    // scrolling panel, where a synthetised click misses the clip rect.
    h.get_by_label("Copy diagnostics");
    h.state_mut().copy_diagnostics();
    h.run();

    let status = h.state().status.clone();
    assert!(
        status.to_lowercase().contains("diagnostic"),
        "the button must answer with a diagnostics status: {status:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// PrintScreen → capture full, Shift+PrintScreen → capture region (the in-app
/// fallback while global hotkeys are off).
#[test]
fn printscreen_keys_dispatch_capture() {
    let dir = scratch_dir("keys");
    let mut h = harness(&dir);
    h.run();
    assert_eq!(h.state().last_capture, None);

    h.key_press(egui::Key::PrintScreen);
    h.run();
    assert_eq!(
        h.state().last_capture,
        Some(CaptureKind::Full),
        "PrintScreen captures the full screen"
    );

    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::PrintScreen);
    h.run();
    assert_eq!(
        h.state().last_capture,
        Some(CaptureKind::Region),
        "Shift+PrintScreen captures a region"
    );
    assert!(
        matches!(h.state().tool, Tool::Crop),
        "region capture arms the crop tool"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The panel's JSON shape carries every W11 field, and hostile values are
/// clamped/dropped instead of failing `settings.set`.
#[test]
fn settings_view_json_round_trips_the_w11_cluster() {
    let v = SettingsView {
        interval_secs: 300,
        mic_source: Some("alsa_input.test".to_string()),
        system_audio: true,
        hotkeys_enabled: true,
        recording_format: "gif".to_string(),
        split_folders: true,
        recording_dir: Some("/tmp/rec".to_string()),
        ..SettingsView::default()
    };
    let back = SettingsView::from_json(&v.to_json());
    assert_eq!(back, v);

    // Unknown recording formats, blank device names and empty dirs never make
    // it into a `settings.set` the engine would reject.
    let hostile = SettingsView::from_json(&serde_json::json!({
        "interval_secs": 99_999,
        "mic_source": "  ",
        "recording_format": "mkv",
        "recording_dir": "",
    }));
    assert_eq!(hostile.interval_secs, 3_600);
    assert_eq!(hostile.mic_source, None);
    assert_eq!(hostile.recording_format, "mp4");
    assert_eq!(hostile.recording_dir, None);
    // Pre-W11 documents keep the defaults.
    let old = SettingsView::from_json(&serde_json::json!({"default_quality": 90}));
    assert_eq!(old.interval_secs, 30);
    assert_eq!(old.recording_format, "mp4");
    assert!(!old.hotkeys_enabled);
}
