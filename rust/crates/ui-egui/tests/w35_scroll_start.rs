//! W35 gates: the scrolling-capture STARTING phase (portal picker open)
//! captures nothing yet — no "scroll slowly" guidance, no Stop (nothing to
//! stitch). Cancel still works: the engine remembers a pending cancel and
//! applies it when the handle lands (W20). Mirrors the W34 recording fix.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w35-{tag}-{}-{:?}",
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
        .with_size(egui::vec2(1100.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn inject_starting(h: &mut Harness<'static, SnagCraftApp>) {
    let app = h.state_mut();
    // The engine must report a pending start, or poll_scroll's running
    // fallback clears the injected armed flag before the HUD paints.
    app.sess().seed_scroll_starting();
    app.scroll.armed = true;
    app.scroll.active = false;
}

#[test]
fn starting_hud_offers_cancel_not_stop() {
    let dir = scratch("hud");
    let mut h = harness(&dir);
    h.run_steps(2);
    inject_starting(&mut h);
    h.run_steps(2);
    let _ = h.get_by_label_contains("Starting scrolling capture");
    assert!(
        h.query_by_label("Stop scroll").is_none(),
        "nothing to stitch while the picker owns the capture"
    );
    assert!(
        h.query_by_label_contains("Scroll slowly").is_none(),
        "no scroll guidance before the capture runs"
    );
    let _ = h.get_by_label("Cancel");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_during_starting_is_honest() {
    let dir = scratch("stop");
    let mut h = harness(&dir);
    h.run_steps(2);
    inject_starting(&mut h);
    h.run_steps(1);
    h.state_mut().stop_scrolling();
    let status = h.state().status.clone();
    assert!(
        status.contains("picker"),
        "explains the picker phase: {status}"
    );
    assert!(
        !status.contains("Finishing"),
        "nothing is finishing: {status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_during_starting_clears_the_capture() {
    let dir = scratch("cancel");
    let mut h = harness(&dir);
    h.run_steps(2);
    inject_starting(&mut h);
    h.run_steps(1);
    h.state_mut().cancel_scrolling();
    h.run_steps(1);
    assert!(!h.state().scroll.armed, "cancel disarms");
    assert!(
        h.state().status.contains("cancelled"),
        "cancel reports: {}",
        h.state().status
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn active_scroll_still_offers_stop() {
    let dir = scratch("active");
    let mut h = harness(&dir);
    h.run_steps(2);
    {
        // active only: poll_scroll clears an injected armed (W18 idiom).
        let app = h.state_mut();
        app.scroll.active = true;
        app.scroll.index = 4;
        app.scroll.total_height = 1280;
    }
    h.run_steps(2);
    let _ = h.get_by_label("Stop scroll");
    let _ = h.get_by_label("Cancel");
    let _ = h.get_by_label_contains("Scroll slowly");
    let _ = std::fs::remove_dir_all(&dir);
}
