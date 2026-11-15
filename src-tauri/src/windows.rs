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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// Size of the widget in its two visual states, in logical pixels. These must
/// stay in step with `src/widget.css`:
///
/// | state    | window | drawn surface           |
/// |----------|--------|-------------------------|
/// | folded   | 56x54  | 44x44 tab ("flärp")     |
/// | expanded | 252x60 | 240x50 capture pill     |
///
/// The window genuinely changes size (see [`widget_apply_size`]) rather than
/// keeping one large transparent surface: on Wayland a surface's input region
/// is the whole surface, so a permanently large window would swallow desktop
/// clicks around a collapsed tab.
const WIDGET_WIDTH: f64 = 252.0;
const WIDGET_HEIGHT: f64 = 60.0;
const WIDGET_TAB_WIDTH: f64 = 56.0;
const WIDGET_TAB_HEIGHT: f64 = 54.0;

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
    // Starts folded: an idle widget is a small tab, never a parked pill.
    .inner_size(WIDGET_TAB_WIDTH, WIDGET_TAB_HEIGHT)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    // Resizable on purpose: GTK clamps `gtk_window_resize` to the window's
    // geometry hints, and `resizable(false)` pins those hints to the current
    // size — a fixed-size widget can never grow into the pill. There is no
    // titlebar and no resize border, so nothing can be resized by hand.
    .resizable(true)
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
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|err| format!("could not resize the widget: {err}"))?;
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

/// The widget's single channel to the backend.
///
/// `mode` is one of:
///   * `"full"` / `"region"` — start a capture in the main window;
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
        other => Err(format!("unknown widget mode: {other}")),
    }
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
            "capicorn: widget resize to {} refused ({err}); keeping the current size",
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

// ---------------------------------------------------------------------------
// Pin — a floating, frameless copy of the screenshot
// ---------------------------------------------------------------------------
//
// Snagit's "pin" (and every screenshot tool's "always on top" reference image):
// drop the current capture in a small window over the app you are documenting
// and keep working.
//
// There is NO frontend file behind a pin on purpose. The window loads a
// `data:text/html` document with the image inlined, so:
//   * pins work while the main webview is reloading or busy,
//   * nothing new appears in the bundle for the frontend track to own,
//   * the pin cannot break when the frontend routes change.
//
// The image is injected through an initialization script rather than being
// pasted into the navigation URL: a full-screen PNG is easily >2 MB of base64
// and browsers cap navigation URLs, but a JS global has no such limit.
//
// Wayland caveats (same tao limitations as the widget): `always_on_top` and
// `skip_taskbar` are best-effort — under Wayland the compositor decides
// stacking and window lists. `decorations(false)`, sizing and closing all work.

/// Prefix of every pin window label (`pin-1`, `pin-2`, …).
pub const PIN_LABEL_PREFIX: &str = "pin-";
const PIN_MAX_WIDTH: f64 = 800.0;
const PIN_MAX_HEIGHT: f64 = 600.0;
const PIN_MIN_SIDE: f64 = 120.0;

/// Next label number to try. Never reset: labels must stay unique for the
/// lifetime of the process, and gaps cost nothing.
static PIN_SEQ: AtomicUsize = AtomicUsize::new(1);

/// The pin document. Dark backdrop, image letterboxed to fit, drag anywhere
/// (`data-tauri-drag-region`), close on ✕ or Esc.
const PIN_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Capicorn Pin</title><style>
  html,body{margin:0;padding:0;height:100%;background:#1b1b21;overflow:hidden;
            display:flex;align-items:center;justify-content:center}
  img{max-width:100%;max-height:100%;width:auto;height:auto;display:block;
      object-fit:contain;user-select:none;-webkit-user-drag:none}
  #close{position:fixed;top:4px;right:4px;width:22px;height:22px;border:0;
         border-radius:11px;background:rgba(18,18,22,.55);color:#f4f4f7;
         font:14px/22px system-ui,sans-serif;text-align:center;cursor:pointer;
         opacity:.35;transition:opacity .12s;padding:0}
  #close:hover{opacity:1;background:#c0392b}
</style></head><body data-tauri-drag-region>
  <img id="pin" alt="pinned screenshot" data-tauri-drag-region>
  <button id="close" title="Close pin (Esc)">&#215;</button>
  <script>
    (function () {
      // A frameless window is moved by dragging the image (Tauri's drag-region
      // script calls start_dragging). Its companion double-click handler would
      // maximise the pin, which nobody wants for a reference image — swallow
      // the dblclick in the capture phase, before that handler sees it.
      document.addEventListener('dblclick', function (e) { e.stopPropagation(); }, true);

      var img = document.getElementById('pin');
      var src = window.__CAPICORN_PIN__ || '';
      if (src) { img.src = src; }
      function close() {
        var internals = window.__TAURI_INTERNALS__;
        if (internals && internals.invoke) { internals.invoke('close_pins'); }
      }
      document.getElementById('close').addEventListener('click', close);
      document.addEventListener('keydown', function (e) {
        if (e.key === 'Escape') { close(); }
      });
    })();
  </script>
