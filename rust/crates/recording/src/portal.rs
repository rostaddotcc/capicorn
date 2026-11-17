//! The portal half: `ashpd` `Screencast` session → picker → PipeWire remote fd.
//!
//! Every step is logged with a timestamp to `~/.cache/snagcraft-portal.log`, so a
//! failed take pinpoints the stage (picker vs PipeWire connect vs no frames)
//! instead of asking the user to describe the symptom. Failures surface their
//! real cause; only a genuine picker cancel gets a neutral message (see
//! [`RecordingError::user_message`]).

use std::future::Future;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::time::Duration;

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};

use crate::RecordingError;

/// Portal calls that are pure D-Bus round-trips: a wedged portal hangs them.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// The picker: the user has to see it, decide and click "Share".
const PICKER_TIMEOUT: Duration = Duration::from_secs(120);

/// A started ScreenCast, ready to be consumed by [`crate::capture`].
pub(crate) struct Cast {
    /// The PipeWire remote to connect to.
    pub fd: OwnedFd,
    /// The stream's node id inside that remote.
    pub node: u32,
    /// The portal's size hint. Only an enumeration default — the negotiated
    /// format from `param_changed` is the truth.
    pub hint: (i32, i32),
    /// Must outlive the capture: dropping it closes the portal session and the
    /// stream dies with it.
    pub session: Session<Screencast>,
}

/// Append a timestamped line to the portal log. Best-effort by design: a log
/// write must never be the reason a take fails.
pub(crate) fn log(message: &str) {
    use std::io::Write;

    let path = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("snagcraft-portal.log");

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "[{stamp}] {message}");
    }
}

/// Await one portal call, turning a hang into a readable error.
async fn call<T>(
    timeout: Duration,
    what: &str,
    future: impl Future<Output = Result<T, ashpd::Error>>,
) -> Result<T, RecordingError> {
    match tokio::time::timeout(timeout, future).await {
        Err(_) => {
            let message = format!(
                "{what} timed out after {}s — is xdg-desktop-portal healthy and the screen unlocked?",
                timeout.as_secs()
            );
            log(&message);
            Err(RecordingError::Portal(message))
        }
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            let message = format!("{what} failed: {error}");
            log(&message);
            Err(RecordingError::Portal(message))
        }
    }
}

/// Create the session, ask the user what to share, wait for approval, and open
/// the PipeWire remote.
///
/// `MONITOR | WINDOW` is offered together: a MONITOR-only request mismatches a
/// window pick in the KDE picker and surfaces as "no screen was picked" even
/// though the user picked one.
pub(crate) async fn open() -> Result<Cast, RecordingError> {
    log("open: creating the ScreenCast proxy");
    let proxy = call(CALL_TIMEOUT, "ScreenCast portal", Screencast::new()).await?;

    log("open: CreateSession");
    let session = call(
        CALL_TIMEOUT,
        "CreateSession",
        proxy.create_session(Default::default()),
    )
    .await?;

    // The Request handle must stay alive until `start()` returns: dropping it
    // early races the portal's selection on KDE and the cast can fail to start.
    log("open: SelectSources (monitor|window, cursor embedded)");
    let _selection = call(
        CALL_TIMEOUT,
        "SelectSources",
        proxy.select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_sources(SourceType::MONITOR | SourceType::WINDOW)
                // The cursor is baked into the frames: right for mp4 and gif.
                .set_cursor_mode(CursorMode::EMBEDDED)
                .set_multiple(false)
                // PersistMode lives in `ashpd::desktop`, not in `screencast`.
                .set_persist_mode(PersistMode::DoNot),
        ),
    )
    .await?;

    // `start` opens the picker and only resolves once the user clicks Share (or
    // the portal answers with an error — a cancel arrives here).
    log("open: Start — opening the picker (waiting for the user)");
    let request = call(
        PICKER_TIMEOUT,
        "Start screen cast (picker)",
        proxy.start(&session, None, Default::default()),
    )
    .await?;

    let streams = request.response().map_err(|error| {
        let message = format!("screen sharing was refused: {error}");
        log(&message);
        RecordingError::Portal(message)
    })?;

    log(&format!(
        "picker answered: {} stream(s)",
        streams.streams().len()
    ));
    let stream = streams.streams().first().ok_or_else(|| {
        let message =
            "the portal returned no streams (this compositor may not support monitor capture)";
        log(message);
        RecordingError::Portal(message.to_string())
    })?;
    let node = stream.pipe_wire_node_id();
    let hint = stream.size().unwrap_or((1920, 1080));
    log(&format!("stream node={node} hint={}x{}", hint.0, hint.1));

    let fd = call(
        CALL_TIMEOUT,
        "OpenPipeWireRemote",
        proxy.open_pipe_wire_remote(&session, Default::default()),
    )
    .await?;
    log("open: PipeWire remote opened — handing over to the capture thread");

    Ok(Cast {
        fd,
        node,
        hint,
        session,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_never_panics_without_a_cache_dir() {
        // The logger must be fire-and-forget: a broken HOME cannot fail a take.
        log("test: portal logger is best-effort");
    }
}
