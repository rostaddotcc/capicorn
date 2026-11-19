//! W34 gates: the recording STARTING phase (portal picker open) must not
//! offer Stop/Pause — there is nothing to finalise yet, and the old Stop
//! button printed "Finishing…" while finalising nothing. Live hides the
//! whole rec pill until the take is active (app.ts:2307).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w34-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn harness(dir: &std::path::Path, show_widget: bool) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        show_widget,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    app.widget_expanded = true;
    Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn inject_starting(h: &mut Harness<'static, SnagCraftApp>) {
    let app = h.state_mut();
    // The engine must report a pending start, or poll_record's running
    // fallback clears the injected armed flag before the HUD paints.
    app.sess().seed_record_starting();
    app.rec.armed = true;
    app.rec.active = false;
}

#[test]
fn starting_hud_offers_no_stop_or_pause() {
    let dir = scratch("hud");
    let mut h = harness(&dir, false);
    h.run_steps(2);
    inject_starting(&mut h);
    h.run_steps(2);
    let _ = h.get_by_label_contains("Starting recorder");
    let _ = h.get_by_label_contains("answer the system picker");
    assert!(
        h.query_by_label("Stop recording").is_none(),
        "no Stop while the picker owns the take"
    );
    assert!(h.query_by_label("Pause").is_none(), "no Pause either");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_during_starting_is_honest() {
    let dir = scratch("stop");
    let mut h = harness(&dir, false);
    h.run_steps(2);
    inject_starting(&mut h);
    h.run_steps(1);
    h.state_mut().stop_recording();
    let status = h.state().status.clone();
    assert!(
        status.contains("picker"),
        "the status explains the picker phase: {status}"
    );
    assert!(
        !status.contains("Finishing"),
        "nothing is finishing yet: {status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn starting_widget_shows_starting_not_stop() {
    let dir = scratch("widget");
    let mut h = harness(&dir, true);
    h.run_steps(2);
    inject_starting(&mut h);
    h.run_steps(2);
    assert_eq!(
        h.query_all_by_label("Stop").count(),
        0,
        "the widget's stop square waits for the live take"
    );
    assert_eq!(h.query_all_by_label("Pause").count(), 0);
    h.snapshot("w34_widget_starting");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_take_still_offers_stop_and_pause() {
    let dir = scratch("live");
    let mut h = harness(&dir, false);
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.rec.active = true;
        // Future start instant: the clock saturates to 00:00 (no tick drift).
        app.rec.started_at =
            std::time::Instant::now().checked_add(std::time::Duration::from_secs(600));
    }
    h.run_steps(2);
    let _ = h.get_by_label("Stop recording");
    let _ = h.get_by_label("Pause");
    let _ = std::fs::remove_dir_all(&dir);
}
