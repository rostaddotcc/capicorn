//! Single instance: the first process listens, later launches hand their argv
//! over and exit.
//!
//! Socket path: `$SNAGCRAFT_SOCK` (test/override hook), else
//! `$XDG_RUNTIME_DIR/snagcraft.sock`, else `/tmp/snagcraft-$UID.sock`.
//!
//! Hostile input rule: every line is capped at [`MAX_LINE`] bytes and malformed
//! JSON is logged and dropped — a rogue client on the socket must never be able
//! to take the app down.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::PlatformError;

/// Cap on one forwarded argv line (64 KiB).
pub const MAX_LINE: usize = 64 * 1024;

/// Override hook for the socket path (tests, and portable installs).
pub const SOCK_ENV: &str = "SNAGCRAFT_SOCK";

/// Who this process is.
pub enum Instance {
    /// We own the socket; `rx` yields the argv of every later launch.
    Primary { rx: Receiver<Vec<String>> },
    /// Another instance owns the socket; our argv was already forwarded.
    Secondary,
}

fn env_non_empty(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// Pure path resolution, so it can be tested without touching the environment.
fn resolve_socket_path(sock_env: Option<&str>, runtime_dir: Option<&str>, uid: &str) -> PathBuf {
    if let Some(path) = sock_env {
        return PathBuf::from(path);
    }
    match runtime_dir {
        Some(dir) => PathBuf::from(dir).join("snagcraft.sock"),
        None => std::env::temp_dir().join(format!("snagcraft-{uid}.sock")),
    }
}

/// Effective user id, without `libc`: `/proc/self/status` on Linux, then `$UID`,
/// then a hash of `$HOME` so two users never share a fallback socket.
fn uid_tag() -> String {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:")
                && let Some(uid) = rest.split_whitespace().next()
            {
                return uid.to_string();
            }
        }
    }
    if let Some(uid) = env_non_empty("UID") {
        return uid;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::env::var("HOME").unwrap_or_default().hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn socket_path() -> PathBuf {
    let sock = env_non_empty(SOCK_ENV);
    let runtime = env_non_empty("XDG_RUNTIME_DIR");
    resolve_socket_path(sock.as_deref(), runtime.as_deref(), &uid_tag())
}

/// Take the instance lock, or forward `args` to whoever holds it.
///
/// - Connected to an existing socket → argv written as one JSON line,
///   [`Instance::Secondary`] (the caller should exit).
/// - No listener → bind, spawn the accept loop, [`Instance::Primary`].
pub fn acquire_or_forward(args: &[String]) -> Result<Instance, PlatformError> {
    let path = socket_path();
    forward_or_listen(&path, args)
}

#[cfg(unix)]
fn forward_or_listen(path: &Path, args: &[String]) -> Result<Instance, PlatformError> {
    use std::os::unix::net::{UnixListener, UnixStream};

    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => Some(stream),
        Err(err) => {
            // Anything but "nobody is listening" means we must not steal the
            // socket (permissions, wrong filesystem, …).
            if !matches!(
                err.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
            ) {
                return Err(PlatformError::Io(format!(
                    "cannot connect to {}: {err}",
                    path.display()
                )));
            }
            None
        }
    };

    if let Some(stream) = stream.as_mut() {
        let line = serde_json::to_string(args)
            .map_err(|err| PlatformError::Io(format!("cannot encode argv: {err}")))?;
        stream
            .write_all(line.as_bytes())
            .and_then(|()| stream.write_all(b"\n"))
            .and_then(|()| stream.flush())
            .map_err(|err| PlatformError::Io(format!("cannot forward argv: {err}")))?;
        tracing::info!(path = %path.display(), "forwarded argv to the running instance");
        return Ok(Instance::Secondary);
    }

    // No live owner: drop whatever stale socket file is left behind.
    match std::fs::remove_file(path) {
        Ok(()) => tracing::debug!(path = %path.display(), "removed stale instance socket"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(PlatformError::Io(format!(
                "cannot remove stale socket {}: {err}",
                path.display()
            )));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(path).map_err(|err| {
        PlatformError::Io(format!(
            "cannot bind instance socket {}: {err}",
            path.display()
        ))
    })?;

    let (tx, rx) = channel::<Vec<String>>();
    let thread_path = path.to_path_buf();
    std::thread::Builder::new()
        .name("snagcraft-ipc".to_string())
        .spawn(move || accept_loop(listener, tx, &thread_path))
        .map_err(|err| PlatformError::Io(format!("cannot spawn the listener thread: {err}")))?;

    tracing::info!(path = %path.display(), "listening for later launches");
    Ok(Instance::Primary { rx })
}

#[cfg(unix)]
fn accept_loop(listener: std::os::unix::net::UnixListener, tx: Sender<Vec<String>>, path: &Path) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // `false` means the events receiver is gone (the UI shut down);
                // stop accepting instead of spinning on a dead channel.
                if !handle_connection(stream, &tx) {
                    break;
                }
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), "instance socket accept failed: {err}");
            }
        }
    }
}

