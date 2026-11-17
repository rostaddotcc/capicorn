// Tests are exempt from the no-unwrap/no-expect house rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! W9 save & recent UX: the Recents footer (count badge, tiles, click-to-open,
//! toggle, one-shot folder scan) and the Save As seam.

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snagcraft-w9-tests-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A real PNG on disk so the tile can decode a thumbnail.
fn write_png(dir: &std::path::Path, name: &str, w: u32, h: u32) -> PathBuf {
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([
            (x * 255 / w.max(1)) as u8,
            (y * 255 / h.max(1)) as u8,
            180,
            255,
        ])
    });
    let path = dir.join(name);
    image::DynamicImage::ImageRgba8(img)
        .save(&path)
        .expect("write png");
    path
}

/// Deterministic shell: no host settings file, no floating widget, and a save
/// folder inside the temp dir so the startup scan never touches ~/Pictures.
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

/// The list rules: newest first, deduped by path, capped at 12.
#[test]
fn push_recent_is_newest_first_deduped_and_capped() {
    let dir = scratch_dir("list");
    let mut app = SnagCraftApp::new();
    let paths: Vec<PathBuf> = (0..14).map(|i| dir.join(format!("shot-{i}.png"))).collect();
    for path in &paths {
        app.push_recent(path.clone());
    }
    assert_eq!(app.recent_count(), 12, "RECENT_LIMIT caps the strip");
    assert_eq!(
        app.recent[0].path, paths[13],
        "the last push is the newest tile"
    );
    assert!(!app.recent.iter().any(|item| item.path == paths[0]));

    // Pushing an existing path moves it to the front without duplicating it.
    let existing = paths[8].clone();
    app.push_recent(existing.clone());
    assert_eq!(app.recent[0].path, existing);
    assert_eq!(
        app.recent.iter().filter(|i| i.path == existing).count(),
        1,
        "no duplicate tiles for one file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The strip renders a count badge and tiles; clicking a tile opens that file.
#[test]
fn filmstrip_renders_count_and_click_opens_the_file() {
    let dir = scratch_dir("click");
    let wide = write_png(&dir, "wide-shot.png", 96, 32);
    let tall = write_png(&dir, "tall-shot.png", 24, 60);
    let mut h = harness(&dir);
    {
        let app = h.state_mut();
        app.push_recent(tall.clone());
        app.push_recent(wide.clone());
        app.film_open = true;
    }
    h.run_steps(2);

    // Toolbar chip carries the count badge, strip shows its own total.
    let _ = h.get_by_label("Recent (2)");
    let _ = h.get_by_label("Recent");

    // Newest first: wide-shot is the first tile.
    h.get_by_label_contains("wide-shot.png").click();
    h.run_steps(2);
    let size = h.state().sess().doc().map(|d| (d.size.w, d.size.h));
    assert_eq!(size, Some((96, 32)), "clicking a tile ran doc.open");

    // The other tile opens its own file, not the first one again.
    h.get_by_label_contains("tall-shot.png").click();
    h.run_steps(2);
    let size = h.state().sess().doc().map(|d| (d.size.w, d.size.h));
    assert_eq!(size, Some((24, 60)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The chip toggles the strip; the strip's Hide button closes it too.
#[test]
fn toggle_chip_shows_and_hides_the_strip() {
    let dir = scratch_dir("toggle");
    let shot = write_png(&dir, "toggle-shot.png", 40, 40);
    let mut h = harness(&dir);
    {
        let app = h.state_mut();
        app.push_recent(shot);
        assert!(!app.film_open, "the strip starts hidden");
    }
    h.run_steps(2);
    assert!(h.query_by_label("Hide").is_none());

    h.get_by_label("Recent (1)").click();
    h.run_steps(2);
    assert!(h.state().film_open);
    let _ = h.get_by_label("Hide");

    h.get_by_label("Hide").click();
    h.run_steps(2);
    assert!(!h.state().film_open);
    assert!(h.query_by_label("Hide").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The startup scan only runs once and merges the folder's images underneath
/// whatever this session already pushed.
#[test]
fn startup_scan_runs_once_and_keeps_session_recents_first() {
    let dir = scratch_dir("scan");
    let old = write_png(&dir, "folder-old.png", 20, 20);
    let mut h = harness(&dir);
    let fresh = write_png(&dir, "session-new.png", 30, 30);
    {
        let app = h.state_mut();
        app.push_recent(fresh.clone());
    }
    h.run_steps(3);
    {
        let app = h.state();
        assert_eq!(app.recent[0].path, fresh, "in-session capture stays first");
        assert!(
            app.recent.iter().any(|item| item.path == old),
            "the save folder's older shot was merged in"
        );
    }
    // A second frame must not re-scan (the scan is once per session).
    let before = h.state().recent_count();
    h.run_steps(2);
    assert_eq!(h.state().recent_count(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save As seam: the dialog's target path is what `doc.save_as` writes, the
/// document goes clean and the file becomes the newest recent.
#[test]
fn save_as_seam_writes_the_path_and_prepends_a_recent() {
    let dir = scratch_dir("saveas");
    let mut h = harness(&dir);
    h.state_mut()
        .sess()
        .dispatch("doc.new", serde_json::json!({"w": 42, "h": 18}))
        .unwrap();
    h.state_mut().doc_dirty = true;
    let target = dir.join("chosen-save-as.png");
    assert!(h.state_mut().save_as_to(target.clone()));
    assert!(target.is_file(), "the seam wrote the chosen path");
    let img = snagcraft_io::open(&target).unwrap();
    assert_eq!(img.dimensions(), (42, 18));
    assert!(h.state().status.starts_with("Saved "));
    assert_eq!(h.state().recent[0].path, target);
    assert!(!h.state().doc_dirty, "save clears the dirty flag");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The File menu offers the two save entries next to Upload….
#[test]
fn file_menu_offers_save_and_save_as() {
    let dir = scratch_dir("menu");
    let mut h = harness(&dir);
    h.run_steps(2);
    h.get_by_label("File").click();
    h.run_steps(3);
    let _ = h.get_by_label("Save As…");
    assert!(
        h.query_all_by_label("Save").count() >= 2,
        "toolbar Save plus the File menu's Save"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// One visual check of the footer, as the wave brief asks.
#[test]
fn recent_strip_snapshot() {
    let dir = scratch_dir("snapshot");
    write_png(&dir, "SnagCraft-a.png", 96, 48);
    write_png(&dir, "SnagCraft-b.png", 48, 96);
    write_png(&dir, "SnagCraft-c.png", 64, 64);
    let mut h = harness(&dir);
    {
        let app = h.state_mut();
        for name in ["SnagCraft-c.png", "SnagCraft-b.png", "SnagCraft-a.png"] {
            app.push_recent(dir.join(name));
        }
        app.film_open = true;
    }
    h.run_steps(2);
    h.snapshot("w9_recent_strip");
    let _ = std::fs::remove_dir_all(&dir);
}
