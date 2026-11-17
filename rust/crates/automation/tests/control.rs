#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Offline integration tests for the loopback JSON control channel.
//!
//! Every test binds `127.0.0.1:0`, i.e. an ephemeral loopback port — no fixed
//! ports, no network, no clock dependency. The MCP stdio surface is covered by
//! the in-memory duplex protocol test in `src/mcp.rs` (drives a real rmcp service
//! through `initialize` + `tools/list` + `tools/call` without a subprocess).
//!
//! The control protocol deliberately sends **no** ack frame after `auth`
//! succeeds, so the first frame read after authenticating is always the response
//! to the request that followed it.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use snagcraft_automation::{
    AutomationError, ControlHandle, generate_token, read_token_file, spawn_control,
    write_token_file,
};
use snagcraft_engine::Session;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

fn session_with_doc(w: u32, h: u32) -> Arc<Mutex<Session>> {
    let mut session = Session::new();
    session
        .dispatch("doc.new", json!({"w": w, "h": h}))
        .expect("doc.new");
    Arc::new(Mutex::new(session))
}

/// Start a control server on an ephemeral loopback port.
fn start(session: Arc<Mutex<Session>>) -> (ControlHandle, String) {
    let token = generate_token();
    let handle = spawn_control(Arc::clone(&session), 0, token.clone()).expect("spawn_control");
    assert_ne!(
        handle.local_addr().port(),
        0,
        "port 0 must report a real port"
    );
    assert!(handle.local_addr().ip().is_loopback());
    (handle, token)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "snagcraft-automation-{}-{}-{name}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    p
}

struct Client {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Client {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let reader = BufReader::new(stream.try_clone().expect("clone"));
        Self { stream, reader }
    }

    fn send_raw(&mut self, line: &str) {
        self.stream.write_all(line.as_bytes()).expect("write");
        self.stream.write_all(b"\n").expect("newline");
        self.stream.flush().expect("flush");
    }

    fn send(&mut self, frame: Value) {
        self.send_raw(&frame.to_string());
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).expect("read frame");
        assert!(n > 0, "server closed the connection instead of replying");
        serde_json::from_str(&line).expect("response is JSON")
    }

    /// True when the server closed the connection (EOF).
    fn at_eof(&mut self) -> bool {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read to EOF") == 0
    }

    fn auth(&mut self, token: &str) {
        self.send(json!({"id": 0, "method": "auth", "params": {"token": token}}));
        // No ack frame on success; the caller's next frame gets the next response.
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"id": id, "method": method, "params": params}));
        self.recv()
    }
}

#[test]
fn wrong_token_is_rejected_and_the_socket_closes() {
    let (_handle, token) = start(session_with_doc(16, 16));
    let mut client = Client::connect(_handle.local_addr());

    let wrong = generate_token();
    assert_ne!(wrong, token);
    client.send(json!({"id": 7, "method": "auth", "params": {"token": wrong}}));
    let res = client.recv();
    assert_eq!(res["id"], 7);
    assert_eq!(res["ok"], false);
    assert_eq!(res["error"], "authentication required");
    assert!(client.at_eof(), "server must close after a failed auth");
}

#[test]
fn missing_auth_frame_is_rejected() {
    let (handle, _token) = start(session_with_doc(16, 16));
    let mut client = Client::connect(handle.local_addr());

    client.send(json!({"id": 1, "method": "commands.list", "params": {}}));
    let res = client.recv();
    assert_eq!(res["ok"], false);
    assert_eq!(res["error"], "authentication required");
    assert!(client.at_eof());
}

#[test]
fn commands_list_exposes_the_whole_registry() {
    let (handle, token) = start(session_with_doc(16, 16));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    let res = client.call(1, "commands.list", json!({}));
    assert_eq!(res["ok"], true, "{res}");
    let commands = res["result"].as_array().expect("array of commands");
    assert_eq!(
        commands.len(),
        42,
        "W7 engine registry has 42 commands (41 + timelapse.assemble)"
    );
    assert_eq!(commands.len(), snagcraft_engine::registry().len());
    for entry in commands {
        assert!(entry["id"].is_string(), "{entry}");
        assert!(entry["label"].is_string(), "{entry}");
        assert!(entry["params"].is_string(), "{entry}");
    }
    let ids: Vec<&str> = commands.iter().filter_map(|c| c["id"].as_str()).collect();
    for expected in ["doc.new", "doc.export", "edit.add", "session.info"] {
        assert!(ids.contains(&expected), "registry lost {expected}");
    }
}

