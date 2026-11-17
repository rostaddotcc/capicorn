#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L6 automation — the two external doors into the engine (W5a):
//!
//! * [`spawn_control`] — loopback-only TCP JSON control channel, token-file auth,
//!   registry-driven methods (PhotoCraft model: 127.0.0.1 only, token 0600, first
//!   frame authenticates).
//! * [`serve_mcp_stdio`] — MCP server over stdio, tools generated from the engine
//!   command registry.
//!
//! Both doors dispatch the *same* engine commands as the UI and CLI, so a command
//! added to `snagcraft_engine::registry()` needs no new automation code.
//!
//! ## Auth model
//! The listener binds `127.0.0.1` only, and the token lives in a `0600` file the
//! same user must be able to read. Token comparison XOR-accumulates over the full
//! length instead of short-circuiting on the first differing byte, which removes
//! the obvious timing oracle without pulling in a crypto dependency; given
//! loopback + `0600`, that is the documented bar (it is not constant-time against
//! an attacker who can already read the token file — such an attacker *is* the user).
//!
//! ## Hostile input
//! Frames are capped at [`MAX_FRAME_BYTES`], idle connections time out, tokens are
//! validated as 64 hex chars on both write and read, and every path / params value
//! handed to the engine goes through the engine's own validation.

mod control;
mod mcp;

use std::io::Read;
use std::path::Path;

pub use control::{ControlHandle, MAX_FRAME_BYTES, spawn_control};
pub use mcp::serve_mcp_stdio;

/// Length of a control token in hex characters (256 bits).
pub const TOKEN_HEX_LEN: usize = 64;

/// Everything that can go wrong at the automation boundary.
#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("io: {0}")]
    Io(String),
    #[error("auth: {0}")]
    Auth(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("mcp: {0}")]
    Mcp(String),
    #[error("engine: {0}")]
    Engine(String),
}

/// 256 random bits, lowercase hex (64 chars).
///
/// Primary source is the OS CSPRNG via `getrandom`. If that ever fails we retry,
/// then fall back to reading `/dev/urandom` directly, and only then to a
/// time/counter-derived value — logged as an error, because that last resort is
/// **not** cryptographically random. The signature is infallible per contract, and
/// a silently weak token would be worse than a loudly degraded one.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes);
    hex(&bytes)
}

fn fill_random(buf: &mut [u8]) {
    for _ in 0..8 {
        if getrandom::fill(buf).is_ok() {
            return;
        }
        std::thread::yield_now();
    }
    if let Ok(mut f) = std::fs::File::open("/dev/urandom")
        && f.read_exact(buf).is_ok()
    {
        return;
    }
    tracing::error!("no CSPRNG available; control token is not cryptographically random");
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
        .hash(&mut h);
    std::process::id().hash(&mut h);
    let seed = h.finish();
    for (i, chunk) in buf.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut h);
        (i as u64).hash(&mut h);
        let word = h.finish().to_le_bytes();
        let n = chunk.len().min(word.len());
        chunk[..n].copy_from_slice(&word[..n]);
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// Is `token` shaped like a token this crate produced (64 hex chars)?
pub fn is_valid_token(token: &str) -> bool {
    token.len() == TOKEN_HEX_LEN && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// XOR-accumulated comparison — see the crate docs for what this does and does
/// not buy. Different lengths still short-circuit (length is not a secret).
pub(crate) fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Write `token` to `path` with unix mode `0600`, creating parent directories.
///
/// Reuse-safe: an existing file is truncated and has its mode forced back to
/// `0600`, so a token file that drifted to `0644` is repaired rather than trusted.
pub fn write_token_file(path: &Path, token: &str) -> Result<(), AutomationError> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if !is_valid_token(token) {
        return Err(AutomationError::Auth(format!(
            "refusing to write a token that is not {TOKEN_HEX_LEN} hex chars"
        )));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| AutomationError::Io(format!("create {}: {e}", parent.display())))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| AutomationError::Io(format!("open {}: {e}", path.display())))?;
    file.write_all(token.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|e| AutomationError::Io(format!("write {}: {e}", path.display())))?;
    // `.mode()` only applies when the file is created; repair a reused file too.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| AutomationError::Io(format!("chmod {}: {e}", path.display())))?;
    Ok(())
}

/// Read and validate a token file (whitespace trimmed, must be 64 hex chars).
pub fn read_token_file(path: &Path) -> Result<String, AutomationError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| AutomationError::Io(format!("read {}: {e}", path.display())))?;
    let token = raw.trim();
    if !is_valid_token(token) {
        return Err(AutomationError::Auth(format!(
            "{} does not contain a {TOKEN_HEX_LEN}-char hex token",
            path.display()
        )));
    }
    Ok(token.to_string())
}

/// The registry as the wire shape both doors expose: `[{id, label, params}]`.
pub(crate) fn commands_json() -> serde_json::Value {
    serde_json::Value::Array(
        snagcraft_engine::registry()
            .into_iter()
            .map(|c| serde_json::json!({"id": c.id, "label": c.label, "params": c.params_doc}))
            .collect(),
    )
}

/// Lock, dispatch, unlock — the one place both doors touch the session.
///
/// The guard is confined to this call, so no I/O or await can ever hold the
/// session mutex: a slow client cannot stall the UI or another client.
pub(crate) fn dispatch(
    session: &std::sync::Mutex<snagcraft_engine::Session>,
    id: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut guard = session
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    guard.dispatch(id, params).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_64_hex_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), TOKEN_HEX_LEN);
        assert!(is_valid_token(&a));
        assert_ne!(a, b);
    }

    #[test]
    fn comparison_is_exact_at_the_last_byte() {
        let t = generate_token();
        assert!(tokens_equal(&t, &t));
        assert!(!tokens_equal(&t, &t[..TOKEN_HEX_LEN - 1]));
        let mut other = t.clone();
        other.pop();
        other.push(if t.ends_with('0') { '1' } else { '0' });
        assert!(!tokens_equal(&t, &other));
    }
}
