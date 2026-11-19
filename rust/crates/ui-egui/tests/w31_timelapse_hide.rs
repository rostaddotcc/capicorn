//! W31 gates: timelapse frames must not contain SnagCraft itself. The worker
//! asks the UI to hide the app's windows before each grab (hide-ack
//! handshake through the engine), and every outcome brings them back.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use serde_json::json;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w31-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn settings(dir: &std::path::Path) -> SettingsView {
    SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    }
}

fn harness(dir: &std::path::Path) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(dir));
    Harness::builder()
        .with_size(egui::vec2(1000.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

/// The state machine: hide → ack → restore per frame, and the run's end
/// restores even if it lands while the windows are hidden.
#[test]
fn hide_ack_handshake_hides_and_restores_per_frame() {
    let dir = scratch("seam");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(&dir));
    let ctx = egui::Context::default();
    app.logic(&ctx); // captures main_ctx
    app.timelapse.armed = true;

    app.apply_timelapse_event(&json!({"type": "hide_for_frame"}));
    assert!(app.hidden && app.timelapse.frame_hidden, "frame grab hides");

    app.apply_timelapse_event(&json!({"type": "restore_after_frame"}));
    assert!(!app.hidden && !app.timelapse.frame_hidden, "windows back");

    // A second frame repeats cleanly (the ack is consumed per frame).
    app.apply_timelapse_event(&json!({"type": "hide_for_frame"}));
    assert!(app.hidden && app.timelapse.frame_hidden);

    // The run ends WHILE hidden — still restored, picker cleared.
    app.apply_timelapse_event(&json!({"type": "failed", "message": "portal: refused"}));
    assert!(!app.hidden && !app.timelapse.frame_hidden);
    assert!(!app.timelapse.armed);
    assert!(app.status.contains("Timelapse failed"), "{}", app.status);

    // A duplicate restore is a no-op (flag-guarded).
    app.apply_timelapse_event(&json!({"type": "restore_after_frame"}));
    assert!(!app.hidden);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A normal end (done) also restores and keeps the W15 stop-time behavior.
#[test]
fn done_event_restores_and_disarms() {
    let dir = scratch("done");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(&dir));
    let ctx = egui::Context::default();
    app.logic(&ctx);
    app.timelapse.armed = true;

    app.apply_timelapse_event(&json!({"type": "hide_for_frame"}));
    assert!(app.hidden);
    app.apply_timelapse_event(&json!({"type": "done", "frames": 0}));
    assert!(!app.hidden && !app.timelapse.frame_hidden);
    assert!(!app.timelapse.armed);
    let _ = std::fs::remove_dir_all(&dir);
}

/// End-to-end through the REAL engine + worker: start a 1-frame run, the
/// handshake must hide the windows for the grab and restore them on the
/// outcome (a portal refusal on CI, the 15 s timeout on a locked build host).
#[test]
fn real_timelapse_run_hides_for_the_frame_and_restores() {
    let dir = scratch("e2e");
    let mut h = harness(&dir);
    h.run_steps(1);
    let started = h
        .state()
        .sess()
        .dispatch(
            "timelapse.start",
            json!({"period_secs": 1, "count": 1, "dir": dir.to_string_lossy()}),
        )
        .expect("timelapse.start");
    assert_eq!(started["started"], true);
    {
        let app = h.state_mut();
        app.timelapse.armed = true;
        app.timelapse.dir = Some(dir.clone());
    }

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut saw_hidden = false;
    let mut ended = false;
    while Instant::now() < deadline {
        h.run_steps(1);
        let st = h.state();
        saw_hidden |= st.timelapse.frame_hidden;
        if !st.timelapse.armed {
            ended = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = h.state().status.clone();
    assert!(ended, "the run reached an outcome: {status}");
    // On a fast-refusal host every event can land in one poll batch, so the
    // hidden phase is not always observable from outside.
    assert!(
        saw_hidden || status.contains("Timelapse failed"),
        "the frame grab hid the windows (or the portal refused): {status}"
    );
    assert!(!h.state().hidden, "windows restored after the run");
    assert!(!h.state().timelapse.frame_hidden);
    let _ = std::fs::remove_dir_all(&dir);
}
