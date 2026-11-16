#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L6 platform services — toolkit-free shell parity for SnagCraft.
//!
//! Ported from rostad's live Tauri shell (`src-tauri/src/{tray,hotkeys}.rs`) with
//! every `tauri::` dependency stripped, so the egui/eframe app can keep the same
//! behaviour without a webview framework:
//!
//! - [`spawn_tray`] — StatusNotifierItem tray on `tray-icon`'s **ksni** backend
//!   (libappindicator emits no mouse events on Wayland). Left click restores the
//!   window, middle click toggles the floating widget, the DBusMenu carries
//!   Show / Capture / Floating widget / Quit.
//! - [`spawn_hotkeys`] — global shortcuts through
//!   `org.freedesktop.portal.GlobalShortcuts`. The compositor owns the grab, so
//!   this works on Wayland (`global-hotkey` is X11-only).
//! - [`set_autostart`] / [`autostart_enabled`] — XDG autostart `.desktop` file.
//! - [`acquire_or_forward`] — single instance: a second launch forwards its argv
//!   to the primary over a unix socket and exits.
//!
//! Degradation rule for the whole crate: a missing tray host, portal or session
//! bus must be a `Result`/event, never a panic — the UI hides tray-only affordances
//! when [`TrayHandle::is_available`] is false.

mod autostart;
mod hotkeys;
mod png;
mod single_instance;
mod tray;
mod trigger;

pub use autostart::{autostart_enabled, set_autostart};
pub use hotkeys::{HotkeyDef, HotkeyEvent, HotkeyHandle, spawn_hotkeys};
pub use single_instance::{Instance, acquire_or_forward};
pub use tray::{TrayEvent, TrayHandle, spawn_tray, tray_available};
pub use trigger::xdg_trigger;

/// Everything that can go wrong at the platform boundary. Every variant is
/// recoverable by the caller: the app degrades instead of dying.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The tray host is missing (headless session, no StatusNotifierWatcher).
    #[error("tray: {0}")]
    Tray(String),
    /// The GlobalShortcuts portal is missing or refused the binding.
    #[error("hotkeys: {0}")]
    Hotkeys(String),
    /// Filesystem / socket failure (autostart file, instance socket).
    #[error("io: {0}")]
    Io(String),
    /// The feature does not exist on this target.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl From<std::io::Error> for PlatformError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}
