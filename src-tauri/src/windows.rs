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

    // The floating widget doubles as the app's "always available" mode: closing
    // the editor must leave the widget on top rather than kill the process.
    if let Some(widget) = app.get_webview_window("widget") {
        if widget.is_visible().unwrap_or(false) {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.hide();
                let _ = app.emit("window-hidden-to-tray", ());
            }
            return;
        }
    }

    // Hide only when a tray icon really exists, or the app would become
    // unreachable. No tray (e.g. no appindicator on this desktop) → just quit.
    if flags
        .as_ref()
        .map(|f| f.minimize_to_tray())
        .unwrap_or(false)
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

/// Size of the widget in its two visual states, in logical pixels. These must
/// stay in step with `src/widget.css`:
///
/// | state    | window  | drawn surface           |
/// |----------|---------|-------------------------|
/// | folded   | 58x74   | 46x64 tab ("flärp")     |
/// | expanded | 304x60  | 292x50 capture pill     |
///
/// The window genuinely changes size (see [`widget_apply_size`]) rather than
/// keeping one large transparent surface: on Wayland a surface's input region
/// is the whole surface, so a permanently large window would swallow desktop
/// clicks around a collapsed tab.
const WIDGET_WIDTH: f64 = 304.0;
const WIDGET_HEIGHT: f64 = 60.0;
const WIDGET_TAB_WIDTH: f64 = 58.0;
const WIDGET_TAB_HEIGHT: f64 = 74.0;

fn widget_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(WIDGET_LABEL)
}

/// Create the widget window. Returns the live window; the caller decides
/// whether to show it.
fn build_widget<R: Runtime>(app: &AppHandle<R>) -> Result<WebviewWindow<R>, String> {
    let builder =
        WebviewWindowBuilder::new(app, WIDGET_LABEL, WebviewUrl::App("widget.html".into()))
            .title("SnagCraft")
            // Starts folded: an idle widget is a small tab, never a parked pill.
            .inner_size(WIDGET_TAB_WIDTH, WIDGET_TAB_HEIGHT)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            // A widget, not a window: the user must never be able to stretch,
            // maximise or minimise it (KDE offers all three even on frameless
            // windows via Alt-drag / the window menu). Programmatic tab↔pill
            // resizes still work: `widget_apply_size` briefly unlocks the
            // geometry hints around each `set_size` — GTK clamps resizes to the
            // hints, and `resizable(false)` pins them to the CURRENT size.
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
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
        eprintln!(
            "snagcraft: widget always-on-top not supported here ({err}); normal stacking applies"
        );
    }
    // Pin the compositor-side size limits to the folded tab as well.
    let tab = tauri::LogicalSize::new(WIDGET_TAB_WIDTH, WIDGET_TAB_HEIGHT);
    let _ = window.set_min_size(Some(tab));
    let _ = window.set_max_size(Some(tab));
    place_top_right(&window, WIDGET_TAB_WIDTH);
    Ok(window)
}