</body></html>"#;

/// Percent-encode an HTML document into a usable `data:` navigation URL.
///
/// Only RFC 3986 unreserved characters survive; everything else is escaped.
/// Conservative on purpose — `#`, `%`, `?`, `&` and quotes all break a data
/// URL, and the document is ~1.6 KB, so the extra bytes are irrelevant.
fn html_data_url(html: &str) -> String {
    let mut out = String::with_capacity(html.len() * 3);
    out.push_str("data:text/html;charset=utf-8,");
    for byte in html.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Is this exactly `data:image/png;base64,` + the base64 alphabet?
///
/// The data URL is inlined into a JS string literal, so this is a hard
/// requirement, not a nicety: no quote, backslash or newline may survive into
/// the generated script. `decode_png_data_url` already proves it, but the
/// check is repeated at the point the string becomes code.
fn is_plain_png_data_url(data_url: &str) -> bool {
    match data_url.strip_prefix(crate::capture::PNG_DATA_URL_PREFIX) {
        Some(b64) => {
            !b64.is_empty()
                && b64
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
        }
        None => false,
    }
}

/// Window size for a pin: the image's own size, scaled down to
/// [`PIN_MAX_WIDTH`]×[`PIN_MAX_HEIGHT`], with tiny images nudged up so the
/// window is still clickable.
fn pin_size(width: u32, height: u32) -> (f64, f64) {
    let w = width.max(1) as f64;
    let h = height.max(1) as f64;

    let mut scale = (PIN_MAX_WIDTH / w).min(PIN_MAX_HEIGHT / h).min(1.0);
    if w * scale < PIN_MIN_SIDE && h * scale < PIN_MIN_SIDE {
        scale = PIN_MIN_SIDE / w.max(h);
    }

    ((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// First free `pin-<n>` label. `is_taken` lets the caller (and the tests) supply
/// the "does this label exist" question without an `AppHandle`.
fn reserve_pin_label(is_taken: impl Fn(&str) -> bool) -> String {
    loop {
        let n = PIN_SEQ.fetch_add(1, Ordering::Relaxed);
        let label = format!("{PIN_LABEL_PREFIX}{n}");
        if !is_taken(&label) {
            return label;
        }
    }
}

/// Labels of the pins that are currently open.
pub fn open_pin_labels<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    let mut labels: Vec<String> = app
        .webview_windows()
        .into_keys()
        .filter(|label| label.starts_with(PIN_LABEL_PREFIX))
        .collect();
    labels.sort();
    labels
}

/// Pin the image in `data_url` to a new always-on-top, frameless window.
///
/// The window is sized to the image (capped), sits above other windows where
/// the compositor allows it, and can be closed with its ✕, Esc, or
/// [`close_pins`].
#[tauri::command]
pub fn pin_image(app: AppHandle, data_url: String) -> Result<(), String> {
    if !is_plain_png_data_url(&data_url) {
        return Err("pin rejected: expected a data:image/png;base64,… URL".to_string());
    }

    // Decode only to learn the dimensions (the window size). A corrupt payload
    // must not produce an empty window.
    let png = crate::capture::decode_png_data_url(&data_url).map_err(|e| e.to_string())?;
    let image = image::load_from_memory(&png).map_err(|e| format!("invalid PNG payload: {e}"))?;
    let (width, height) = pin_size(image.width(), image.height());

    let label = reserve_pin_label(|candidate| app.get_webview_window(candidate).is_some());
    let url: tauri::Url = html_data_url(PIN_HTML)
        .parse()
        .map_err(|err| format!("could not build the pin document: {err}"))?;

    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title("Capicorn Pin")
        .inner_size(width, height)
        .min_inner_size(80.0, 60.0)
        .decorations(false)
        .resizable(true)
        .skip_taskbar(true)
        .visible(true)
        .focused(true)
        // The image itself: too large to travel inside the navigation URL.
        .initialization_script(format!(
            "window.__CAPICORN_PIN__ = \"{data_url}\";"
        ))
        .build()
        .map_err(|err| format!("could not create the pin window: {err}"))?;

    // Best-effort, as for the widget: Wayland ignores this and the compositor
    // keeps normal stacking until the user adds a "Keep above" rule.
    if let Err(err) = window.set_always_on_top(true) {
        eprintln!("capicorn: pin always-on-top not supported here ({err}); normal stacking applies");
    }

    Ok(())
}

/// Close every pin window; returns how many were closed.
#[tauri::command]
pub fn close_pins(app: AppHandle) -> Result<usize, String> {
    let labels = open_pin_labels(&app);
    let mut closed = 0;
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            if window.close().is_ok() {
                closed += 1;
            }
        }
    }
    Ok(closed)
}

/// Saved main-window size while the scrolling-capture card owns the window.
#[derive(Default)]
pub struct ScrollWindowSize(pub Mutex<Option<tauri::LogicalSize<f64>>>);

/// Shrink/restore the main window for the scrolling-capture card.
///
/// The card used to be a DOM overlay inside a full-size **click-through** window:
/// its Done button was unclickable (clicks passed through to the app below) and
/// Enter went to whatever window the user had scrolled. Shrinking the window to
/// the card (and turning click-through off) keeps Done clickable without the
/// overlay blocking the page being scrolled.
#[tauri::command]
pub fn set_scroll_mode(
    app: AppHandle,
    saved: State<'_, ScrollWindowSize>,
    enabled: bool,
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
        win.set_size(tauri::LogicalSize::new(372.0, 330.0))
            .map_err(|e| format!("cannot shrink the window for the scrolling card: {e}"))?;
        let _ = win.set_always_on_top(true);
        let _ = win.set_focus();
    } else {
        let restore = saved.0.lock().ok().and_then(|g| *g);
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
        // Collapsed must be strictly smaller than expanded, or the window resize
        // would be a no-op and the widget would keep blocking desktop clicks
        // around the tab.
        assert!(WIDGET_TAB_WIDTH < WIDGET_WIDTH);
        assert!(WIDGET_TAB_HEIGHT < WIDGET_HEIGHT);

        // Content boxes must match src/widget.css exactly:
        //   window − (6px + 6px horizontal, 5px + 5px vertical padding)
        assert_eq!(WIDGET_TAB_WIDTH - 12.0, 44.0, "tab width");
        assert_eq!(WIDGET_TAB_HEIGHT - 10.0, 44.0, "tab height");
        assert_eq!(WIDGET_WIDTH - 12.0, 240.0, "pill width");
        assert_eq!(WIDGET_HEIGHT - 10.0, 50.0, "pill height");
    }

    #[test]
    fn pin_html_url_is_percent_encoded() {
        let url = html_data_url(PIN_HTML);
        assert!(url.starts_with("data:text/html;charset=utf-8,"));
        // Nothing raw that would break the navigation URL.
        assert!(!url.contains('#'));
        assert!(!url.contains(' '));
        assert!(!url.contains('"'));
        assert!(url.contains("%23"), "css colours must be escaped");
        assert!(url.parse::<tauri::Url>().is_ok(), "must parse as a URL");
    }

    #[test]
    fn pin_html_keeps_the_image_hook() {
        // The document and the initialization script must agree on the global.
        assert!(PIN_HTML.contains("__CAPICORN_PIN__"));
        assert!(PIN_HTML.contains("close_pins"));
        // Frameless: the image is the only thing that can move the window.
        assert!(PIN_HTML.contains("data-tauri-drag-region"));
    }

    #[test]
    fn pin_size_caps_and_floors() {
        // Small images are blown up to something clickable.
        let (w, h) = pin_size(3, 2);
        assert!(w >= PIN_MIN_SIDE || h >= PIN_MIN_SIDE, "tiny pin: {w}x{h}");
        assert!((w / h - 1.5).abs() < 0.05, "aspect ratio preserved: {w}x{h}");

        // Large images are capped, aspect preserved.
        let (w, h) = pin_size(3840, 2160);
        assert!(w <= PIN_MAX_WIDTH + 0.5, "width capped: {w}");
        assert!(h <= PIN_MAX_HEIGHT + 0.5, "height capped: {h}");
        assert!((w / h - 3840.0 / 2160.0).abs() < 0.02, "aspect: {w}x{h}");

        // In-between sizes are left alone.
        assert_eq!(pin_size(640, 480), (640.0, 480.0));
        // Degenerate input never yields a zero-sized window.
        assert_eq!(pin_size(0, 0), (PIN_MIN_SIDE, PIN_MIN_SIDE));
    }

    #[test]
    fn pin_labels_are_unique_and_prefixed() {
        let first = reserve_pin_label(|_| false);
        assert!(first.starts_with(PIN_LABEL_PREFIX));
        // A taken label is skipped, never reused.
        let taken = first.clone();
        let second = reserve_pin_label(|candidate| candidate == taken);
        assert_ne!(first, second);
        assert!(second.starts_with(PIN_LABEL_PREFIX));
    }

    #[test]
    fn pin_input_must_be_plain_png_data_url() {
        assert!(is_plain_png_data_url("data:image/png;base64,AAECAw=="));
        // Quotes or script syntax must never reach the injected JS string.
        assert!(!is_plain_png_data_url("data:image/png;base64,AA\";alert(1);//"));
        assert!(!is_plain_png_data_url("data:image/png;base64,"));
        assert!(!is_plain_png_data_url("data:image/jpeg;base64,AAAA"));
        assert!(!is_plain_png_data_url("https://example.com/a.png"));
    }
}