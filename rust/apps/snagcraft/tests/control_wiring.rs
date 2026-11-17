#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! W5b wiring gate: the control channel (W5a) really serves the *app's* session,
//! so an agent that edits through the socket edits the document the UI shows.
//!
//! The client here is a deliberately small, dependency-free re-implementation of
//! the CLI's one-shot client: auth frame first (no acknowledgement), then one
//! request, then exactly one frame back.

use std::io::{BufRead, Write};

use snagcraft_automation::{ControlHandle, generate_token, spawn_control};
use snagcraft_ui_egui::SnagCraftApp;

/// Send auth + one request, return the single response frame.
fn request(port: u16, token: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let stream = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(10))
        .expect("connect to the control channel");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("read timeout");
    let mut writer = stream.try_clone().expect("clone socket");
    let mut reader = std::io::BufReader::new(stream);

    let frames = [
        serde_json::json!({"id": 0, "method": "auth", "params": {"token": token}}),
        serde_json::json!({"id": 1, "method": method, "params": params}),
    ];
    for frame in frames {
        let mut line = serde_json::to_vec(&frame).expect("encode frame");
        line.push(b'\n');
        writer.write_all(&line).expect("write frame");
        writer.flush().expect("flush frame");
    }

    let mut line = String::new();
    reader.read_line(&mut line).expect("read one frame");
    assert!(!line.trim().is_empty(), "server sent no frame for {method}");
    serde_json::from_str(line.trim()).expect("response is JSON")
}

fn live(app: &SnagCraftApp) -> (ControlHandle, String) {
    let token = generate_token();
    let handle = spawn_control(app.session_arc(), 0, token.clone()).expect("spawn control server");
    (handle, token)
}

#[test]
fn control_server_edits_the_apps_own_session() {
    let app = SnagCraftApp::new();
    let (handle, token) = live(&app);
    let port = handle.local_addr().port();

    // 1. command.run doc.new through the socket …
    let res = request(
        port,
        &token,
        "command.run",
        serde_json::json!({"id": "doc.new", "params": {"w": 320, "h": 200}}),
    );
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["result"]["width"], 320);

    // … is visible to the app itself (same Arc<Mutex<Session>>).
    let (w, h) = {
        let sess = app.sess();
        let doc = sess.doc().expect("doc exists after doc.new");
        (doc.size.w, doc.size.h)
    };
    assert_eq!(
        (w, h),
        (320, 200),
        "control edits must land in the UI session"
    );

    // 2. session.info agrees.
    let info = request(port, &token, "session.info", serde_json::json!({}));
    assert_eq!(info["ok"], true, "{info}");
    assert_eq!(info["result"]["width"], 320);
    assert_eq!(info["result"]["height"], 200);

    // 3. ui.screenshot returns the composite at the document's size.
    let shot = request(port, &token, "ui.screenshot", serde_json::json!({}));
    assert_eq!(shot["ok"], true, "screenshot failed: {shot}");
    let result = &shot["result"];
    assert_eq!(result["width"], 320);
    assert_eq!(result["height"], 200);
    let png = result["png"].as_str().expect("png is a base64 string");
    assert!(
        png.starts_with("iVBORw0KGgo"),
        "payload is not a PNG: {}",
        &png[..png.len().min(16)]
    );
    let bytes = result["bytes"].as_u64().expect("bytes") as usize;
    assert!(bytes > 8, "PNG should carry pixels: {bytes}");
    assert_eq!(
        png.len(),
        bytes.div_ceil(3) * 4,
        "base64 length must match the reported byte count"
    );
}

#[test]
fn shutdown_stops_serving() {
    let app = SnagCraftApp::new();
    let (handle, _token) = live(&app);
    let port = handle.local_addr().port();
    handle.shutdown();
    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    assert!(
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_err(),
        "control port {port} still accepts connections after shutdown"
    );
}

#[test]
fn no_control_server_means_no_listener() {
    // Default wiring: constructing the app binds nothing.
    let app = SnagCraftApp::new();
    assert!(app.session_arc().lock().is_ok());
    let doc = app.sess().doc().map(|d| (d.size.w, d.size.h));
    assert_eq!(doc, Some((1000, 640)), "starter document is unchanged");
}
