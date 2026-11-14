//! Close-guard, quit flag and minimize-to-tray preference.
//!
//! Flow when the user clicks the window's X (or the WM asks to close):
//!
//! ```text
//! CloseRequested
//!   └─ api.prevent_close()            (the window NEVER closes on its own)
//!        ├─ force_quit set?      → app.exit(0)            (tray Quit / force_close)
//!        ├─ editor dirty?        → emit "close-requested" (frontend shows save modal,
//!        │                                                  then calls force_close)
//!        ├─ minimize_to_tray?    → window.hide()          (lives on in the tray)
//!        └─ otherwise            → app.exit(0)
//! ```
//!
//! `dirty` and `minimize_to_tray` are pushed in by the frontend through the
//! `set_dirty` / `set_minimize_to_tray` commands; Rust never guesses.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, Runtime, State, WindowEvent};

/// Process-wide close-guard flags, registered as managed state in `lib.rs`.
pub struct AppFlags {
    /// Set when the user has already confirmed they want out, so re-entering the
    /// close handler must not ask again.
    force_quit: AtomicBool,
    /// Frontend-reported: the editor holds unsaved work.
    dirty: AtomicBool,
    /// User preference: closing hides to tray instead of quitting.
    minimize_to_tray: Mutex<bool>,
}

impl Default for AppFlags {
    fn default() -> Self {
        Self {
            force_quit: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            minimize_to_tray: Mutex::new(false),
        }
    }
}

impl AppFlags {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn force_quit(&self) -> bool {
        self.force_quit.load(Ordering::SeqCst)
    }

    pub fn set_force_quit(&self) {
        self.force_quit.store(true, Ordering::SeqCst);
    }

    pub fn dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }

    pub fn set_dirty(&self, dirty: bool) {
        self.dirty.store(dirty, Ordering::SeqCst);
    }

    pub fn minimize_to_tray(&self) -> bool {
        // A poisoned lock still holds a usable bool; recover rather than panic
        // inside a window event handler.
        *self
            .minimize_to_tray
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_minimize_to_tray(&self, enabled: bool) {
        let mut guard = self
            .minimize_to_tray
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = enabled;
    }
}

/// Quit for real: mark the flag so the close guard stops asking, then exit.
/// Used by the tray "Quit" item and by the `force_close` command after the
/// frontend finished its save/discard decision.
pub fn request_quit<R: Runtime>(app: &AppHandle<R>) {
    if let Some(flags) = app.try_state::<AppFlags>() {
        flags.set_force_quit();
    }
    app.exit(0);
}

/// Run the close-guard decision tree. See the module docs for the flow.
fn handle_close<R: Runtime>(app: &AppHandle<R>) {
    let flags = app.try_state::<AppFlags>();

    if flags.as_ref().map(|f| f.force_quit()).unwrap_or(false) {
        app.exit(0);
        return;
    }

    if flags.as_ref().map(|f| f.dirty()).unwrap_or(false) {
        // Ask the frontend; it shows its own non-blocking save modal and calls
        // `force_close` (or `set_dirty(false)`) when the user decides.
        let _ = app.emit("close-requested", ());
        return;
    }

    // Hide only when a tray icon really exists, or the app would become
    // unreachable. No tray (e.g. no appindicator on this desktop) → just quit.
    if flags.as_ref().map(|f| f.minimize_to_tray()).unwrap_or(false)
        && crate::tray::is_available()
    {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.hide();
            let _ = app.emit("window-hidden-to-tray", ());
        }
        return;
    }

    app.exit(0);
}

/// Install the close guard on the main window. Called from `lib.rs` setup.
pub fn register_close_guard<R: Runtime>(app: &tauri::App<R>) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let handle = app.handle().clone();
    win.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            handle_close(&handle);
        }
    });
}

/// Exit the process, bypassing the close guard (save modal already resolved).
#[tauri::command]
pub fn force_close(app: AppHandle) {
    request_quit(&app);
}

/// Frontend reports whether the editor has unsaved changes.
#[tauri::command]
pub fn set_dirty(flags: State<'_, AppFlags>, dirty: bool) {
    flags.set_dirty(dirty);
}

/// Read the current dirty flag (frontend restore after a reload, tests, menus).
#[tauri::command]
pub fn is_dirty(flags: State<'_, AppFlags>) -> bool {
    flags.dirty()
}

/// Persist "closing the window hides it to the tray" (settings toggle).
#[tauri::command]
pub fn set_minimize_to_tray(flags: State<'_, AppFlags>, enabled: bool) {
    flags.set_minimize_to_tray(enabled);
}

#[tauri::command]
pub fn get_minimize_to_tray(flags: State<'_, AppFlags>) -> bool {
    flags.minimize_to_tray()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_start_closed_clean_and_visible() {
        let flags = AppFlags::new();
        assert!(!flags.force_quit());
        assert!(!flags.dirty());
        assert!(!flags.minimize_to_tray());
    }

    #[test]
    fn flags_round_trip() {
        let flags = AppFlags::new();
        flags.set_dirty(true);
        flags.set_minimize_to_tray(true);
        flags.set_force_quit();
        assert!(flags.dirty());
        assert!(flags.minimize_to_tray());
        assert!(flags.force_quit());
    }
}