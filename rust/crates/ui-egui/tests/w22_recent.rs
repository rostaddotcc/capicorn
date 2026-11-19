//! W22 gates: the live vertical film-item — badge + time caption, the active
//! (loaded) tile's accent highlight, and fresh timestamps on push.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w22-{tag}-{}-{:?}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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
    image::DynamicImage::ImageRgba8(img).save(&path).unwrap();
    path
}

fn harness(dir: &std::path::Path) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

#[test]
fn tiles_carry_the_format_badge_and_time_caption() {
    let dir = scratch("caption");
    let shot = write_png(&dir, "caption-shot.png", 40, 30);
    let mut h = harness(&dir);
    {
        let app = h.state_mut();
        app.push_recent(shot);
        app.film_open = true;
    }
    h.run_steps(2);
    // The a11y label is "{name} · {badge} · {time}" — the live caption data.
    let _ = h.get_by_label_contains("caption-shot.png · PNG ·");
    let _ = h.get_by_label_contains("· PNG ·");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn push_recent_stamps_the_tile_with_now() {
    let dir = scratch("ts");
    let mut app = SnagCraftApp::new();
    let before = SystemTime::now();
    app.push_recent(dir.join("stamped.png"));
    let after = SystemTime::now();
    let ts = app.recent[0].ts;
    assert!(ts >= before && ts <= after, "push stamps the tile with now");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_a_tile_marks_it_active_and_new_canvas_clears_it() {
    let dir = scratch("active");
    let first = write_png(&dir, "first.png", 40, 30);
    let second = write_png(&dir, "second.png", 30, 40);
    let mut h = harness(&dir);
    {
        let app = h.state_mut();
        app.push_recent(second.clone());
        app.push_recent(first.clone());
        // W22: pin the time captions — a minute boundary between bless and
        // compare must not drift the snapshot.
        for item in &mut app.recent {
            item.ts = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        }
        app.film_open = true;
    }
    h.run_steps(2);
    assert!(h.state().recent_active.is_none(), "nothing loaded yet");

    h.get_by_label_contains("first.png").click();
    h.run_steps(2);
    assert_eq!(
        h.state().recent_active,
        Some(first.clone()),
        "the opened tile is the live is-active item"
    );

    // The accent ring is painted: snapshot for the visual evidence. Pin the
    // status too — the scratch path carries pid/nanos that drift per run.
    h.state_mut().status = "Opened first.png".into();
    h.run_steps(1);
    h.snapshot("w22_recent_active");

    // A blank canvas is not a recent capture — the highlight clears.
    h.state_mut().create_new_canvas(200.0, 150.0, "white");
    h.run_steps(2);
    assert!(h.state().recent_active.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_marks_the_saved_copy_active() {
    let dir = scratch("save-active");
    let mut h = harness(&dir);
    h.state_mut().doc_dirty = true;
    assert!(h.state_mut().save_now());
    let active = h.state().recent_active.clone();
    assert!(active.is_some(), "the saved copy becomes the loaded recent");
    assert_eq!(active, Some(h.state().recent[0].path.clone()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn video_tiles_keep_the_film_placeholder() {
    let dir = scratch("video");
    let mut h = harness(&dir);
    {
        let app = h.state_mut();
        // No decodable thumb: the tile must fall back to the ▶ placeholder,
        // with the video's extension as the badge.
        app.push_recent(dir.join("take.mp4"));
        app.film_open = true;
    }
    h.run_steps(2);
    let _ = h.get_by_label_contains("take.mp4 · MP4 ·");
    let _ = std::fs::remove_dir_all(&dir);
}
