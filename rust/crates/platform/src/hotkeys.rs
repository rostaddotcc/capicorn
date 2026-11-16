//! Global hotkeys through `org.freedesktop.portal.GlobalShortcuts`.
//!
//! Ported from the live `src-tauri/src/hotkeys.rs` (portal flow, status types,
//! `xdg_trigger`); the `tauri-command`/state plumbing became a channel.
//!
//! Why the portal: `global-hotkey` (what most Tauri/X11 stacks wrap) is X11-only
//! on Linux — under a Wayland session there is no root window to grab, so the
//! registration silently never fires. The portal is the sanctioned path: the
//! compositor owns the binding (KDE routes it through KGlobalAccel / System
//! Settings → Shortcuts) and the app receives `Activated` signals.
//!
//! Flow:
//!
//! ```text
//! spawn_hotkeys([{id, trigger}])
//!   └─ dedicated thread + current-thread tokio runtime
//!        ├─ GlobalShortcuts::new() + create_session()  -- fails? → HotkeyEvent::Failed
//!        ├─ bind_shortcuts()        -- MAY BLOCK until the user accepts the
//!        │                             compositor's dialog; the reply can contain
//!        │                             FEWER shortcuts → we report what was granted
//!        └─ receive_activated()     -- Activated → HotkeyEvent::Activated
//! ```
//!
//! Only one `BindShortcuts` call is allowed per portal *session*, so re-binding
//! means a new session — the caller drops the [`HotkeyHandle`]-owning task and
//! calls `spawn_hotkeys` again.

use std::pin::Pin;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use crate::PlatformError;
use crate::xdg_trigger;

/// A hotkey the shell wants bound. `trigger` is either a human/X11-ish combo
/// (`"CTRL+SHIFT+P"`, `"Print"`, `"SHIFT+Print"`) or an already valid XDG
/// shortcuts string (`"<Control><Shift>p"`), which is passed through untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyDef {
    pub id: String,
    pub trigger: String,
}

impl HotkeyDef {
    pub fn new(id: impl Into<String>, trigger: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            trigger: trigger.into(),
        }
    }
}

/// What happened to the requested hotkeys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The compositor reports the shortcut was pressed.
    Activated { id: String },
    /// The portal answered: `(id, trigger description)` for each binding the
    /// user actually granted — possibly fewer than requested.
    Bound { granted: Vec<(String, String)> },
    /// Nothing is bound (no portal, user refused, portal error). The app keeps
    /// the tray/`--capture` fallbacks.
    Failed(String),
}

/// Keeps the portal session (and its activation listener) alive.
pub struct HotkeyHandle {
    _thread: JoinHandle<()>,
}

impl HotkeyHandle {
    /// True while the portal thread is still listening for activations.
    pub fn is_running(&self) -> bool {
        !self._thread.is_finished()
    }
}

/// Human text for the compositor's approval dialog. Unknown ids fall back to
/// the id itself so a new shortcut still gets a sensible label.
fn description_for(id: &str) -> String {
    match id {
        "capture-full" | "capture_full" => "Capture the whole screen".to_string(),
        "capture-region" | "capture_region" => "Capture a region".to_string(),
        "delayed-capture" | "delayed_capture" => "Delayed capture".to_string(),
        "open-widget" | "open_widget" | "toggle-widget" | "toggle_widget" => {
            "Show the quick capture widget".to_string()
        }
        other => format!("SnagCraft: {other}"),
    }
}

/// Map a portal shortcut id back to the definition we asked for.
///
/// Portal ids are the ids we passed in, but the lookup keeps the mapping in one
/// place and lets us drop activations for shortcuts we never requested.
fn resolve_def_id(defs: &[HotkeyDef], portal_id: &str) -> Option<String> {
    defs.iter()
        .find(|def| def.id == portal_id)
        .map(|def| def.id.clone())
}

