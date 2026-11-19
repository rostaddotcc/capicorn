//! W4d live-layer gates: widget Rec, the recording HUD, the frozen-screen
//! region picker, scrolling capture, OCR result window, timelapse dialog,
//! watermark export settings and the autosave dedup helper.
//!
//! Everything runs headless: the portal/encoder paths are exercised only far
//! enough to prove the shell degrades with a real message and never panics.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{
    PickPurpose, SettingsView, SnagCraftApp, Theme, app_ui, apply_theme, clock_label, dedup_path,
};

/// Deterministic shell: no host settings file, no floating widget unless asked.
fn harness(size: egui::Vec2, show_widget: bool) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget,
        ..SettingsView::default()
    };
    app.widget_expanded = true;
    // Never write into the real ~/Pictures from a test.
    app.settings.save_dir = Some(scratch_dir().to_string_lossy().to_string());
    // Pin the tesseract probe: the menu renders identically with or without it.
    app.ocr_available = Some(true);
    Harness::builder()
        .with_size(size)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("snagcraft-w4d-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
fn widget_rec_starts_a_take_without_panicking() {
    let mut h = harness(egui::vec2(1100.0, 760.0), true);
    h.run_steps(3);
    h.get_by_label("Rec").click();
    h.run_steps(2);
    // W18: the take is armed with the live waiting status (format named),
    // or the start failure is classified like the live app.
    let status = h.state().status.clone();
    assert!(
        status.contains("Starting MP4 recording")
            || status.contains("Starting GIF recording")
            || status.contains("Recording failed")
            || status.contains("Recording cancelled")
            || status.contains("Could not start recording"),
        "Rec must either arm a take or report the real failure: {status}"
    );
    // The widget swaps to the stop square once the take is LIVE; during the
    // starting phase (portal picker open) it shows "Starting…" instead (W34).
    if h.state().recording() {
        assert_eq!(
            h.query_all_by_label("»").count(),
            0,
            "the fold control must be gone while a take is in flight"
        );
        if h.state().rec.active {
            // Recording layout: painted stop square + one pause toggle.
            let _ = h.get_by_label("Stop");
            let _ = h.get_by_label("Pause");
            h.get_by_label("Stop").click();
            h.run_steps(1);
            assert!(
                h.state().status.contains("Finishing the recording")
                    || h.state().status.contains("Recording"),
                "Stop must reach the engine: {}",
                h.state().status
            );
        } else {
            // W34: the picker owns the starting phase — no stop/pause yet.
            assert_eq!(h.query_all_by_label("Stop").count(), 0);
            assert_eq!(h.query_all_by_label("Pause").count(), 0);
        }
    }
}

/// Deterministic render of the recording pill: the live state is injected so
/// the snapshot never depends on how fast the headless encoder fails.
#[test]
fn widget_recording_pill_layout() {
    let mut h = harness(egui::vec2(1100.0, 760.0), true);
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.rec.active = true;
        app.rec.started_at = Some(std::time::Instant::now());
    }
    h.run_steps(2);
    let _ = h.get_by_label("Stop");
    // One pause affordance in the pill (the HUD has its own, hence >= 1).
    assert!(h.query_all_by_label("Pause").count() >= 1);
    assert_eq!(
        h.query_all_by_label("»").count(),
        0,
        "the fold control must be gone while a take is live"
    );
    // Park the pointer off the pill so hover shading cannot drift the pixels.
    h.hover_at(egui::pos2(40.0, 740.0));
    h.run_steps(1);
    h.snapshot("w4_widget_recording");
}

#[test]
fn recording_hud_renders_the_injected_take() {
    let mut h = harness(egui::vec2(1100.0, 760.0), false);
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.rec.active = true;
        // A start instant in the future saturates `elapsed()` to zero, so the
        // HUD clock cannot tick across a second boundary mid-snapshot.
        app.rec.started_at =
            std::time::Instant::now().checked_add(std::time::Duration::from_secs(600));
    }
    h.run_steps(2);
    // Elapsed clock + both controls are painted in the top banner.
    let _ = h.get_by_label_contains("Recording 00:00");
    let _ = h.get_by_label("Pause");
    let _ = h.get_by_label("Stop recording");
    h.snapshot("w4_recording_hud");

    // Paused takes say so and offer Resume.
    h.state_mut().rec.paused = true;
    h.run_steps(2);
    let _ = h.get_by_label_contains("Recording paused");
    let _ = h.get_by_label("Resume");

    h.get_by_label("Stop recording").click();
    h.run_steps(1);
    assert!(
        h.state().status.contains("Finishing the recording"),
        "Stop must dispatch record.stop: {}",
        h.state().status
    );
}

