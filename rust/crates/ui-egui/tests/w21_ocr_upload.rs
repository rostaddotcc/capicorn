//! W21 gates: OCR + upload flow parity — guards, live result wording, Copy
//! affordance, settings-open on missing endpoint, and a real off-thread upload
//! round trip against a mock HTTP server (worker + poll + link-copy wording).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use egui_kittest::Harness;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn app() -> SnagCraftApp {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        show_widget: false,
        ..SettingsView::default()
    });
    app
}

fn harness(app: SnagCraftApp) -> Harness<'static, SnagCraftApp> {
    Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

#[test]
fn ocr_without_image_says_capture_first() {
    let mut app = app();
    app.sess()
        .dispatch("doc.close", serde_json::json!({}))
        .unwrap();
    app.run_ocr();
    assert_eq!(app.status, "Capture something first.");
    assert!(!app.ocr.busy);
}

#[test]
fn ocr_empty_result_suggests_another_language() {
    let mut app = app();
    app.ocr_finished(Ok(String::new()));
    assert_eq!(
        app.status,
        "No text found in this capture — try another language."
    );
    assert!(!app.ocr.busy);
}

#[test]
fn ocr_success_points_at_copy() {
    let mut app = app();
    app.ocr_finished(Ok("hello".into()));
    assert_eq!(
        app.status,
        "Found 5 characters — Copy puts them on the clipboard."
    );
    assert_eq!(app.ocr.text, "hello");
}

#[test]
fn ocr_error_uses_live_wording() {
    let mut app = app();
    app.ocr_finished(Err("tesseract exploded".into()));
    assert_eq!(app.status, "OCR failed: tesseract exploded");
    assert!(!app.ocr.busy);
}

#[test]
fn upload_without_image_reports_nothing_to_upload() {
    let mut app = app();
    app.sess()
        .dispatch("doc.close", serde_json::json!({}))
        .unwrap();
    app.upload_image_now();
    assert_eq!(
        app.status,
        "Nothing to upload — capture or open an image first."
    );
}

#[test]
fn upload_without_endpoint_opens_settings() {
    let mut app = app();
    app.push_settings(SettingsView {
        upload_url: String::new(),
        ..app.settings.clone()
    });
    app.upload_image_now();
    assert!(app.status.starts_with("Set an upload URL in Settings"));
    assert!(app.settings_open, "the live app opens the panel here");
}

#[test]
fn upload_success_copies_the_link_and_says_so() {
    let mut app = app();
    app.upload_finished(Ok("https://mock.example/shot.png".into()));
    assert_eq!(
        app.status,
        "Uploaded — link copied: https://mock.example/shot.png"
    );
}

#[test]
fn upload_empty_link_is_a_failure() {
    let mut app = app();
    app.upload_finished(Ok("  ".into()));
    assert_eq!(app.status, "Upload failed: the endpoint returned no link.");
}

#[test]
fn upload_error_uses_live_wording() {
    let mut app = app();
    app.upload_finished(Err("the upload did not finish within 30s".into()));
    assert_eq!(
        app.status,
        "Upload failed: the upload did not finish within 30s"
    );
}

/// The io crate's mock pattern: read until curl stalls waiting for the reply
/// (the body is complete), then answer with a plain-text link.
fn mock_upload_server(response: &'static str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock server");
    let addr = listener.local_addr().expect("mock address");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept the upload");
        stream
            .set_read_timeout(Some(Duration::from_millis(1500)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
                Err(_) => break, // curl is waiting for our reply
            }
        }
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        );
        stream.write_all(reply.as_bytes()).expect("write reply");
        let _ = stream.flush();
        request
    });
    (format!("http://{addr}/upload"), handle)
}

#[test]
fn upload_round_trip_runs_off_thread_and_reports_the_link() {
    if std::process::Command::new("curl")
        .arg("--version")
        .output()
        .is_err()
    {
        return; // skip-if-no-curl (the W13 pattern)
    }
    let (endpoint, server) = mock_upload_server("https://mock.example/round-trip.png");
    let mut app = app();
    app.push_settings(SettingsView {
        upload_url: endpoint.clone(),
        ..app.settings.clone()
    });
    let mut h = harness(app);
    h.run_steps(2);
    h.state_mut().upload_image_now();
    assert_eq!(h.state().status, "Uploading…");

    // The worker + poll must land the link without blocking the UI loop.
    // The mock answers in WALL time (read-timeout probe), so give each poll
    // iteration a real slice — kittest steps alone advance no wall clock.
    let mut done = false;
    for _ in 0..600 {
        h.run_steps(5);
        if h.state().status.starts_with("Uploaded — link copied:") {
            done = true;
            break;
        }
        if h.state().status.starts_with("Upload failed:") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(done, "upload did not finish: {}", h.state().status);
    assert_eq!(
        h.state().status,
        "Uploaded — link copied: https://mock.example/round-trip.png"
    );
    let request = server.join().expect("mock server");
    assert!(
        request.windows(4).any(|w| w == b"POST"),
        "the mock must have received a POST"
    );
    assert!(
        request.windows(9).any(|w| w == b"snagcraft"),
        "the PNG must be sent as the snagcraft.png file field"
    );
}
