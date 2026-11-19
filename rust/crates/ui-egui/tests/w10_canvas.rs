// Tests are exempt from the no-unwrap/no-expect house rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! W10 parity long-tail: the New-canvas preset dialog (parity 94) and the
//! Resize canvas dialog (parity 95) — dialogs, presets, Esc, and the engine
//! `doc.resize` seam they drive.

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("snagcraft-w10-tests-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Deterministic shell: no host settings file, no floating widget, and a save
/// folder inside the temp dir so the dirty-guard's auto-save never touches
/// ~/Pictures.
fn harness(dir: &std::path::Path) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    };
    Harness::builder()
        .with_size(egui::vec2(900.0, 640.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn doc_size(h: &Harness<'static, SnagCraftApp>) -> Option<(u32, u32)> {
    h.state().sess().doc().map(|d| (d.size.w, d.size.h))
}

/// Ctrl+N and File → New… both open the dialog; a preset button creates that
/// exact canvas through `doc.new`.
#[test]
fn ctrl_n_and_file_menu_open_the_dialog_and_a_preset_creates_its_canvas() {
    let dir = scratch_dir("preset");
    let mut h = harness(&dir);
    h.run_steps(2);

    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::N);
    h.run_steps(2);
    assert!(
        h.state().new_canvas_open,
        "Ctrl+N opens the New-canvas dialog"
    );
    let _ = h.get_by_label("New canvas");

    // Esc closes it again (the dialog owns the keyboard while it is up).
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().new_canvas_open, "Esc closes the dialog");

    // The File menu carries the same entry.
    h.get_by_label("File").click();
    h.run_steps(3);
    h.get_by_label("New…").click();
    h.run_steps(3);
    assert!(h.state().new_canvas_open, "File → New… opens the dialog");

    // Every one of the eight live presets is offered; QUHD (QHD) is one.
    for name in [
        "Default", "HD", "Full HD", "QHD", "4K UHD", "Square", "Story", "Small",
    ] {
        assert!(
            h.query_all_by_label_contains(name).count() >= 1,
            "preset {name} is offered"
        );
    }
    // W23: presets are radios — clicking fills the size fields, Create applies
    // (the live newPresetsEl click handler + syncNewSelection).
    // HD, not QHD: the kittest renderer caps texture sides at 2048 px, so a
    // bigger canvas would fail on the harness's texture upload, not our code.
    h.get_by_label_contains("HD 1280").click();
    h.run_steps(2);
    assert!(
        h.state().new_canvas_open,
        "a preset fills the fields; the dialog stays up"
    );
    assert_eq!(h.state().new_canvas_w, 1280.0);
    assert_eq!(h.state().new_canvas_h, 720.0);
    h.get_by_label("Create").click();
    h.run_steps(2);
    assert_eq!(doc_size(&h), Some((1280, 720)), "HD created its canvas");
    assert!(!h.state().new_canvas_open, "creating closes the dialog");
    assert!(
        h.state().status.starts_with("New canvas 1280×720"),
        "status names the size: {}",
        h.state().status
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The custom width/height fields feed the same `doc.new` seam.
#[test]
fn custom_width_and_height_create_that_canvas() {
    let dir = scratch_dir("custom");
    let mut h = harness(&dir);
    h.run_steps(2);
    h.state_mut().open_new_canvas();
    {
        let app = h.state_mut();
        app.new_canvas_w = 640.0;
        app.new_canvas_h = 360.0;
    }
    h.run_steps(2);
    h.get_by_label("Create").click();
    h.run_steps(2);
    assert_eq!(doc_size(&h), Some((640, 360)));
    assert!(!h.state().doc_dirty, "a fresh canvas starts clean");

    // Hostile values are clamped to the live 16..=8192 window, not passed on.
    h.state_mut().create_new_canvas(1.0, 99_999.0, "white");
    assert_eq!(doc_size(&h), Some((16, 8192)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The dirty guard: annotations present → the document is saved before the
/// blank canvas replaces it (live `newCanvas()` semantics, no prompt).
#[test]
fn a_dirty_document_is_saved_before_the_new_canvas_replaces_it() {
    let dir = scratch_dir("dirty");
    let mut h = harness(&dir);
    h.run_steps(2);
    {
        // Point the engine's save folder at the temp dir so the auto-save from
        // the dirty guard lands there, never in ~/Pictures.
        let _ = h.state_mut().sess().dispatch(
            "settings.set",
            serde_json::json!({"settings": {"save_dir": dir.to_string_lossy()}}),
        );
    }
    {
        let app = h.state_mut();
        app.doc_dirty = true;
        assert!(app.sess().doc().is_some_and(|d| !d.objects.is_empty()));
    }
    h.state_mut().open_new_canvas();
    h.run_steps(2);
    // W23: preset fills the fields, Create applies (radio semantics).
    h.get_by_label_contains("Small").click();
    h.run_steps(2);
    h.get_by_label("Create").click();
    h.run_steps(2);
    assert_eq!(doc_size(&h), Some((800, 600)), "Small preset applied");
    assert!(!h.state().doc_dirty, "the replace leaves a clean document");
    // The auto-save ran: the saved file became the newest recent and exists.
    let saved = h.state().recent.first().map(|r| r.path.clone());
    assert!(
        saved.is_some_and(|p| p.is_file()),
        "the dirty doc was saved first"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The toolbar Resize… dialog prefills the current dims and Apply drives
/// `doc.resize`; annotations scale with the canvas.
#[test]
fn resize_dialog_prefills_applies_and_scales_annotations() {
    let dir = scratch_dir("resize");
    let mut h = harness(&dir);
    h.run_steps(2);
    assert_eq!(doc_size(&h), Some((900, 600)), "seeded canvas");

    let before = h
        .state()
        .sess()
        .doc()
        .and_then(|d| d.objects.first().map(|o| o.bounds()))
        .expect("seeded app has annotations");

    h.get_by_label("File").click();
    h.run_steps(3);
    h.get_by_label("Resize…").click();
    h.run_steps(2);
    {
        let app = h.state();
        assert!(app.resize_open, "the File menu item opens the dialog");
        assert_eq!(
            (app.resize_w, app.resize_h),
            (900.0, 600.0),
            "dims prefilled"
        );
    }
    let _ = h.get_by_label("Resize image");
    {
        let app = h.state_mut();
        app.resize_w = 450.0;
        app.resize_h = 300.0;
    }
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(2);

    assert_eq!(doc_size(&h), Some((450, 300)), "doc.resize ran");
    assert!(!h.state().resize_open, "Apply closes the dialog");
    assert!(h.state().doc_dirty, "a resize is an unsaved edit");
    assert!(
        h.state().status.starts_with("Resized to 450 × 300"),
        "status: {}",
        h.state().status
    );
    let after = h
        .state()
        .sess()
        .doc()
        .and_then(|d| d.objects.first().map(|o| o.bounds()))
        .expect("annotations survive the resize");
    assert!(
        (after.x - before.x * 0.5).abs() < 0.6 && (after.y - before.y * 0.5).abs() < 0.6,
        "annotation geometry scaled with the canvas: {before:?} -> {after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Esc closes the Resize dialog and leaves the canvas alone.
#[test]
fn resize_dialog_esc_closes_without_changing_the_canvas() {
    let dir = scratch_dir("resize-esc");
    let mut h = harness(&dir);
    h.run_steps(2);
    h.get_by_label("File").click();
    h.run_steps(3);
    h.get_by_label("Resize…").click();
    h.run_steps(2);
    assert!(h.state().resize_open);
    h.state_mut().resize_w = 100.0;
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().resize_open, "Esc closes the dialog");
    assert_eq!(doc_size(&h), Some((900, 600)), "the canvas is untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

/// One visual check of each dialog, as the wave brief asks.
#[test]
fn canvas_dialogs_snapshot() {
    let dir = scratch_dir("snapshot");
    let mut h = harness(&dir);
    h.run_steps(2);
    h.state_mut().open_new_canvas();
    h.run_steps(2);
    h.snapshot("w10_new_canvas");

    h.state_mut().new_canvas_open = false;
    h.state_mut().open_resize();
    h.run_steps(2);
    h.snapshot("w10_resize");
    let _ = std::fs::remove_dir_all(&dir);
}