/// Read one bounded line and forward the parsed argv, if it is well formed.
///
/// Returns whether the events channel is still open. A malformed payload is
/// logged and dropped — hostile input must not stop the listener.
#[cfg(unix)]
fn handle_connection(stream: std::os::unix::net::UnixStream, tx: &Sender<Vec<String>>) -> bool {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut limited = (&mut reader).take((MAX_LINE + 1) as u64);
    match limited.read_line(&mut line) {
        Ok(0) => true,
        Ok(read) => {
            if read > MAX_LINE {
                tracing::warn!("instance socket line exceeds {MAX_LINE} bytes; dropped");
                return true;
            }
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed.is_empty() {
                return true;
            }
            match serde_json::from_str::<Vec<String>>(trimmed) {
                Ok(args) => tx.send(args).is_ok(),
                Err(err) => {
                    tracing::warn!("dropped malformed instance payload: {err}");
                    true
                }
            }
        }
        Err(err) => {
            tracing::warn!("instance socket read failed: {err}");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    fn temp_socket(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "snagcraft-platform-ipc-{}-{tag}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn path_resolution_prefers_env_then_runtime_dir_then_tmp() {
        assert_eq!(
            resolve_socket_path(
                Some("/run/user/1000/custom.sock"),
                Some("/run/user/1000"),
                "1000"
            ),
            PathBuf::from("/run/user/1000/custom.sock")
        );
        assert_eq!(
            resolve_socket_path(None, Some("/run/user/1000"), "1000"),
            PathBuf::from("/run/user/1000/snagcraft.sock")
        );
        assert_eq!(
            resolve_socket_path(None, None, "1000"),
            std::env::temp_dir().join("snagcraft-1000.sock")
        );
        assert!(!uid_tag().is_empty());
    }

    #[test]
    fn primary_then_secondary_forwards_argv() {
        let path = temp_socket("forward");
        let first = forward_or_listen(&path, &args(&["--first"])).expect("primary");
        let rx = match first {
            Instance::Primary { rx } => rx,
            Instance::Secondary => panic!("first launch must own the socket"),
        };

        let second = forward_or_listen(&path, &args(&["--capture", "--full"])).expect("secondary");
        assert!(matches!(second, Instance::Secondary));

        let forwarded = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("forwarded argv");
        assert_eq!(forwarded, args(&["--capture", "--full"]));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_payload_does_not_stop_the_listener() {
        let path = temp_socket("hostile");
        let first = forward_or_listen(&path, &[]).expect("primary");
        let rx = match first {
            Instance::Primary { rx } => rx,
            Instance::Secondary => panic!("first launch must own the socket"),
        };

        let mut hostile = UnixStream::connect(&path).expect("connect");
        hostile
            .write_all(b"}] this is not json {\n")
            .expect("write");
        // A JSON value that is not an argv array is rejected too.
        let mut wrong_shape = UnixStream::connect(&path).expect("connect");
        wrong_shape.write_all(b"{\"nope\":1}\n").expect("write");
        // And a giant line is capped, not buffered forever.
        let mut huge = UnixStream::connect(&path).expect("connect");
        huge.write_all(&vec![b'a'; MAX_LINE + 16]).expect("write");
        huge.write_all(b"\n").expect("write");

        // The listener still accepts a well-formed line afterwards.
        assert!(matches!(
            forward_or_listen(&path, &args(&["--show"])).expect("secondary"),
            Instance::Secondary
        ));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5))
                .expect("forwarded argv"),
            args(&["--show"])
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_socket_is_replaced() {
        let path = temp_socket("stale");
        // A plain file where the socket should be is not connectable → replaced.
        std::fs::write(&path, b"stale").expect("write stale file");
        let first = forward_or_listen(&path, &[]).expect("primary takes the path over");
        assert!(matches!(first, Instance::Primary { .. }));
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(not(unix))]
fn forward_or_listen(_path: &Path, _args: &[String]) -> Result<Instance, PlatformError> {
    tracing::info!("second-launch forwarding is unsupported on this target");
    let (_tx, rx) = channel::<Vec<String>>();
    Ok(Instance::Primary { rx })
}