#[test]
fn ocr_window_shows_the_text_and_copies_it() {
    let mut h = harness(egui::vec2(1100.0, 760.0), false);
    h.run_steps(2);
    h.state_mut().ocr.text = "SnagCraft extracted this line".to_string();
    h.state_mut().ocr.open = true;
    h.run_steps(2);

    let _ = h.get_by_label("Extracted text");
    // The body is a TextEdit (its value is not an accessible label), so the
    // window also reports the extracted length.
    let _ = h.get_by_label_contains("characters");
    h.snapshot("w4_ocr_window");

    h.get_by_label("Copy text").click();
    h.run_steps(1);
    assert!(
        h.state().status.contains("copied to the clipboard"),
        "Copy text must report: {}",
        h.state().status
    );
    h.get_by_label("Close").click();
    h.run_steps(1);
    assert!(!h.state().ocr.open, "Close dismisses the OCR window");
}

#[test]
fn timelapse_dialog_opens_from_the_capture_menu() {
    let mut h = harness(egui::vec2(1100.0, 760.0), false);
    h.run_steps(3);
    h.get_by_label("Capture").click();
    h.run_steps(2);
    h.get_by_label("Timelapse…").click();
    h.run_steps(2);
    assert!(
        h.state().timelapse.dialog_open,
        "the Capture menu must open the timelapse dialog"
    );
    for label in ["Timelapse", "Every", "Start", "Cancel"] {
        let _ = h.get_by_label(label);
    }
    h.snapshot("w4_timelapse_dialog");
    // Defaults are the documented ones.
    assert_eq!(h.state().timelapse.period_secs, 30);
    h.get_by_label("Cancel").click();
    h.run_steps(1);
    assert!(!h.state().timelapse.dialog_open);
}

#[test]
fn capture_menu_lists_every_live_action() {
    let mut h = harness(egui::vec2(1100.0, 760.0), false);
    h.run_steps(3);
    h.get_by_label("Capture").click();
    h.run_steps(2);
    for label in [
        "Full screen",
        "Region…",
        "Record screen",
        "Record region…",
        "Recording options",
        "Scrolling capture…",
        "Grab text (OCR)",
        "Timelapse…",
        "Open upload page",
    ] {
        let _ = h.get_by_label(label);
    }
    h.snapshot("w4_capture_menu");
}

#[test]
fn watermark_settings_and_export_payload() {
    let mut h = harness(egui::vec2(1100.0, 760.0), false);
    h.run_steps(2);
    h.get_by_label("Settings (Ctrl+,)").click();
    h.run_steps(2);
    for label in ["Watermark", "Corner", "Share", "Upload URL"] {
        let _ = h.get_by_label(label);
    }
    // No watermark while the field is blank.
    assert!(h.state().watermark_value().is_none());

    h.state_mut().settings.watermark_text = "SnagCraft".to_string();
    h.state_mut().settings.watermark_opacity = 45;
    let watermark = h.state().watermark_value().expect("watermark payload");
    assert_eq!(watermark["text"], "SnagCraft");
    assert_eq!(watermark["corner"], "bottom-right");
    assert_eq!(watermark["size"], 24.0);
    assert!((watermark["opacity"].as_f64().unwrap_or(0.0) - 0.45).abs() < 1e-6);
    h.run_steps(1);
    h.snapshot("w4_settings_watermark");

    // A settings save must not wipe the local-only watermark values.
    let mut next = h.state().settings.clone();
    next.default_quality = 80;
    h.state_mut().push_settings(next);
    assert_eq!(h.state().settings.watermark_text, "SnagCraft");
    assert_eq!(h.state().settings.watermark_opacity, 45);

    // Upload stays off until a URL is configured, then the engine owns it.
    assert!(h.state().upload_target().is_none());
    h.state_mut().settings.upload_url = "https://snagcraft.example/upload".to_string();
    assert_eq!(
        h.state().upload_target().as_deref(),
        Some("https://snagcraft.example/upload")
    );
}

