//! W29 gates: the capture hide/settle/restore dance (live commands.rs:41-96).
//! The app's own windows must vanish for the grab and come back on EVERY
//! outcome — and while the root is hidden, only `logic` runs, so it must be
//! the pump that lands the result (the W28 app_ui/logic lesson).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use snagcraft_ui_egui::{CaptureMsg, SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w29-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn settings(dir: &std::path::Path, show_widget: bool) -> SettingsView {
    SettingsView {
        show_widget,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    }
}

fn harness(dir: &std::path::Path) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(dir, false));
    Harness::builder()
        .with_size(egui::vec2(1000.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

/// End-to-end through the REAL worker thread: HideForGrab must hide the root
/// mid-flight, and the outcome (a portal refusal on CI, the 15 s timeout on a
/// locked build host — never a success without an unlocked desktop) must
/// restore it. Wall-time wait loop: kittest steps advance egui time, not the
/// clock the capture thread sleeps on.
#[test]
fn real_capture_hides_the_windows_and_restores_them_on_the_outcome() {
    let dir = scratch("e2e");
    let mut h = harness(&dir);
    h.state_mut().capture_full();
    assert!(h.state().capture_in_flight(), "the worker was armed");

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut saw_hidden = false;
    let mut outcome = String::new();
    while Instant::now() < deadline {
        h.run_steps(1);
        let st = h.state();
        saw_hidden |= st.hidden;
        if st.status.starts_with("Capture failed") || st.status.starts_with("Captured") {
            outcome = st.status.clone();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !outcome.is_empty(),
        "the capture reached an outcome, status: {}",
        h.state().status
    );
    // The 220 ms settle guarantees HideForGrab is pollable before Done, so
    // the hidden phase is observable on every host.
    assert!(saw_hidden, "HideForGrab hid the windows mid-flight");
    assert!(!h.state().hidden, "windows restored after the outcome");
    assert!(!h.state().capture_hidden);
    assert!(!h.state().capture_in_flight());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The hidden path: eframe runs ONLY `logic` while the root is hidden, so a
/// successful capture must land (document + autosave + Recent) and restore
/// the windows without a single `app_ui` pass after the hide.
#[test]
fn logic_lands_a_capture_while_the_root_is_hidden() {
    let dir = scratch("logic-ok");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(&dir, false));
    let tx = app.seed_capture_channel();
    let ctx = egui::Context::default();

    tx.send(CaptureMsg::HideForGrab).unwrap();
    app.logic(&ctx);
    assert!(
        app.hidden && app.capture_hidden,
        "HideForGrab hides the root"
    );

    tx.send(CaptureMsg::Done(Ok(snagcraft_raster::filled(
        snagcraft_geom::Size::new(8, 8),
        snagcraft_color::Rgba::WHITE,
    ))))
    .unwrap();
    app.logic(&ctx);
    assert!(!app.hidden && !app.capture_hidden, "windows restored");
    assert!(!app.capture_in_flight());
    assert!(
        app.status.contains("Captured 8×8"),
        "the surface landed: {}",
        app.status
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failed capture restores the windows too (live `restore_app_windows`
/// runs on the error path), with the widget that was hidden alongside.
#[test]
fn logic_restores_the_widget_too_on_a_failed_capture() {
    let dir = scratch("logic-err");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(&dir, true));
    app.widget_shown = true;
    let tx = app.seed_capture_channel();
    let ctx = egui::Context::default();

    tx.send(CaptureMsg::HideForGrab).unwrap();
    app.logic(&ctx);
    assert!(app.hidden && app.capture_hidden);

    tx.send(CaptureMsg::Done(Err("portal: refused".into())))
        .unwrap();
    app.logic(&ctx);
    assert!(!app.hidden && !app.capture_hidden, "restored on failure");
    assert!(
        app.status.contains("Capture failed"),
        "the failure surfaced: {}",
        app.status
    );
    assert!(!app.capture_in_flight());
    let _ = std::fs::remove_dir_all(&dir);
}