#[test]
fn command_run_creates_edits_and_exports_a_document() {
    let (handle, token) = start(session_with_doc(64, 48));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    let new = client.call(
        1,
        "command.run",
        json!({"id": "doc.new", "params": {"w": 64, "h": 48}}),
    );
    assert_eq!(new["ok"], true, "{new}");
    assert_eq!(new["result"]["width"], 64);
    assert_eq!(new["result"]["height"], 48);

    let added = client.call(
        2,
        "command.run",
        json!({"id": "edit.add", "params": {"object": {
            "kind": "Shape", "id": 0, "shape": "Rect",
            "rect": {"x": 4.0, "y": 4.0, "w": 20.0, "h": 12.0},
            "color": [220, 40, 40, 255], "width": 2.0
        }}}),
    );
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(added["result"]["id"], 1);

    let inspected = client.call(3, "doc.inspect", json!({}));
    assert_eq!(inspected["ok"], true, "{inspected}");
    assert_eq!(inspected["result"]["size"]["w"], 64);
    assert_eq!(inspected["result"]["objects"].as_array().unwrap().len(), 1);

    let info = client.call(4, "session.info", json!({}));
    assert_eq!(info["ok"], true, "{info}");
    assert_eq!(info["result"]["has_doc"], true);
    assert_eq!(info["result"]["objects"], 1);

    let out = temp_path("export.png");
    let exported = client.call(
        5,
        "command.run",
        json!({"id": "doc.export", "params": {"path": out.to_str().unwrap(), "format": "png"}}),
    );
    assert_eq!(exported["ok"], true, "{exported}");
    assert!(out.exists(), "doc.export must write {}", out.display());
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(PNG_MAGIC), "exported file is not a PNG");
    let surface = snagcraft_io::decode(&bytes).unwrap();
    assert_eq!(surface.dimensions(), (64, 48));
    let _ = std::fs::remove_file(&out);

    // Errors surface as error frames, not dropped connections.
    let no_doc = client.call(6, "command.run", json!({"id": "doc.close"}));
    assert_eq!(no_doc["ok"], true, "{no_doc}");
    let bad = client.call(7, "doc.inspect", json!({}));
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error"], "no document open");
}

#[test]
fn ui_screenshot_returns_a_valid_png_base64() {
    let (handle, token) = start(session_with_doc(64, 48));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    let res = client.call(1, "ui.screenshot", json!({}));
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["result"]["width"], 64);
    assert_eq!(res["result"]["height"], 48);
    let png = BASE64
        .decode(res["result"]["png"].as_str().expect("png is a string"))
        .expect("valid base64");
    assert!(png.starts_with(PNG_MAGIC), "not a PNG payload");
    let surface = snagcraft_io::decode(&png).expect("PNG decodes");
    assert_eq!(surface.dimensions(), (64, 48));
}

#[test]
fn malformed_frames_error_but_keep_the_connection() {
    let (handle, token) = start(session_with_doc(16, 16));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    client.send_raw("this is not json");
    let res = client.recv();
    assert_eq!(res["ok"], false);
    assert!(
        res["error"]
            .as_str()
            .unwrap()
            .starts_with("malformed frame"),
        "{res}"
    );

    // Missing method.
    client.send(json!({"id": 9}));
    let res = client.recv();
    assert_eq!(res["ok"], false);
    assert_eq!(res["id"], 9);

    // Still alive and authenticated.
    let ok = client.call(10, "session.info", json!({}));
    assert_eq!(ok["ok"], true, "{ok}");
}

#[test]
fn unknown_method_errors_and_keeps_the_connection() {
    let (handle, token) = start(session_with_doc(16, 16));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    let res = client.call(1, "does.not.exist", json!({}));
    assert_eq!(res["id"], 1);
    assert_eq!(res["ok"], false);
    assert_eq!(res["error"], "unknown method: does.not.exist");

    let ok = client.call(2, "commands.list", json!({}));
    assert_eq!(ok["ok"], true, "{ok}");
}

