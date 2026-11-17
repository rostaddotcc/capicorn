//! Loopback TCP JSON control channel (PhotoCraft-derived model, W5a subset).
//!
//! Wire format is line-delimited JSON, one frame per line:
//!
//! ```text
//! -> {"id": 1, "method": "auth", "params": {"token": "<64 hex>"}}
//! -> {"id": 2, "method": "command.run", "params": {"id": "doc.new", "params": {"w": 64, "h": 48}}}
//! <- {"id": 2, "ok": true, "result": {"width": 64, "height": 48}}
//! <- {"id": 3, "ok": false, "error": "unknown method: nope"}
//! ```
//!
//! The first frame on every connection must be `auth`; a wrong token answers
//! `{"ok": false, "error": "authentication required"}` and closes the socket.
//! Auth sends **no** acknowledgement frame when it succeeds — a client that sends
//! `auth` then a method reads exactly one frame back, which is the frame for the
//! method it asked about. (A client may re-send `auth` on an authenticated
//! connection and gets `{"ok": true, "result": {"authenticated": true}}`.)

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use snagcraft_engine::Session;

use crate::{AutomationError, commands_json, dispatch, tokens_equal};

/// Hard cap on one request frame (1 MB). Longer lines are rejected *and* drained
/// so the peer's next frame is not misparsed as garbage.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Idle connections are dropped after this long without a frame.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

const AUTH_REQUIRED: &str = "authentication required";

/// A running control server. Dropping the handle shuts it down.
pub struct ControlHandle {
    local_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<(u64, TcpStream)>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl ControlHandle {
    /// The bound address — pass `port = 0` and read the real port from here.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stop accepting and drop live connections, then join the accept thread.
    ///
    /// Idempotent. Connections are closed at the socket level so their blocking
    /// reads return immediately instead of waiting out [`IDLE_TIMEOUT`].
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        {
            let conns = match self.conns.lock() {
                Ok(c) => c,
                Err(poisoned) => poisoned.into_inner(),
            };
            for (_, stream) in conns.iter() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        // Wake the blocking `accept` with a throwaway loopback connection.
        let _ = TcpStream::connect_timeout(&self.local_addr, Duration::from_millis(250));
        let handle = self.join.lock().ok().and_then(|mut slot| slot.take());
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl Drop for ControlHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Bind `127.0.0.1:port` and serve the control protocol on background threads.
///
/// `port = 0` picks an ephemeral port — read it back with
/// [`ControlHandle::local_addr`]. One accept thread plus one thread per connection;
/// the session mutex is held only for the duration of a single engine dispatch.
pub fn spawn_control(
    session: Arc<Mutex<Session>>,
    port: u16,
    token: String,
) -> Result<ControlHandle, AutomationError> {
    if !crate::is_valid_token(&token) {
        return Err(AutomationError::Auth(
            "control token must be 64 hex chars".to_string(),
        ));
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|e| AutomationError::Io(format!("bind 127.0.0.1:{port}: {e}")))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| AutomationError::Io(format!("listener local_addr: {e}")))?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let conns: Arc<Mutex<Vec<(u64, TcpStream)>>> = Arc::new(Mutex::new(Vec::new()));
    let next_id = Arc::new(AtomicU64::new(1));
    let token: Arc<str> = Arc::from(token);

    let flag = Arc::clone(&shutdown);
    let registry = Arc::clone(&conns);
    let join = thread::Builder::new()
        .name("snagcraft-control".to_string())
        .spawn(move || accept_loop(listener, session, token, flag, registry, next_id))
        .map_err(|e| AutomationError::Io(format!("spawn control thread: {e}")))?;

    tracing::info!(%local_addr, "control channel listening");
    Ok(ControlHandle {
        local_addr,
        shutdown,
        conns,
        join: Mutex::new(Some(join)),
    })
}

fn accept_loop(
    listener: TcpListener,
    session: Arc<Mutex<Session>>,
    token: Arc<str>,
    shutdown: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<(u64, TcpStream)>>>,
    next_id: Arc<AtomicU64>,
) {
    for incoming in listener.incoming() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "control accept failed");
                continue;
            }
        };
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(clone) = stream.try_clone() {
            match conns.lock() {
                Ok(mut c) => c.push((id, clone)),
                Err(poisoned) => poisoned.into_inner().push((id, clone)),
            }
        }
        let session = Arc::clone(&session);
        let token = Arc::clone(&token);
        let conns = Arc::clone(&conns);
        let spawned = thread::Builder::new()
            .name(format!("snagcraft-control-conn-{id}"))
            .spawn(move || {
                if let Err(e) = serve_connection(stream, &session, &token) {
                    tracing::debug!(connection = id, error = %e, "control connection ended");
                }
                // Deregister so the shutdown list stays bounded.
                match conns.lock() {
                    Ok(mut c) => c.retain(|(cid, _)| *cid != id),
                    Err(poisoned) => poisoned.into_inner().retain(|(cid, _)| *cid != id),
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not spawn control connection thread");
        }
    }
    tracing::info!("control channel stopped accepting");
}

/// How a bounded line read ended.
enum Frame {
    Line(Vec<u8>),
    TooLong,
    Eof,
}

