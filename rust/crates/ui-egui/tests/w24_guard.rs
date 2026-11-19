//! W24 gates: the live close-guard hierarchy — "Save & close" (primary) saves
//! before closing, "Discard" (ghost-danger) closes without saving, Cancel stays.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w24-{tag}-{}-{:?}",
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
    app.doc_dirty = true;
    app.confirm_close = true;
    Harness::builder()
        .with_size(egui::vec2(1000.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

#[test]
fn save_and_close_saves_the_copy_first() {
    let dir = scratch("save");
    let mut h = harness(&dir);
    h.run_steps(2);
    let _ = h.get_by_label("Save & close");
    h.get_by_label("Save & close").click();
    h.run_steps(2);
    assert!(!h.state().confirm_close, "the guard is answered");
    assert!(h.state().force_quit, "Save & close proceeds to quit");
    // The dirty doc was saved as a copy before closing (live saveEdited).
    assert!(
        h.state().recent.first().is_some_and(|r| r.path.is_file()),
        "the unsaved work landed on disk"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_closes_without_saving() {
    let dir = scratch("discard");
    let mut h = harness(&dir);
    h.run_steps(2);
    let _ = h.get_by_label("Discard");
    h.get_by_label("Discard").click();
    h.run_steps(2);
    assert!(!h.state().confirm_close);
    assert!(h.state().force_quit, "Discard proceeds to quit");
    assert!(
        h.state().recent.is_empty(),
        "Discard must NOT write the work to disk"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_keeps_the_document_open() {
    let dir = scratch("cancel");
    let mut h = harness(&dir);
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(!h.state().confirm_close, "the guard dismisses");
    assert!(!h.state().force_quit, "Cancel must not quit");
    assert!(h.state().doc_dirty, "the work is still there");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn guard_snapshot_shows_the_hierarchy() {
    let dir = scratch("snap");
    let mut h = harness(&dir);
    h.run_steps(2);
    h.snapshot("w24_close_guard");
    let _ = std::fs::remove_dir_all(&dir);
}
