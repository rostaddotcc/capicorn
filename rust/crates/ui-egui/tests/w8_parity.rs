// Tests are exempt from the no-unwrap/no-expect house rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! W8 parity backlog: File → Upload… (native POST flow), Ctrl+. stop, and the
//! shrunk 3-2-1 countdown HUD.
//!
//! The upload tests point at a local mock `TcpListener` — nothing leaves the
//! machine, and they skip when curl is not installed.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn harness(size: egui::Vec2) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView::default();
    app.ocr_available = Some(true);
    // Never write into the real ~/Pictures from a test.
    app.settings.save_dir = Some(scratch_dir().to_string_lossy().to_string());
    Harness::builder()
        .with_size(size)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("snagcraft-w8-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn have_curl() -> bool {
    std::process::Command::new("curl")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// One-shot local endpoint. The body is read until idle because curl streams a
/// multipart body from stdin with chunked encoding.
fn mock_endpoint(response: &'static str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
    use std::io::{Read, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock endpoint");
    let addr = listener.local_addr().expect("mock address");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept upload");
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(1500)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        );
        stream.write_all(reply.as_bytes()).expect("write reply");
        let _ = stream.flush();
        request
    });
    (format!("http://{addr}/upload"), handle)
}

fn open_doc(h: &mut Harness<'static, SnagCraftApp>) {
    h.state_mut()
        .sess()
        .dispatch("doc.new", serde_json::json!({"w": 32, "h": 24}))
        .expect("doc.new");
}

// ---- Feature 2: Ctrl+. stops a running interval capture (timelapse) -------

#[test]
fn ctrl_period_stops_the_running_timelapse() {
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.timelapse.running = true;
    }
    h.run_steps(2);
    assert!(h.state().timelapse.running);

    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Period);
    h.run_steps(2);
    assert!(
        h.state().status.contains("Timelapse stopping"),
        "Ctrl+. must dispatch timelapse.stop: {}",
        h.state().status
    );
}

#[test]
fn ctrl_period_is_inert_without_a_timelapse() {
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(2);
    let before = h.state().status.clone();
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Period);
    h.run_steps(2);
    assert_eq!(h.state().status, before, "no timelapse → no timelapse.stop");
    assert!(!h.state().timelapse.running);
}

// ---- Feature 3: shrunk countdown HUD ------------------------------------

#[test]
fn countdown_hud_renders_a_compact_pill() {
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(2);
    h.state_mut().show_countdown(3);
    h.run_steps(2);

    let _ = h.get_by_label_contains("CAPTURE IN");
    let _ = h.get_by_label("3");
    let _ = h.get_by_label_contains("Screen capture starts automatically");
    // Park the pointer so hover shading cannot drift the pixels.
    h.hover_at(egui::pos2(30.0, 740.0));
    h.run_steps(1);
    h.snapshot("w8_countdown_hud");
}

// ---- Feature 1: File → Upload… ------------------------------------------

#[test]
fn upload_menu_refuses_without_an_image() {
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(2);
    // `seeded()` ships a demo document; close it so there is really nothing.
    h.state_mut()
        .sess()
        .dispatch("doc.close", serde_json::json!({}))
        .expect("doc.close");
    h.run_steps(1);
    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label("Upload…").click();
    h.run_steps(1);
    assert!(
        h.state().status.contains("Nothing to upload"),
        "no image must be refused with a status: {}",
        h.state().status
    );
}

#[test]
fn upload_menu_requires_an_endpoint() {
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(2);
    open_doc(&mut h);
    h.state_mut().settings.upload_url = String::new();
    h.run_steps(1);

    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label("Upload…").click();
    h.run_steps(1);
    assert!(
        h.state().status.contains("Set an upload URL in Settings"),
        "an empty endpoint must be reported: {}",
        h.state().status
    );
}

#[test]
fn upload_menu_posts_the_image_and_copies_the_link() {
    if !have_curl() {
        eprintln!("skipping upload menu test: curl is not installed");
        return;
    }
    let (endpoint, server) = mock_endpoint(r#"{"url":"https://example.com/i/abc.png"}"#);
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(2);
    open_doc(&mut h);
    h.state_mut().settings.upload_url = endpoint;
    h.run_steps(1);

    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label("Upload…").click();
    h.run_steps(2);

    // W21: the upload runs off-thread — wait for the worker in wall time
    // (the mock answers on a read-timeout probe, kittest steps alone don't).
    for _ in 0..600 {
        if !h.state().status.starts_with("Uploading") {
            break;
        }
        h.run_steps(5);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        h.state().status,
        "Uploaded — link copied: https://example.com/i/abc.png",
        "the returned link must become the status"
    );
    let request = server.join().expect("mock endpoint thread");
    assert!(
        request.starts_with(b"POST /upload HTTP/1.1"),
        "not a POST: {}",
        String::from_utf8_lossy(&request[..request.len().min(120)])
    );
    assert!(
        request.windows(4).any(|w| w == b"\x89PNG"),
        "the request body is not a PNG"
    );
}

#[test]
fn upload_menu_reports_an_endpoint_without_a_link() {
    if !have_curl() {
        eprintln!("skipping upload menu test: curl is not installed");
        return;
    }
    let (endpoint, server) = mock_endpoint(r#"{"ok":true}"#);
    let mut h = harness(egui::vec2(1100.0, 760.0));
    h.run_steps(2);
    open_doc(&mut h);
    h.state_mut().settings.upload_url = endpoint;
    h.run_steps(1);

    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label("Upload…").click();
    h.run_steps(2);

    // W21: off-thread upload — wait for the worker in wall time.
    for _ in 0..600 {
        if !h.state().status.starts_with("Uploading") {
            break;
        }
        h.run_steps(5);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        h.state().status.contains("Upload failed") && h.state().status.contains("no link"),
        "a linkless reply must error: {}",
        h.state().status
    );
    server.join().expect("mock endpoint thread");
}
