//! W37 gates: native drag & drop opens the dropped file (live app.ts:5099
//! drop → loadImageFile). The rebuild handled drops on web only; native
//! dropped files were ignored. Also covers the editable .snagcraft
//! round-trip through the same open path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_kittest::Harness;
use serde_json::json;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w37-{tag}-{}-{:?}",
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
    Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

/// A native dropped file: absolute path, synchronous bytes (egui 0.37's
/// DroppedFile is a trait owned by the integration).
#[derive(Debug)]
struct TestDrop(PathBuf);

impl egui::DroppedFile for TestDrop {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn drop_file(h: &mut Harness<'static, SnagCraftApp>, path: PathBuf) {
    h.input_mut()
        .dropped_files
        .push(Arc::new(TestDrop(path)) as egui::DroppedFileHandle);
    h.run_steps(2);
}

fn write_png(dir: &std::path::Path, name: &str, w: u32, h: u32) -> PathBuf {
    let path = dir.join(name);
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
    img.save(&path).unwrap();
    path
}

#[test]
fn dropping_an_image_opens_it() {
    let dir = scratch("drop-img");
    let mut h = harness(&dir);
    h.run_steps(2);
    let path = write_png(&dir, "dropped.png", 40, 25);
    drop_file(&mut h, path.clone());
    let size = h.state().sess().doc().map(|d| (d.size.w, d.size.h));
    assert_eq!(size, Some((40, 25)), "the dropped image is loaded");
    assert!(
        h.state().status.contains("Opened") && h.state().status.contains("dropped.png"),
        "the status names the dropped file: {}",
        h.state().status
    );
    assert!(!h.state().doc_dirty, "a fresh open is clean");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_an_editable_document_round_trips_the_objects() {
    let dir = scratch("drop-native");
    let mut h = harness(&dir);
    h.run_steps(2);
    // Add a styled caption to the seeded scene, save editable, close, then
    // drop the file back in — the caption must survive with its style.
    let before = h.state().sess().doc().map(|d| d.objects.len()).unwrap_or(0);
    h.state()
        .sess()
        .dispatch(
            "edit.add",
            json!({"object": {"kind": "Text", "id": 0,
                "pos": {"x": 30.0, "y": 40.0}, "text": "round-trip",
                "color": [255, 0, 0, 255], "size": 32.0,
                "bold": true, "italic": false, "family": "Lato"}}),
        )
        .unwrap();
    let path = dir.join("doc.snagcraft");
    h.state()
        .sess()
        .dispatch("doc.save", json!({"path": path.to_string_lossy()}))
        .unwrap();
    h.state().sess().dispatch("doc.close", json!({})).unwrap();
    h.run_steps(1);
    assert!(h.state().sess().doc().is_none(), "closed before the drop");

    drop_file(&mut h, path.clone());
    let objs = h
        .state()
        .sess()
        .doc()
        .map(|d| d.objects.clone())
        .unwrap_or_default();
    assert_eq!(
        objs.len(),
        before + 1,
        "every object survives the round-trip"
    );
    let kept = objs
        .iter()
        .find(
            |o| matches!(o, snagcraft_doc::AnnotObject::Text { text, .. } if text == "round-trip"),
        )
        .expect("the added caption survives");
    match kept {
        snagcraft_doc::AnnotObject::Text {
            family,
            size,
            bold,
            color,
            ..
        } => {
            assert_eq!(family, "Lato");
            assert_eq!(*size, 32.0);
            assert!(*bold);
            assert_eq!(color.r, 255);
        }
        other => panic!("expected Text, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_a_non_image_reports_an_honest_failure() {
    let dir = scratch("drop-bad");
    let mut h = harness(&dir);
    h.run_steps(2);
    let path = dir.join("notes.txt");
    std::fs::write(&path, b"not an image").unwrap();
    let before = h.state().sess().doc().map(|d| (d.size.w, d.size.h));
    drop_file(&mut h, path);
    assert!(
        h.state().status.contains("Open failed"),
        "the failure is reported: {}",
        h.state().status
    );
    assert_eq!(
        h.state().sess().doc().map(|d| (d.size.w, d.size.h)),
        before,
        "the current document is untouched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
