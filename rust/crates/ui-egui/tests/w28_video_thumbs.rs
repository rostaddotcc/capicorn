// Tests are exempt from the no-unwrap/no-expect house rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! W28 parity: the Recent rail shows a real first-frame thumbnail for video
//! recordings (live `recording_thumb`), extracted off the UI thread by ffmpeg.
//! Images keep the fast synchronous path; only the newest 8 videos get a thumb.

use std::path::PathBuf;

use egui_kittest::Harness;
use snagcraft_ui_egui::{RecentItem, SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w28-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ffmpeg_present() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Encode a 1-second synthetic clip so the thumbnail worker has a real video.
fn make_clip(dir: &std::path::Path) -> PathBuf {
    let clip = dir.join("take.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("testsrc=size=64x48:rate=10:duration=1")
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&clip)
        .stdin(std::process::Stdio::null())
        .status()
        .expect("spawn ffmpeg")
        .success();
    assert!(ok, "fixture clip must encode");
    clip
}

fn harness(app: SnagCraftApp) -> Harness<'static, SnagCraftApp> {
    Harness::builder()
        .with_size(egui::vec2(1000.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

#[test]
fn video_recent_tile_gets_a_first_frame_thumbnail() {
    if !ffmpeg_present() {
        return; // no ffmpeg on this host (e.g. Windows CI) — extraction is moot
    }
    let dir = scratch_dir("tile");
    let clip = make_clip(&dir);
    // W37: pin the caption clock — ts=now drifted the HH:MM caption across
    // minute boundaries (the W22 lesson). Pin the file mtime too, so the
    // folder scan cannot re-stamp it.
    let pinned = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    if let Ok(f) = std::fs::File::open(&clip) {
        let _ = f.set_modified(pinned);
    }

    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    app.recent.push(RecentItem {
        path: clip.clone(),
        name: "take.mp4".into(),
        ts: pinned,
        thumb: None,
    });
    app.film_open = true;

    let mut h = harness(app);
    // The rail requests the thumb on first paint; the worker extracts frame 1
    // off-thread and poll_video_thumbs lands the texture. Advance wall time so
    // the ffmpeg subprocess can finish (kittest steps alone advance no clock).
    let mut loaded = false;
    for _ in 0..100 {
        h.run_steps(2);
        if h.state()
            .recent
            .iter()
            .any(|r| r.path == clip && r.thumb.is_some())
        {
            loaded = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(loaded, "the video tile must gain a first-frame thumbnail");
    h.snapshot("w28_video_thumb");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn image_recent_tile_still_loads_synchronously() {
    let dir = scratch_dir("image");
    // A real PNG on disk so the synchronous image path decodes it.
    let img = snagcraft_raster::Surface::from_pixel(40, 30, image::Rgba([10, 20, 30, 255]));
    let png = dir.join("shot.png");
    snagcraft_io::save(&img, &png, snagcraft_io::ImageFormat::Png, 90).unwrap();
    let pinned = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);

    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    app.recent.push(RecentItem {
        path: png.clone(),
        name: "shot.png".into(),
        ts: pinned,
        thumb: None,
    });
    app.film_open = true;

    let mut h = harness(app);
    h.run_steps(3);
    assert!(
        h.state()
            .recent
            .iter()
            .any(|r| r.path == png && r.thumb.is_some()),
        "an image tile loads its thumbnail synchronously"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
