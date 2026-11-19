//! W30 gates: the region-pick freeze must not show SnagCraft itself. Live
//! grabs the freeze through `capture_screenshot` (hide/settle/restore,
//! app.ts:2548/2732); the rebuild hid nothing before W30. The hide lasts only
//! until the freeze lands — the picker overlay paints in the main window, so
//! the windows must be back for the drag, on success AND failure.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use snagcraft_ui_egui::{PickPurpose, SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w30-{tag}-{}-{:?}",
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

/// End-to-end through the real worker: the windows hide at pick start and
/// come back when the grab ends (a portal refusal on CI, the 15 s timeout on
/// a locked build host — never a freeze without an unlocked desktop).
#[test]
fn region_pick_hides_for_the_freeze_and_restores_on_the_outcome() {
    let dir = scratch("e2e");
    let mut h = harness(&dir);
    h.run_steps(1);
    h.state_mut().start_region_pick(PickPurpose::Scroll);
    assert!(
        h.state().hidden && h.state().pick_hidden,
        "the windows hide for the freeze grab"
    );
    assert!(h.state().region_pick().is_some(), "the picker is armed");

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut settled = false;
    while Instant::now() < deadline {
        h.run_steps(1);
        let st = h.state();
        if st.pick_has_surface() || st.status.starts_with("Region pick failed") {
            settled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        settled,
        "the freeze grab reached an outcome: {}",
        h.state().status
    );
    assert!(!h.state().hidden, "windows restored for the drag");
    assert!(!h.state().pick_hidden);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The hidden path: while the root is hidden only `logic` runs (via
/// poll_media), so it must land the freeze AND bring the windows back.
#[test]
fn logic_lands_the_freeze_and_restores_the_windows() {
    let dir = scratch("logic-ok");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(&dir));
    let ctx = egui::Context::default();
    app.logic(&ctx); // captures main_ctx

    let tx = app.seed_pick_channel(PickPurpose::Record);
    app.hide_for_pick();
    assert!(app.hidden && app.pick_hidden);

    tx.send(Ok(snagcraft_raster::filled(
        snagcraft_geom::Size::new(8, 8),
        snagcraft_color::Rgba::WHITE,
    )))
    .unwrap();
    app.logic(&ctx);
    assert!(!app.hidden && !app.pick_hidden, "windows back for the drag");
    assert!(app.pick_has_surface(), "the freeze landed on the picker");
    assert!(
        app.status.contains("Drag a rectangle"),
        "the picker prompts for the drag: {}",
        app.status
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failed freeze restores the windows too and clears the picker.
#[test]
fn logic_restores_on_a_failed_freeze() {
    let dir = scratch("logic-err");
    let mut app = SnagCraftApp::seeded();
    app.push_settings(settings(&dir));
    let ctx = egui::Context::default();
    app.logic(&ctx);

    let tx = app.seed_pick_channel(PickPurpose::Scroll);
    app.hide_for_pick();
    assert!(app.hidden && app.pick_hidden);

    tx.send(Err("portal: refused".into())).unwrap();
    app.logic(&ctx);
    assert!(!app.hidden && !app.pick_hidden, "restored on failure");
    assert!(app.region_pick().is_none(), "the picker is cleared");
    assert!(
        app.status.contains("Region pick failed"),
        "the failure surfaced: {}",
        app.status
    );
    let _ = std::fs::remove_dir_all(&dir);
}