#[test]
fn scrolling_hud_renders_the_injected_capture() {
    let mut h = harness(egui::vec2(1100.0, 760.0), false);
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.scroll.active = true;
        app.scroll.index = 4;
        app.scroll.total_height = 1280;
    }
    h.run_steps(2);
    let _ = h.get_by_label_contains("Scroll slowly — captured 1280 px, 4 frames");
    h.get_by_label("Stop scroll").click();
    h.run_steps(1);
    assert!(
        h.state().status.contains("Finishing the scrolling capture")
            || h.state().status.contains("scroll"),
        "Stop must dispatch scroll.stop: {}",
        h.state().status
    );
}

#[test]
fn timelapse_hud_renders_the_injected_run() {
    let mut h = harness(egui::vec2(1100.0, 760.0), false);
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.timelapse.running = true;
        app.timelapse.frames = 7;
    }
    h.run_steps(2);
    let _ = h.get_by_label_contains("Timelapse: 7 frames");
    let _ = h.get_by_label("Stop timelapse");
}

#[test]
fn region_pick_drag_then_enter_dispatches_the_scroll_capture() {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        ..SettingsView::default()
    };
    app.settings.save_dir = Some(scratch_dir().to_string_lossy().to_string());
    // Seed the picker with a frozen "screen" so no portal is needed.
    let mut screen = snagcraft_raster::Surface::new(400, 300);
    for y in 0..300 {
        for x in 0..400 {
            screen.put_pixel(x, y, image::Rgba([30, 60, 90, 255]));
        }
    }
    app.seed_region_pick(PickPurpose::Scroll, screen);
    let mut h = Harness::builder()
        .with_size(egui::vec2(900.0, 700.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app);
    h.run_steps(2);
    assert_eq!(h.state().region_pick(), Some(PickPurpose::Scroll));

    // Drag across the canvas: the crop idiom, in screen pixels.
    let from = egui::pos2(300.0, 250.0);
    let to = egui::pos2(600.0, 450.0);
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(1);
    h.snapshot("w4_region_pick");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().region_pick(), None, "Enter closes the picker");
    // Enter must DISPATCH scroll.start. Proof is environment-robust: locally the
    // portal hangs (armed stays true); on a CI runner with no desktop portal the
    // grab fails fast with the screen-sharing message — which equally proves the
    // dispatch ran. Accept armed, the armed/waiting status, or the portal error.
    let status = h.state().status.clone();
    assert!(
        h.state().scroll.armed
            || status.contains("Scrolling capture")
            || status.contains("system picker")
            || status.contains("Screen sharing"),
        "Enter must dispatch scroll.start: {status}",
    );
}

#[test]
fn dedup_path_never_returns_an_existing_file() {
    let dir = scratch_dir().join(format!("dedup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");

    let first = dedup_path(&dir, "snagcraft-1700000000", "png");
    assert_eq!(
        first.file_name().and_then(|n| n.to_str()),
        Some("snagcraft-1700000000.png")
    );
    std::fs::write(&first, b"1").expect("write first");

    let second = dedup_path(&dir, "snagcraft-1700000000", "png");
    assert_eq!(
        second.file_name().and_then(|n| n.to_str()),
        Some("snagcraft-1700000000-2.png"),
        "-2 on the first collision"
    );
    std::fs::write(&second, b"2").expect("write second");

    let third = dedup_path(&dir, "snagcraft-1700000000", "png");
    assert_eq!(
        third.file_name().and_then(|n| n.to_str()),
        Some("snagcraft-1700000000-3.png"),
        "-3 on the second collision"
    );
    // The original capture is still intact — nothing overwrote it.
    assert_eq!(std::fs::read(&first).expect("read"), b"1");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clock_label_formats_minutes_and_hours() {
    assert_eq!(clock_label(0), "00:00");
    assert_eq!(clock_label(12), "00:12");
    assert_eq!(clock_label(75), "01:15");
    assert_eq!(clock_label(3_600), "1:00:00");
}
