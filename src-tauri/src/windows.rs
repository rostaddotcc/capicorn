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

use tauri::{
    AppHandle, Emitter, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

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

// ---------------------------------------------------------------------------
// Floating quick-capture widget
// ---------------------------------------------------------------------------
//
// A frameless, small window ("widget") that the hotkey can summon for a
// one-click capture. It loads `widget.html` from the frontend bundle.
//
// Wayland caveats, stated plainly (tao docs: `set_always_on_top` and
// `set_outer_position` are "Linux(Wayland): Unsupported"):
//   * `always_on_top` is applied best-effort — on Wayland normal stacking
//     applies until the user adds a KWin rule ("Keep above other windows").
//   * the top-right placement is best-effort too; under Wayland the compositor
//     decides where the window appears.
//   * `skip_taskbar` is an X11-only GTK hint and is ignored under Wayland.
// Everything essential — decorations(false), resizable(false), show/hide,
// set_focus — does work under Wayland.

/// Label of the widget webview window.
pub const WIDGET_LABEL: &str = "widget";
const WIDGET_WIDTH: f64 = 260.0;
const WIDGET_HEIGHT: f64 = 64.0;

fn widget_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WIDGET_LABEL)
}

/// Create the widget window. Returns the live window; the caller decides
/// whether to show it.
fn build_widget(app: &AppHandle) -> Result<WebviewWindow, String> {
    let builder = WebviewWindowBuilder::new(
        app,
        WIDGET_LABEL,
        WebviewUrl::App("widget.html".into()),
    )
    .title("Capicorn")
    .inner_size(WIDGET_WIDTH, WIDGET_HEIGHT)
    .decorations(false)
    .resizable(false)
    .skip_taskbar(true)
    .visible(true)
    .focused(true);

    // Requesting always-on-top at build time can fail on Wayland; apply it the
    // same best-effort way everywhere so window creation itself never fails.
    let window = builder
        .build()
        .map_err(|err| format!("could not create the widget window: {err}"))?;

    // Best-effort on Wayland (see module comment above).
    if let Err(err) = window.set_always_on_top(true) {
        eprintln!("capicorn: widget always-on-top not supported here ({err}); normal stacking applies");
    }
    place_top_right(&window);
    Ok(window)
}

/// Ask the compositor to park the widget in the top-right corner. Under Wayland
/// this is a no-op (positioning is the compositor's job) and the error is
/// ignored on purpose.
fn place_top_right<R: Runtime>(window: &WebviewWindow<R>) {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let origin = monitor.position();
    let x = origin.x + size.width as i32 - (WIDGET_WIDTH * scale) as i32 - (16.0 * scale) as i32;
    let y = origin.y + (32.0 * scale) as i32;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

fn show_widget<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    window
        .show()
        .map_err(|err| format!("could not show the widget: {err}"))?;
    // Focus is the part that still works under Wayland; a failure is not fatal.
    let _ = window.set_focus();
    Ok(())
}

/// Show the widget, creating it on first use.
#[tauri::command]
pub fn open_widget(app: AppHandle) -> Result<(), String> {
    match widget_window(&app) {
        Some(window) => show_widget(&window),
        None => {
            let window = build_widget(&app)?;
            show_widget(&window)
        }
    }
}

/// Close the widget and release its webview.
#[tauri::command]
pub fn close_widget(app: AppHandle) -> Result<(), String> {
    if let Some(window) = widget_window(&app) {
        window
            .close()
            .map_err(|err| format!("could not close the widget: {err}"))?;
    }
    Ok(())
}

/// Toggle the widget; returns whether it is visible afterwards.
#[tauri::command]
pub fn toggle_widget(app: AppHandle) -> Result<bool, String> {
    match widget_window(&app) {
        Some(window) => {
            if window.is_visible().unwrap_or(false) {
                window
                    .hide()
                    .map_err(|err| format!("could not hide the widget: {err}"))?;
                Ok(false)
            } else {
                show_widget(&window)?;
                Ok(true)
            }
        }
        None => {
            let window = build_widget(&app)?;
            show_widget(&window)?;
            Ok(true)
        }
    }
}

/// "Capture" pressed inside the widget: hand the mode to the main window, hide
/// the widget (so it never ends up in the screenshot) and bring the main window
/// forward.
#[tauri::command]
pub fn widget_capture(app: AppHandle, mode: String) -> Result<(), String> {
    if let Some(window) = widget_window(&app) {
        let _ = window.hide();
    }
    app.emit_to(
        "main",
        "widget-capture",
        serde_json::json!({ "mode": mode }),
    )
    .map_err(|err| format!("could not reach the main window: {err}"))?;

    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
    Ok(())
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