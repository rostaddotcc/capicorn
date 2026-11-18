//! W18 gates: recording-flow parity with the live app — stop→Recent pickup,
//! empty-stop report, pause-aware HUD clock, start-error classification,
//! live filename pattern, region scope tag, and the capture-busy guard.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{
    SettingsView, SnagCraftApp, Theme, app_ui, apply_theme, record_start_error_status,
    recording_filename,
};

fn harness() -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        // Never scan/write the real ~/Pictures from a test (W4 idiom).
        save_dir: Some(
            std::env::temp_dir()
                .join("snagcraft-w18-tests")
                .to_string_lossy()
                .to_string(),
        ),
        ..SettingsView::default()
    };
    Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

#[test]
fn stopped_take_lands_in_the_recent_rail() {
    let mut h = harness();
    {
        let app = h.state_mut();
        app.rec.armed = true;
        app.rec.active = true;
        app.rec.started_at = Some(Instant::now());
    }
    let path = "/tmp/snagcraft-w18/SnagCraft-2026-11-18-12-00-00.mp4";
    h.state_mut().record_stopped(path);
    assert_eq!(h.state().status, format!("Recording saved: {path}"));
    assert!(!h.state().rec.armed);
    assert!(!h.state().rec.active);
    assert!(
        h.state()
            .recent
            .iter()
            .any(|item| item.path == std::path::Path::new(path)),
        "the finished take must appear in the Recent rail immediately"
    );
}

#[test]
fn empty_stop_reports_no_file_written() {
    let mut h = harness();
    h.state_mut().record_stopped("");
    assert_eq!(
        h.state().status,
        "Recording stopped, but no file was written."
    );
    assert!(h.state().recent.is_empty());
}

#[test]
fn pause_clock_freezes_and_resume_skips_the_paused_span() {
    let mut h = harness();
    let now = Instant::now();
    {
        let app = h.state_mut();
        // A 20 s take with 5 s already banked as paused: 15 s on the clock.
        app.rec.started_at = Some(now - Duration::from_secs(20));
        app.rec.paused_total = Duration::from_secs(5);
    }
    let running = h.state().record_elapsed();
    assert!(
        running >= Duration::from_secs(14) && running <= Duration::from_secs(16),
        "running clock should be ~15 s, got {running:?}"
    );
    // Pause at (now - 2 s): the clock must freeze at exactly 13 s.
    h.state_mut().rec.paused_at = Some(now - Duration::from_secs(2));
    assert_eq!(h.state().record_elapsed(), Duration::from_secs(13));
    assert_eq!(h.state().record_clock(), "00:13");
    // A second read while paused must not advance.
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(h.state().record_elapsed(), Duration::from_secs(13));
}

#[test]
fn start_errors_classify_like_the_live_app() {
    // The rebuild wraps portal errors; a user cancel stays neutral.
    assert_eq!(
        record_start_error_status(
            "screen cast portal: Start screen cast (picker) failed: Request cancelled by user"
        ),
        "Recording cancelled — no screen was picked."
    );
    // Permission/portal refusal says how to recover.
    assert_eq!(
        record_start_error_status("screen cast portal: screen sharing was refused: Access denied"),
        "Recording cancelled — screen sharing wasn't allowed. Approve the system prompt and try again."
    );
    // Anything else points at the portal log the rebuild actually writes.
    let other =
        record_start_error_status("ffmpeg: cannot start ffmpeg (No such file or directory)");
    assert!(other.starts_with("Could not start recording: ffmpeg:"));
    assert!(other.contains("~/.cache/snagcraft-portal.log"));
}

#[test]
fn recording_filename_matches_the_live_pattern() {
    let path = recording_filename(std::path::Path::new("/tmp/snagcraft-w18"), "mp4");
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.starts_with("SnagCraft-"), "live stem: {name}");
    assert!(name.ends_with(".mp4"), "container extension: {name}");
    assert!(!name.contains("snagcraft-rec"), "no epoch names: {name}");
    // {date}-{time} expanded: YYYY-MM-DD-HH-MM-SS digits and dashes.
    let stem = name
        .trim_start_matches("SnagCraft-")
        .trim_end_matches(".mp4");
    assert!(
        stem.chars().all(|c| c.is_ascii_digit() || c == '-'),
        "expanded tokens: {stem}"
    );
    assert_eq!(stem.len(), 19, "YYYY-MM-DD-HH-MM-SS: {stem}");
    let gif = recording_filename(std::path::Path::new("/tmp"), "gif");
    assert!(gif.to_string_lossy().ends_with(".gif"));
}

#[test]
fn region_take_shows_the_scope_tag_in_the_hud() {
    let mut h = harness();
    {
        let app = h.state_mut();
        // `active` only: poll_record clears an injected `armed` flag when the
        // engine reports no running take (the W4 injection idiom).
        app.rec.active = true;
        // Future start instant saturates the clock to 00:00 (W4 idiom).
        app.rec.started_at = Instant::now().checked_add(Duration::from_secs(600));
        app.rec.region = Some(snagcraft_geom::Rect {
            x: 0.0,
            y: 0.0,
            w: 640.0,
            h: 480.0,
        });
    }
    h.run_steps(2);
    let _ = h.get_by_label_contains("Recording 00:00");
    let _ = h.get_by_label("Region");
    // Whole-screen takes carry no scope tag.
    h.state_mut().rec.region = None;
    h.run_steps(2);
    assert!(
        h.query_by_label("Region").is_none(),
        "the Region tag must disappear without a region"
    );
}

#[test]
fn capture_in_flight_blocks_a_new_take() {
    let mut h = harness();
    h.state_mut().scroll.armed = true;
    h.state_mut().start_recording(None);
    assert_eq!(
        h.state().status,
        "Wait for the capture to finish before recording."
    );
    assert!(!h.state().rec.armed);
}