/// Bind `defs` and report progress/activations on `events`.
///
/// Returns as soon as the portal thread is running; a missing portal, a refused
/// consent dialog or a slow compositor are reported as [`HotkeyEvent::Failed`]
/// (or `Bound` with an empty grant list) rather than as a caller-side error.
/// Only an unusable request (nothing to bind, or no thread available) is `Err`.
pub fn spawn_hotkeys(
    defs: Vec<HotkeyDef>,
    events: Sender<HotkeyEvent>,
) -> Result<HotkeyHandle, PlatformError> {
    let cleaned: Vec<HotkeyDef> = defs
        .into_iter()
        .filter(|def| !def.id.trim().is_empty() && !def.trigger.trim().is_empty())
        .collect();
    if cleaned.is_empty() {
        return Err(PlatformError::Hotkeys("no shortcuts requested".to_string()));
    }

    #[cfg(target_os = "linux")]
    {
        let thread = thread::Builder::new()
            .name("snagcraft-hotkeys".to_string())
            .spawn(move || run_portal(cleaned, events))
            .map_err(|err| {
                PlatformError::Hotkeys(format!("cannot spawn the hotkey thread: {err}"))
            })?;
        Ok(HotkeyHandle { _thread: thread })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cleaned, events);
        Err(PlatformError::Unsupported(
            "global hotkeys need the XDG GlobalShortcuts portal (Linux)".to_string(),
        ))
    }
}

#[cfg(target_os = "linux")]
fn run_portal(defs: Vec<HotkeyDef>, events: Sender<HotkeyEvent>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            report_failure(&events, format!("cannot start the portal runtime: {err}"));
            return;
        }
    };
    if let Err(msg) = runtime.block_on(bind_and_listen(&defs, &events)) {
        report_failure(&events, msg);
    }
}

#[cfg(target_os = "linux")]
fn report_failure(events: &Sender<HotkeyEvent>, msg: String) {
    tracing::warn!("global hotkeys unavailable ({msg}); falling back to the tray");
    let _ = events.send(HotkeyEvent::Failed(msg));
}

#[cfg(target_os = "linux")]
async fn bind_and_listen(defs: &[HotkeyDef], events: &Sender<HotkeyEvent>) -> Result<(), String> {
    use ashpd::desktop::global_shortcuts::GlobalShortcuts;
    use ashpd::desktop::{CreateSessionOptions, Session};

    let proxy = GlobalShortcuts::new()
        .await
        .map_err(|err| format!("GlobalShortcuts portal unavailable: {err}"))?;

    let session: Session<GlobalShortcuts> = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|err| format!("could not create a shortcuts session: {err}"))?;

    let outcome = bind_and_stream(&proxy, &session, defs, events).await;
    // Ends the portal session so the compositor releases the combos — on the
    // success path and on every error/timeout path.
    let _ = session.close().await;
    outcome
}

