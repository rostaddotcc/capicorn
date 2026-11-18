//! W15 gates: stopping a timelapse converts the run to an MP4 — the live
//! `app.ts:3019-3047` flow (`make_timelapse` at 10 fps into the run folder).
//! The encode runs off-thread through the real `assemble_timelapse`, so these
//! gates exercise worker + poll + status + recent rail end to end.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn harness() -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::new();
    app.settings = SettingsView {
        show_widget: false,
        ..SettingsView::default()
    };
    Harness::builder()
        .with_size(egui::vec2(1200.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w15-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `n` real frames through the engine (odd size on purpose: the pad
/// filter must make the video even, exactly like the engine-level gate).
fn seed_frames(app: &mut SnagCraftApp, dir: &Path, n: u32) {
    app.sess()
        .dispatch("doc.new", serde_json::json!({"w": 33, "h": 21}))
        .unwrap();
    for index in 1..=n {
        let frame = dir.join(format!("snagcraft-timelapse-{index:05}.png"));
        app.sess()
            .dispatch(
                "doc.export",
                serde_json::json!({"path": frame.to_str().unwrap()}),
            )
            .unwrap();
    }
}

/// Run steps until the status leaves the "Building…" state (bounded).
fn wait_for_encode(h: &mut Harness<'static, SnagCraftApp>) {
    for _ in 0..600 {
        h.run_steps(10);
        if !h.state().status.starts_with("Building a timelapse") {
            return;
        }
    }
}

#[test]
fn empty_run_reports_nothing_saved() {
    let mut h = harness();
    h.state_mut().finish_timelapse(0);
    assert_eq!(
        h.state().status,
        "Interval capture stopped before the first shot — nothing was saved."
    );
    assert!(!h.state().timelapse.assembling);
}

#[test]
fn stop_builds_the_video_and_updates_the_recent_rail() {
    if !snagcraft_recording::ffmpeg_available() {
        return; // skip-if-no-ffmpeg (the W13 pattern)
    }
    let dir = scratch("encode");
    let mut h = harness();
    seed_frames(h.state_mut(), &dir, 3);
    {
        let app = h.state_mut();
        app.timelapse.dir = Some(dir.clone());
        app.timelapse.frames = 3;
        app.finish_timelapse(3);
    }
    assert_eq!(h.state().status, "Building a timelapse from 3 shots…");
    assert!(h.state().timelapse.assembling);

    wait_for_encode(&mut h);
    let status = h.state().status.clone();
    assert!(
        status.starts_with("Timelapse saved: "),
        "encode did not finish: {status}"
    );
    assert!(
        status.contains("(3 shots, 10 fps)"),
        "live wording: {status}"
    );
    assert!(!h.state().timelapse.assembling);

    // The MP4 landed in the run folder under the live name pattern.
    let videos: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("SnagCraft-timelapse-"))
                .unwrap_or(false)
                && p.extension().map(|e| e == "mp4").unwrap_or(false)
        })
        .collect();
    assert_eq!(videos.len(), 1, "exactly one video in {}", dir.display());
    let video = &videos[0];
    assert!(std::fs::metadata(video).unwrap().len() > 1000);
    assert!(status.contains(&video.display().to_string()));

    // ffprobe is the evidence: real h264, 3 frames, padded to even.
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,nb_frames,width,height",
            "-of",
            "default=nw=1",
        ])
        .arg(video)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&probe.stdout);
    assert!(text.contains("codec_name=h264"), "ffprobe said: {text}");
    assert!(text.contains("nb_frames=3"), "ffprobe said: {text}");
    assert!(text.contains("width=34"), "padded to even: {text}");
    assert!(text.contains("height=22"), "padded to even: {text}");

    // The recent rail picked the video up (newest first).
    assert!(
        h.state().recent.iter().any(|item| item.path == *video),
        "the finished video must appear in the recent rail"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_encode_keeps_the_shots_and_clears_the_flag() {
    // NoFrames is reported before ffmpeg is ever spawned, so this gate runs
    // even on a host without ffmpeg.
    let dir = scratch("noframes");
    let mut h = harness();
    {
        let app = h.state_mut();
        app.timelapse.dir = Some(dir.clone());
        app.timelapse.frames = 3;
        app.finish_timelapse(3);
    }
    wait_for_encode(&mut h);
    let status = h.state().status.clone();
    assert!(
        status.starts_with("Timelapse failed: "),
        "expected a failure report: {status}"
    );
    assert!(
        status.contains("the 3 shots are still in"),
        "shots-never-lost wording: {status}"
    );
    assert!(status.contains(&dir.display().to_string()));
    assert!(!h.state().timelapse.assembling);

    // The flag cleared, so a later run can assemble again.
    seed_frames(h.state_mut(), &dir, 2);
    if snagcraft_recording::ffmpeg_available() {
        h.state_mut().timelapse.frames = 2;
        h.state_mut().finish_timelapse(2);
        wait_for_encode(&mut h);
        assert!(
            h.state().status.starts_with("Timelapse saved: "),
            "second run must assemble: {}",
            h.state().status
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
