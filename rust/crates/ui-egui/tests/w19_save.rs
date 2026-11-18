//! W19 gates: the live Save flow (`saveEdited`, app.ts:1862-1885) — guards,
//! `-edited` copy that never overwrites, "Saved copy {filename}" wording,
//! save-point move + Recent pickup, and the three auto-save outcomes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use snagcraft_ui_egui::{SettingsView, SnagCraftApp};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w19-{tag}-{}-{:?}",
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
    // push_settings syncs the ENGINE too — doc.save reads the engine's
    // save_dir, not the UI mirror (assigning app.settings alone leaks writes
    // into the host's real Pictures folder).
    app.push_settings(SettingsView {
        save_dir: Some(dir.to_string_lossy().to_string()),
        show_widget: false,
        ..SettingsView::default()
    });
    app
}

#[test]
fn save_without_image_reports_nothing_to_save() {
    let dir = scratch("nodoc");
    let mut app = app_with_dir(&dir);
    app.sess()
        .dispatch("doc.close", serde_json::json!({}))
        .unwrap();
    assert!(!app.save_now());
    assert_eq!(app.status, "Nothing to save yet — capture a screen first.");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_clean_document_reports_no_changes() {
    let dir = scratch("clean");
    let mut app = app_with_dir(&dir);
    app.doc_dirty = false;
    assert!(!app.save_now());
    assert_eq!(app.status, "No changes to save");
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dirty_save_writes_an_edited_copy_and_moves_the_save_point() {
    let dir = scratch("edited");
    let mut app = app_with_dir(&dir);
    app.doc_dirty = true;
    assert!(app.save_now());
    assert!(
        app.status.starts_with("Saved copy "),
        "live wording: {}",
        app.status
    );
    assert!(app.status.contains("-edited"), "suffix: {}", app.status);
    assert!(!app.doc_dirty, "the save point must move");
    assert!(!app.sess().dirty(), "engine dirty must clear too");

    // Exactly one file landed, named with the suffix, and it is in Recent.
    let files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(name.contains("-edited"), "{name}");
    assert!(name.ends_with(".png"), "{name}");
    assert!(app.recent.iter().any(|item| item.path == files[0]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn repeated_saves_never_overwrite() {
    let dir = scratch("dedup");
    let mut app = app_with_dir(&dir);
    app.doc_dirty = true;
    assert!(app.save_now());
    let first = app.recent[0].path.clone();
    // Dirty again (a new edit) and save once more.
    app.doc_dirty = true;
    assert!(app.save_now());
    let second = app.recent[0].path.clone();
    assert_ne!(first, second, "a second save must be a new copy");
    assert!(first.is_file(), "the first copy survives");
    assert!(second.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_menu_offers_both_save_paths() {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use snagcraft_ui_egui::{Theme, app_ui, apply_theme};

    let dir = scratch("menu");
    let mut app = app_with_dir(&dir);
    app.settings.show_widget = false;
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app);
    h.run_steps(2);
    h.get_by_label("File").click();
    h.run_steps(3);
    // The flat-copy Save exists twice while the menu is open (toolbar twin).
    assert!(h.query_all_by_label("Save").count() >= 2);
    let _ = h.get_by_label("Save As…");
    // W19: the rebuild's editable format stays reachable, explicitly labeled.
    let _ = h.get_by_label("Save editable (.snagcraft)…");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn autosave_reports_all_three_outcomes() {
    let dir = scratch("autosave");
    let mut app = app_with_dir(&dir);
    let surface = app.sess().render_watermarked(None).unwrap();

    // On: a real copy lands.
    let saved = app.autosave_capture(&surface).unwrap();
    let path = saved.expect("auto-save is on by default");
    assert!(path.is_file());

    // Off: Ok(None), nothing new written.
    app.settings.autosave_captures = false;
    assert!(app.autosave_capture(&surface).unwrap().is_none());

    // Failure: the reason surfaces instead of being swallowed. Point the
    // save dir at a regular file so the write cannot succeed.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    app.settings.autosave_captures = true;
    app.settings.save_dir = Some(blocker.to_string_lossy().to_string());
    let err = app.autosave_capture(&surface).unwrap_err();
    assert!(!err.is_empty(), "a failure reason must be reported");
    let _ = std::fs::remove_dir_all(&dir);
}