/// Longest we wait for the compositor's answer to `BindShortcuts`. The dialog is
/// allowed to be slow, but a portal that never replies at all must not leave the
/// shell permanently unsure whether hotkeys are live.
#[cfg(target_os = "linux")]
const BIND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[cfg(target_os = "linux")]
async fn bind_and_stream(
    proxy: &ashpd::desktop::global_shortcuts::GlobalShortcuts,
    session: &ashpd::desktop::Session<ashpd::desktop::global_shortcuts::GlobalShortcuts>,
    defs: &[HotkeyDef],
    events: &Sender<HotkeyEvent>,
) -> Result<(), String> {
    use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, NewShortcut};
    // `futures_core::Stream` is not a direct dependency (Cargo.toml is owned
    // elsewhere); borrow the trait through zbus's macro-support re-export.
    use ashpd::zbus::export::futures_core::Stream;

    let triggers: Vec<String> = defs.iter().map(|def| xdg_trigger(&def.trigger)).collect();
    let requested: Vec<NewShortcut> = defs
        .iter()
        .zip(triggers.iter())
        .map(|(def, trigger)| {
            NewShortcut::new(def.id.clone(), description_for(&def.id))
                .preferred_trigger(Some(trigger.as_str()))
        })
        .collect();

    // One BindShortcuts per session; this call blocks for as long as the
    // compositor's approval dialog is on screen. We are on our own thread, so
    // the caller (and the UI) is never blocked.
    let bind = tokio::time::timeout(
        BIND_TIMEOUT,
        proxy.bind_shortcuts(session, &requested, None, BindShortcutsOptions::default()),
    )
    .await;
    let request = match bind {
        Ok(Ok(request)) => request,
        Ok(Err(err)) => return Err(format!("BindShortcuts failed: {err}")),
        Err(_) => {
            return Err(format!(
                "the compositor did not answer BindShortcuts within {}s",
                BIND_TIMEOUT.as_secs()
            ));
        }
    };
    let bound = request
        .response()
        .map_err(|err| format!("shortcuts were not granted: {err}"))?;

    // The reply may hold fewer shortcuts than we asked for (user declined,
    // combo already taken) — report only what is really bound.
    let granted: Vec<(String, String)> = bound
        .shortcuts()
        .iter()
        .map(|shortcut| {
            let id = shortcut.id().to_string();
            let described = shortcut.trigger_description().trim().to_string();
            let trigger = if described.is_empty() {
                defs.iter()
                    .find(|def| def.id == id)
                    .map(|def| def.trigger.clone())
                    .unwrap_or_default()
            } else {
                described
            };
            (id, trigger)
        })
        .collect();

    if events.send(HotkeyEvent::Bound { granted }).is_err() {
        // Nobody is listening any more; stop and let the caller close.
        return Ok(());
    }

    let mut stream = Box::pin(
        proxy
            .receive_activated()
            .await
            .map_err(|err| format!("cannot listen for activations: {err}"))?,
    );

    loop {
        let next = std::future::poll_fn(|cx| Pin::new(&mut stream).as_mut().poll_next(cx)).await;
        let Some(activated) = next else { break };
        let portal_id = activated.shortcut_id().to_string();
        match resolve_def_id(defs, &portal_id) {
            Some(id) => {
                tracing::debug!(%id, "hotkey activated");
                if events.send(HotkeyEvent::Activated { id }).is_err() {
                    break;
                }
            }
            None => tracing::warn!(%portal_id, "activation for an unknown shortcut"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    fn defs() -> Vec<HotkeyDef> {
        vec![
            HotkeyDef::new("capture-full", "Print"),
            HotkeyDef::new("capture-region", "Shift+Print"),
        ]
    }

    #[test]
    fn portal_ids_resolve_back_to_definitions() {
        let defs = defs();
        assert_eq!(
            resolve_def_id(&defs, "capture-region"),
            Some("capture-region".to_string())
        );
        // An id we never requested (or a hostile one) maps to nothing.
        assert_eq!(resolve_def_id(&defs, "some-other-app"), None);
        assert_eq!(resolve_def_id(&defs, ""), None);
    }

    #[test]
    fn descriptions_are_human_readable() {
        assert_eq!(description_for("capture-full"), "Capture the whole screen");
        assert_eq!(description_for("capture-region"), "Capture a region");
        assert!(description_for("custom-id").contains("custom-id"));
    }

    #[test]
    fn empty_request_is_an_error_not_a_thread() {
        let (tx, _rx) = channel();
        match spawn_hotkeys(Vec::new(), tx) {
            Err(PlatformError::Hotkeys(msg)) => assert!(!msg.is_empty()),
            Err(other) => panic!("unexpected error kind: {other}"),
            Ok(_) => panic!("an empty request must not start a portal thread"),
        }
        let (tx, _rx) = channel();
        let blank = vec![HotkeyDef::new("  ", "Print"), HotkeyDef::new("id", "  ")];
        assert!(matches!(
            spawn_hotkeys(blank, tx),
            Err(PlatformError::Hotkeys(_))
        ));
    }

    /// Live test: talks to the real portal (and may pop a consent dialog).
    ///
    /// On a headless host — or one where the portal is present but its consent
    /// dialog can never be shown — the binding may simply stay pending. That is
    /// fine: the contract is "no panic, caller never blocked", so a pending
    /// bind is accepted as long as the handle is still alive and nothing blew up.
    #[test]
    #[ignore = "requires a desktop session with the GlobalShortcuts portal"]
    fn spawn_hotkeys_never_panics_headless() {
        let (tx, rx): (Sender<HotkeyEvent>, Receiver<HotkeyEvent>) = channel();
        match spawn_hotkeys(defs(), tx) {
            Ok(handle) => match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(HotkeyEvent::Bound { granted }) => {
                    assert!(granted.len() <= 2);
                    for (id, trigger) in granted {
                        assert!(!id.is_empty());
                        assert!(!trigger.is_empty());
                    }
                }
                Ok(HotkeyEvent::Failed(msg)) => assert!(!msg.is_empty()),
                Ok(HotkeyEvent::Activated { id }) => panic!("unexpected activation {id}"),
                // Still waiting for the compositor's answer: allowed.
                Err(_) => assert!(handle.is_running()),
            },
            Err(PlatformError::Hotkeys(msg)) => assert!(!msg.is_empty()),
            Err(other) => panic!("unexpected error kind: {other}"),
        }
    }
}
