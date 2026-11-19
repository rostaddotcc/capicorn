//! W20 gates: scrolling-capture flow parity — done summary + Recent pickup,
//! error classification (live scrollErrorText), Cancel (discard) via button /
//! Esc / command, and the HUD's two controls.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{
    SettingsView, SnagCraftApp, Theme, app_ui, apply_theme, scroll_error_status,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w20-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app_with_dir(dir: &std::path::Path) -> SnagCraftApp {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        save_dir: Some(dir.to_string_lossy().to_string()),
        show_widget: false,
        ..SettingsView::default()
    });
    app
}

/// Write a real PNG through the engine so doc.open has something to load.
fn write_png(app: &mut SnagCraftApp, path: &std::path::Path, w: u32, h: u32) {
    app.sess()
        .dispatch("doc.new", serde_json::json!({"w": w, "h": h}))
        .unwrap();
    app.sess()
        .dispatch(
            "doc.export",
            serde_json::json!({"path": path.to_str().unwrap()}),
        )
        .unwrap();
}

#[test]
fn done_reports_the_live_summary_and_lands_in_recent() {
    let dir = scratch("done");
    let mut app = app_with_dir(&dir);
    let file = dir.join("SnagCraft-2026-11-18-12-00-00.png");
    write_png(&mut app, &file, 60, 200);
    app.scroll.armed = true;
    app.scroll.active = true;
    app.scroll.index = 5;

    app.scroll_done(file.to_str().unwrap(), 60, 200);
    assert_eq!(
        app.status,
        format!(
            "Scrolling capture: 60 × 200, 5 frames · saved {}",
            file.file_name().unwrap().to_string_lossy()
        )
    );
    assert!(!app.scroll.armed);
    assert!(!app.scroll.active);
    assert!(app.recent.iter().any(|item| item.path == file));
    // The stitched image is the open document now.
    assert_eq!(
        app.sess().doc().map(|d| (d.size.w, d.size.h)),
        Some((60, 200))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn done_with_no_path_reports_no_image() {
    let dir = scratch("empty");
    let mut app = app_with_dir(&dir);
    app.scroll_done("", 0, 0);
    assert_eq!(app.status, "Scrolling capture finished with no image.");
    assert!(app.recent.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn done_frames_floor_at_one() {
    let dir = scratch("floor");
    let mut app = app_with_dir(&dir);
    let file = dir.join("shot.png");
    write_png(&mut app, &file, 20, 20);
    app.scroll.index = 0;
    app.scroll_done(file.to_str().unwrap(), 20, 20);
    assert!(app.status.contains(", 1 frames ·"), "{}", app.status);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn errors_classify_like_the_live_scroll_error_text() {
    assert_eq!(
        scroll_error_status("the screen is locked"),
        "The screen is locked — unlock it, then start the scrolling capture again."
    );
    assert_eq!(
        scroll_error_status("stitched height would pass the 40000 px cap"),
        "That would pass the 40 000 px height limit. Select a shorter region, or capture the page in two passes."
    );
    assert_eq!(
        scroll_error_status("screen cast portal: Start failed: access denied"),
        "Screen sharing wasn't allowed. Approve the system prompt (or pick this window) and try again."
    );
    assert_eq!(
        scroll_error_status("something odd happened"),
        "Scrolling capture failed: something odd happened"
    );
}

#[test]
fn cancel_scrolling_reports_cancelled_and_clears_flags() {
    let dir = scratch("cancel");
    let mut app = app_with_dir(&dir);
    app.scroll.armed = true;
    app.scroll.active = true;
    // No engine run exists: scroll.cancel answers gracefully, the UI still
    // reports the cancel and clears the flags.
    app.cancel_scrolling();
    assert_eq!(app.status, "Scrolling capture cancelled");
    assert!(!app.scroll.armed);
    assert!(!app.scroll.active);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hud_offers_stop_and_cancel_and_esc_discards() {
    let dir = scratch("hud");
    let app = app_with_dir(&dir);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app);
    h.run_steps(2);
    {
        // `active` only: poll_scroll clears an injected `armed` (W18 idiom).
        let app = h.state_mut();
        app.scroll.active = true;
        app.scroll.index = 4;
        app.scroll.total_height = 1280;
    }
    h.run_steps(2);
    let _ = h.get_by_label("Stop scroll");
    let _ = h.get_by_label("Cancel");

    // Esc discards the run (the live card's Cancel semantics).
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().status, "Scrolling capture cancelled");
    assert!(!h.state().scroll.active);
    let _ = std::fs::remove_dir_all(&dir);
}