/// Ask the compositor to park the widget in the top-right corner, keeping its
/// right edge fixed whichever state it is in.
///
/// Under Wayland this is a no-op (positioning is the compositor's job) and the
/// error is ignored on purpose: the window then keeps its top-left corner and
/// grows rightwards when it expands.
fn place_top_right<R: Runtime>(window: &WebviewWindow<R>, logical_width: f64) {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let origin = monitor.position();
    let x = origin.x + size.width as i32 - (logical_width * scale) as i32 - (16.0 * scale) as i32;
    let y = origin.y + (32.0 * scale) as i32;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// Resize the widget for one visual state (see [`WIDGET_TAB_WIDTH`]).
///
/// `set_size` is the only supported size path on Wayland; under X11 it also
/// re-anchors the window so the pill grows leftwards from the tab's right edge
/// instead of walking off the monitor.
fn widget_apply_size<R: Runtime>(window: &WebviewWindow<R>, expanded: bool) -> Result<(), String> {
    let (width, height) = if expanded {
        (WIDGET_WIDTH, WIDGET_HEIGHT)
    } else {
        (WIDGET_TAB_WIDTH, WIDGET_TAB_HEIGHT)
    };
    // Unlock → new limits → resize → lock. GTK clamps `set_size` to the geometry
    // hints, and a locked (resizable=false) window pins them to the OLD size —
    // so the hints must move first, and the lock returns afterwards to keep the
    // widget un-stretchable by hand between state changes.
    let size = tauri::LogicalSize::new(width, height);
    let _ = window.set_resizable(true);
    let _ = window.set_min_size(Some(size));
    let _ = window.set_max_size(Some(size));
    let result = window.set_size(size);
    let _ = window.set_resizable(false);
    result.map_err(|err| format!("could not resize the widget: {err}"))?;
    place_top_right(window, width);
    Ok(())
}

fn show_widget<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    // Always come back folded: `widget_capture` hides the widget mid-pill, and
    // it has to reappear as a tab rather than a pill parked over the desktop.
    let _ = widget_apply_size(window, false);
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
    // The widget doubles as the app's always-available mode: closing it while the
    // editor is hidden must bring the editor back, never leave the app unreachable.
    if let Some(main) = app.get_webview_window("main") {
        if !main.is_visible().unwrap_or(true) {
            let _ = main.show();
            let _ = main.set_focus();
        }
    }
    Ok(())
}

/// Toggle the widget; returns whether it is visible afterwards.
/// Generic over the runtime so the tray (which is built for any `R: Runtime`)
/// can call it directly — the command wrapper below pins `Wry`.
pub fn toggle_widget_for<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    match widget_window(app) {
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
            let window = build_widget(app)?;
            show_widget(&window)?;
            Ok(true)
        }
    }
}

/// Toggle the widget; returns whether it is visible afterwards.
#[tauri::command]
pub fn toggle_widget(app: AppHandle) -> Result<bool, String> {
    toggle_widget_for(&app)
}

/// The widget's single channel to the backend.
///
/// `mode` is one of:
///   * `"full"` / `"region"` — start a capture in the main window;
///   * `"record"` / `"stop-record"` — start / stop a screen recording;
///   * `"move"`              — native interactive window move (the drag);
///   * `"expand"` / `"collapse"` — resize between the pill and the edge tab.
///
/// Why window plumbing rides on a "capture" command: the `default` capability
/// is scoped to `windows: ["main"]`, so every `plugin:window|*` command is
/// rejected from the widget webview with "not allowed by ACL". That is the real
/// reason the widget never moved — `getCurrentWindow().startDragging()` *and*
/// `data-tauri-drag-region` both invoke `plugin:window|start_dragging`, which
/// this window may not call. Application commands are not ACL-gated, so the
/// widget asks here and Rust performs the very same native calls
/// (`start_dragging`, `set_size`) with no permission in the way.
#[tauri::command]
pub fn widget_capture(app: AppHandle, mode: String) -> Result<(), String> {
    match mode.as_str() {
        "move" => widget_drag(&app),
        "expand" => widget_resize(&app, true),
        "collapse" => widget_resize(&app, false),
        "full" | "region" => capture_from_widget(&app, &mode),
        "record" => record_from_widget(&app, true),
        "stop-record" => record_from_widget(&app, false),
        other => Err(format!("unknown widget mode: {other}")),
    }
}

/// "Rec" pressed inside the widget: hand the recording to the main window and
/// hide the widget so it never ends up in the video. "Stop" keeps the widget
/// visible — the user asked for it mid-take, and the main window's Stop button,
/// tray and hotkeys keep working as before.
fn record_from_widget(app: &AppHandle, start: bool) -> Result<(), String> {
    if start {
        if let Some(window) = widget_window(app) {
            let _ = window.hide();
        }
        app.emit_to("main", "widget-record", ())
            .map_err(|err| format!("could not reach the main window: {err}"))?;
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.unminimize();
            let _ = main.show();
            let _ = main.set_focus();
        }
    } else {
        app.emit_to("main", "widget-stop-record", ())
            .map_err(|err| format!("could not reach the main window: {err}"))?;
    }
    Ok(())
}