#[test]
fn command_run_requires_a_command_id() {
    let (handle, token) = start(session_with_doc(16, 16));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    let res = client.call(1, "command.run", json!({"params": {}}));
    assert_eq!(res["ok"], false);
    assert!(
        res["error"].as_str().unwrap().contains("params.id"),
        "{res}"
    );

    let unknown = client.call(2, "command.run", json!({"id": "nope.nope"}));
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["error"], "unknown command: nope.nope");
}

#[test]
fn handle_shutdown_stops_accepting() {
    let (handle, _token) = start(session_with_doc(16, 16));
    let addr = handle.local_addr();
    handle.shutdown();
    handle.shutdown(); // idempotent
    match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
        Err(_) => {}
        Ok(stream) => {
            // A late connect may complete against the listen backlog, but the
            // server can never answer: reading yields EOF or a timeout.
            stream
                .set_read_timeout(Some(Duration::from_millis(250)))
                .unwrap();
            let mut buf = [0u8; 1];
            let mut reader = &stream;
            let read = std::io::Read::read(&mut reader, &mut buf);
            assert!(read.is_err() || read.unwrap() == 0, "server still serving");
        }
    }
}

#[test]
fn spawn_control_rejects_a_bad_token() {
    let session = session_with_doc(8, 8);
    match spawn_control(session.clone(), 0, "not-a-token".to_string()) {
        Err(AutomationError::Auth(msg)) => assert!(msg.contains("64 hex"), "{msg}"),
        other => panic!(
            "expected an auth error, got {:?}",
            other.err().map(|e| e.to_string())
        ),
    }
}

#[test]
fn oversized_frames_are_rejected_without_buffering_them() {
    let (handle, token) = start(session_with_doc(16, 16));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    let huge = format!(
        "{{\"id\":1,\"method\":\"command.run\",\"params\":{{\"id\":\"doc.new\",\"pad\":\"{}\"}}}}",
        "x".repeat(snagcraft_automation::MAX_FRAME_BYTES + 16)
    );
    client.send_raw(&huge);
    let res = client.recv();
    assert_eq!(res["ok"], false);
    assert_eq!(res["error"], "frame too large");
    assert!(client.at_eof(), "oversized frame closes the connection");
}

#[test]
fn screenshot_without_a_document_errors_cleanly() {
    let (handle, token) = start(Arc::new(Mutex::new(Session::new())));
    let mut client = Client::connect(handle.local_addr());
    client.auth(&token);

    let res = client.call(1, "ui.screenshot", json!({}));
    assert_eq!(res["ok"], false);
    assert_eq!(res["error"], "no document open");

    // Session is still usable after the failed render.
    let new = client.call(
        2,
        "command.run",
        json!({"id": "doc.new", "params": {"w": 8, "h": 8}}),
    );
    assert_eq!(new["ok"], true, "{new}");
    let shot = client.call(3, "ui.screenshot", json!({}));
    assert_eq!(shot["ok"], true, "{shot}");
}

#[test]
fn token_file_round_trip_is_0600_and_repairs_mode() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_path("token-dir");
    let path = dir.join("nested").join("control.token");
    let token = generate_token();

    write_token_file(&path, &token).expect("write token");
    assert!(path.exists());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "token file must be 0600");
    assert_eq!(read_token_file(&path).unwrap(), token);

    // A file that drifted to 0644 is repaired on the next write.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let fresh = generate_token();
    write_token_file(&path, &fresh).expect("rewrite token");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "rewrite must restore 0600");
    assert_eq!(read_token_file(&path).unwrap(), fresh);
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents.trim(), fresh);

    // Garbage is refused on both ends.
    std::fs::write(&path, "deadbeef\n").unwrap();
    assert!(matches!(
        read_token_file(&path),
        Err(AutomationError::Auth(_))
    ));
    assert!(matches!(
        write_token_file(&path, "short"),
        Err(AutomationError::Auth(_))
    ));
    assert!(matches!(
        read_token_file(Path::new("/nonexistent/snagcraft/token")),
        Err(AutomationError::Io(_))
    ));

    let _ = std::fs::remove_dir_all(&dir);
    assert!(!dir.exists(), "temp dir cleaned up");
}