/// One line, never buffering more than `max` bytes.
///
/// Reads in `fill_buf` chunks so an attacker who never sends `\n` cannot make us
/// allocate unboundedly; an oversized line is swallowed up to its newline so the
/// stream stays framed.
fn read_frame(reader: &mut BufReader<TcpStream>, max: usize) -> std::io::Result<Frame> {
    let mut buf: Vec<u8> = Vec::new();
    let mut oversize = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if buf.is_empty() && !oversize {
                return Ok(Frame::Eof);
            }
            break;
        }
        let (chunk, consumed, newline) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (&available[..=i], i + 1, true),
            None => (available, available.len(), false),
        };
        if !oversize {
            buf.extend_from_slice(chunk);
            if buf.len() > max {
                oversize = true;
                buf.clear();
            }
        }
        reader.consume(consumed);
        if newline {
            break;
        }
    }
    if oversize {
        return Ok(Frame::TooLong);
    }
    Ok(Frame::Line(buf))
}

fn serve_connection(
    stream: TcpStream,
    session: &Mutex<Session>,
    token: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    // ---- first frame: auth ------------------------------------------------
    let first = match read_frame(&mut reader, MAX_FRAME_BYTES)? {
        Frame::Line(line) => line,
        Frame::TooLong => {
            write_error(&mut writer, Value::Null, "frame too large")?;
            return Ok(());
        }
        Frame::Eof => return Ok(()),
    };
    let frame = serde_json::from_slice::<Value>(&first).unwrap_or(Value::Null);
    let id = frame.get("id").cloned().unwrap_or(Value::Null);
    let authorized = frame.get("method").and_then(Value::as_str) == Some("auth")
        && frame
            .get("params")
            .and_then(|p| p.get("token"))
            .and_then(Value::as_str)
            .is_some_and(|t| tokens_equal(t, token));
    if !authorized {
        tracing::debug!("control connection rejected: bad or missing token");
        write_error(&mut writer, id, AUTH_REQUIRED)?;
        let _ = writer.shutdown(Shutdown::Both);
        return Ok(());
    }

    // ---- authenticated request loop ---------------------------------------
    loop {
        let line = match read_frame(&mut reader, MAX_FRAME_BYTES) {
            Ok(Frame::Line(line)) => line,
            Ok(Frame::TooLong) => {
                write_error(&mut writer, Value::Null, "frame too large")?;
                return Ok(());
            }
            Ok(Frame::Eof) => return Ok(()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                tracing::debug!("control connection idle timeout");
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        match handle_frame(session, &line) {
            Ok(res) => write_frame(&mut writer, &res)?,
            Err(msg) => write_error(&mut writer, Value::Null, &msg)?,
        }
    }
}

/// Turn one request line into one response frame. Never panics, never blocks on I/O.
fn handle_frame(session: &Mutex<Session>, line: &[u8]) -> Result<Value, String> {
    let frame = match serde_json::from_slice::<Value>(line) {
        Ok(v) => v,
        Err(e) => {
            return Ok(error_frame(Value::Null, &format!("malformed frame: {e}")));
        }
    };
    let id = frame.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = frame.get("method").and_then(Value::as_str) else {
        return Ok(error_frame(id, "malformed frame: missing 'method'"));
    };
    let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));

    let result: Result<Value, String> = match method {
        "auth" => Ok(json!({"authenticated": true})),
        "commands.list" => Ok(commands_json()),
        "command.run" => {
            let Some(command) = params.get("id").and_then(Value::as_str) else {
                return Ok(error_frame(
                    id,
                    "command.run requires params.id (a command id)",
                ));
            };
            let command_params = params.get("params").cloned().unwrap_or_else(|| json!({}));
            dispatch(session, command, command_params)
        }
        "session.info" => dispatch(session, "session.info", json!({})),
        "doc.inspect" => dispatch(session, "doc.inspect", json!({})),
        "ui.screenshot" => screenshot(session),
        other => Err(format!("unknown method: {other}")),
    };

    Ok(match result {
        Ok(value) => json!({"id": id, "ok": true, "result": value}),
        Err(error) => error_frame(id, &error),
    })
}

fn error_frame(id: Value, error: &str) -> Value {
    json!({"id": id, "ok": false, "error": error})
}

/// Render the composite and return it as base64 PNG plus its pixel size.
fn screenshot(session: &Mutex<Session>) -> Result<Value, String> {
    // Lock only for `render`; encoding a multi-megabyte PNG must not serialize
    // other clients behind the session mutex.
    let surface = {
        let guard = session
            .lock()
            .map_err(|_| "session lock poisoned".to_string())?;
        guard.render().map_err(|e| e.to_string())?
    };
    let (width, height) = surface.dimensions();
    let png = snagcraft_io::encode(&surface, snagcraft_io::ImageFormat::Png, 100)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "png": BASE64.encode(&png),
        "width": width,
        "height": height,
        "bytes": png.len(),
    }))
}

fn write_frame(writer: &mut TcpStream, frame: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(frame).map_err(std::io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

fn write_error(writer: &mut TcpStream, id: Value, message: &str) -> std::io::Result<()> {
    write_frame(writer, &error_frame(id, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_exposed_with_the_documented_shape() {
        let value = commands_json();
        let list = value.as_array().expect("array");
        assert_eq!(list.len(), snagcraft_engine::registry().len());
        for entry in list {
            assert!(entry["id"].is_string());
            assert!(entry["label"].is_string());
            assert!(entry["params"].is_string());
        }
    }
}