/// Start a compositor-driven move. This maps to `xdg_toplevel.move` on Wayland
/// with the serial of the press that is still current; a manual `set_position`
/// would simply be ignored there.
fn widget_drag(app: &AppHandle) -> Result<(), String> {
    let Some(window) = widget_window(app) else {
        return Ok(());
    };
    window
        .start_dragging()
        .map_err(|err| format!("could not start the widget drag: {err}"))
}

/// Resize the widget to one of its two visual states. Failures are logged and
/// swallowed: a compositor that refuses the resize must not break the widget's
/// capture buttons.
fn widget_resize(app: &AppHandle, expanded: bool) -> Result<(), String> {
    let Some(window) = widget_window(app) else {
        return Ok(());
    };
    if let Err(err) = widget_apply_size(&window, expanded) {
        eprintln!(
            "snagcraft: widget resize to {} refused ({err}); keeping the current size",
            if expanded { "the pill" } else { "the tab" }
        );
    }
    Ok(())
}

/// "Capture" pressed inside the widget: hand the mode to the main window, hide
/// the widget (so it never ends up in the screenshot) and bring the main window
/// forward.
fn capture_from_widget(app: &AppHandle, mode: &str) -> Result<(), String> {
    if let Some(window) = widget_window(app) {
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

/// Saved main-window size while a HUD (scrolling card / countdown) owns it.
#[derive(Default)]
pub struct HudWindowSize(pub Mutex<Option<tauri::LogicalSize<f64>>>);

/// Shrink/restore the main window for a HUD (the scrolling card or the timer).
///
/// Both used to be DOM overlays inside the full-size window: the scrolling card
/// was unclickable (the window was click-through) and the countdown covered the
/// whole editor while the user was supposed to prepare the screen. Shrinking the
/// main window to the HUD (decorations off, on top) leaves the desktop usable and
/// keeps the HUD's buttons clickable.
#[tauri::command]
pub fn set_hud_mode(
    app: AppHandle,
    saved: State<'_, HudWindowSize>,
    enabled: bool,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Err("the main window is gone".to_string());
    };
    if enabled {
        let scale = win.scale_factor().unwrap_or(1.0);
        if let Ok(size) = win.inner_size() {
            if let Ok(mut slot) = saved.0.lock() {
                *slot = Some(size.to_logical::<f64>(scale));
            }
        }
        let _ = win.set_min_size(None::<tauri::LogicalSize<f64>>);
        win.set_size(tauri::LogicalSize::new(width.max(160.0), height.max(120.0)))
            .map_err(|e| format!("cannot shrink the window for the HUD: {e}"))?;
        let _ = win.set_decorations(false);
        let _ = win.set_always_on_top(true);
        let _ = win.set_focus();
    } else {
        let restore = saved.0.lock().ok().and_then(|g| *g);
        let _ = win.set_decorations(true);
        let _ = win.set_min_size(Some(tauri::LogicalSize::new(800.0, 560.0)));
        if let Some(size) = restore {
            let _ = win.set_size(size);
        }
        let _ = win.set_always_on_top(false);
        let _ = win.set_focus();
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

    #[test]
    fn widget_folds_tighter_than_it_expands() {
        // Collapsed must block strictly less desktop than expanded. The flärp
        // is a vertical bookmark: intentionally TALLER than the pill, but far
        // smaller in area — and on Wayland the whole window surface is the
        // input region, so area is the invariant that matters.
        const { assert!(WIDGET_TAB_WIDTH < WIDGET_WIDTH) };
        const { assert!(WIDGET_TAB_WIDTH * WIDGET_TAB_HEIGHT < WIDGET_WIDTH * WIDGET_HEIGHT) };

        // Content boxes must match src/widget.css exactly:
        //   window − (6px + 6px horizontal, 5px + 5px vertical padding)
        assert_eq!(WIDGET_TAB_WIDTH - 12.0, 46.0, "tab width");
        assert_eq!(WIDGET_TAB_HEIGHT - 10.0, 64.0, "tab height");
        assert_eq!(WIDGET_WIDTH - 12.0, 240.0, "pill width");
        assert_eq!(WIDGET_HEIGHT - 10.0, 50.0, "pill height");
    }
}
