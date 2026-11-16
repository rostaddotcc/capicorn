//! System tray — `tray-icon` on the **ksni** (StatusNotifierItem) backend.
//!
//! Ported from the live `src-tauri/src/tray.rs`, with `tauri::` stripped: the
//! events that the Tauri build emitted to the frontend (`tray-capture`,
//! `show_main`, `toggle_widget_for`, `request_quit`) now arrive as
//! [`TrayEvent`] values on the caller's channel.
//!
//! Why ksni: the default libappindicator backend emits no mouse events at all,
//! so click-to-restore would be impossible on Wayland. ksni maps KDE's
//! `Activate` to a left click and `SecondaryActivate` to a middle click, and
//! serves the menu over DBusMenu (host-rendered on right click).
//!
//! Interactions:
//! - LEFT click (or double click) → [`TrayEvent::Show`]. SNI has no native
//!   double click: a double click is two left clicks and the first one has
//!   already restored the window, so the gesture works either way.
//! - MIDDLE click → [`TrayEvent::ToggleWidget`].
//! - Menu: `Show SnagCraft` / `Capture` / `Floating widget` / ─ / `Quit`.
//!
//! Degradation: no DBus session means no tray host, which is a
//! [`PlatformError::Tray`] from [`spawn_tray`] and `TRAY_READY == false` — the
//! app then keeps a normal window and never hides to a tray that is not there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::thread::{self, JoinHandle};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::PlatformError;
use crate::png;

/// Whether a tray icon actually exists in this session. Hide-to-tray is only
/// safe when it does — otherwise the window would vanish with no way back.
static TRAY_READY: AtomicBool = AtomicBool::new(false);

const ID_SHOW: &str = "tray-show";
const ID_CAPTURE: &str = "tray-capture-item";
const ID_WIDGET: &str = "tray-widget";
const ID_QUIT: &str = "tray-quit";

const TOOLTIP: &str = "SnagCraft — click to open";

/// What the shell should do in response to the tray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left click / double click / `Show SnagCraft`.
    Show,
    /// `Capture` menu item — the app starts the capture flow.
    Capture,
    /// Middle click / `Floating widget`.
    ToggleWidget,
    /// `Quit` menu item — bypasses the hide-to-tray guard.
    Quit,
}

/// Keeps the tray icon alive. Dropping it detaches the tray thread: the icon
/// stays until the process exits, which is what a shell wants.
pub struct TrayHandle {
    _thread: JoinHandle<()>,
}

impl TrayHandle {
    /// True once the StatusNotifierItem is registered with the host.
    pub fn is_available(&self) -> bool {
        TRAY_READY.load(Ordering::SeqCst)
    }
}

/// Whether a tray icon exists in this session (the live `TRAY_READY` gate).
pub fn tray_available() -> bool {
    TRAY_READY.load(Ordering::SeqCst)
}

/// Build the tray icon and start forwarding its interactions to `events`.
///
/// `icon_png` is the PNG the caller embedded with `include_bytes!` (32×32 is the
/// AppIndicator standard size). The builder runs on a dedicated thread — ksni
/// needs its own event loop — and this function only returns once that thread
/// has either registered the icon or failed, so a headless session gets a clean
/// `Err` instead of a silently dead tray.
pub fn spawn_tray(icon_png: &[u8], events: Sender<TrayEvent>) -> Result<TrayHandle, PlatformError> {
    let image = png::decode(icon_png).map_err(|err| PlatformError::Tray(err.to_string()))?;
    let (ready_tx, ready_rx) = channel::<Result<(), String>>();

    let thread = thread::Builder::new()
        .name("snagcraft-tray".to_string())
        .spawn(move || run_tray(image, events, &ready_tx))
        .map_err(|err| PlatformError::Tray(format!("cannot spawn the tray thread: {err}")))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(TrayHandle { _thread: thread }),
        Ok(Err(msg)) => Err(PlatformError::Tray(msg)),
        Err(_) => Err(PlatformError::Tray(
            "the tray thread exited before the icon was registered".to_string(),
        )),
    }
}

fn run_tray(image: png::PngImage, events: Sender<TrayEvent>, ready: &Sender<Result<(), String>>) {
    let icon = match icon_for(image) {
        Ok(icon) => icon,
        Err(msg) => {
            let _ = ready.send(Err(msg));
            return;
        }
    };

    let menu = match build_menu() {
        Ok(menu) => menu,
        Err(msg) => {
            let _ = ready.send(Err(msg));
            return;
        }
    };

    // `tray-icon` keeps the menu (and its DBusMenu snapshot) alive, and drops
    // the SNI service when the icon is dropped — so it must stay in scope.
    let tray = match TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip(TOOLTIP)
        .with_menu(Box::new(menu))
        .build()
    {
        Ok(tray) => tray,
        Err(err) => {
            let _ = ready.send(Err(format!("no tray host: {err}")));
            return;
        }
    };

    spawn_menu_listener(events.clone());
    spawn_icon_listener(events);

    TRAY_READY.store(true, Ordering::SeqCst);
    tracing::info!("tray icon registered");
    let _ = ready.send(Ok(()));

    // Park holding `tray`: dropping it would unregister the icon and tear down
    // the SNI service. The process exiting is the only teardown a tray needs.
    let _keep_alive = &tray;
    loop {
        thread::park();
    }
}

fn icon_for(image: png::PngImage) -> Result<tray_icon::Icon, String> {
    tray_icon::Icon::from_rgba(image.rgba, image.width, image.height).map_err(|err| {
        format!(
            "unusable tray icon ({}x{}): {err}",
            image.width, image.height
        )
    })
}

fn build_menu() -> Result<Menu, String> {
    let menu = Menu::new();
    let show = MenuItem::with_id(ID_SHOW, "Show SnagCraft", true, None);
    let capture = MenuItem::with_id(ID_CAPTURE, "Capture", true, None);
    let widget = MenuItem::with_id(ID_WIDGET, "Floating widget", true, None);
    let separator = PredefinedMenuItem::separator();
    let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
    menu.append_items(&[&show, &capture, &widget, &separator, &quit])
        .map_err(|err| format!("cannot build the tray menu: {err}"))?;
    Ok(menu)
}

fn spawn_menu_listener(events: Sender<TrayEvent>) {
    // The receiver is a process-wide static: the DBusMenu host activates items
    // through it, and it must be consumed on a thread that never blocks the UI.
    let receiver = MenuEvent::receiver();
    let spawned = thread::Builder::new()
        .name("snagcraft-tray-menu".to_string())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                let mapped = match event.id().as_ref() {
                    ID_SHOW => Some(TrayEvent::Show),
                    ID_CAPTURE => Some(TrayEvent::Capture),
                    ID_WIDGET => Some(TrayEvent::ToggleWidget),
                    ID_QUIT => Some(TrayEvent::Quit),
                    _ => None,
                };
                if let Some(tray_event) = mapped {
                    tracing::debug!(?tray_event, "tray menu event");
                    if events.send(tray_event).is_err() {
                        break;
                    }
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("cannot spawn the tray menu listener: {err}");
    }
}

fn spawn_icon_listener(events: Sender<TrayEvent>) {
    let receiver = TrayIconEvent::receiver();
    let spawned = thread::Builder::new()
        .name("snagcraft-tray-icons".to_string())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                let mapped = match event {
                    // ksni reports every left click as Click/Up; a double click
                    // is simply two of these, and restoring twice is a no-op.
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => Some(TrayEvent::Show),
                    TrayIconEvent::DoubleClick { .. } => Some(TrayEvent::Show),
                    TrayIconEvent::Click {
                        button: MouseButton::Middle,
                        button_state: MouseButtonState::Up,
                        ..
                    } => Some(TrayEvent::ToggleWidget),
                    _ => None,
                };
                if let Some(tray_event) = mapped {
                    tracing::debug!(?tray_event, "tray icon event");
                    if events.send(tray_event).is_err() {
                        break;
                    }
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("cannot spawn the tray icon listener: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1×1 opaque red RGBA PNG (stored-deflate friendly, 70 bytes).
    const ONE_PIXEL_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 248, 207, 192, 240,
        31, 0, 5, 0, 1, 255, 86, 199, 47, 13, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn garbage_icon_bytes_are_a_tray_error() {
        let (tx, _rx) = channel();
        match spawn_tray(b"definitely not a png", tx) {
            Err(PlatformError::Tray(msg)) => assert!(msg.contains("png")),
            Err(other) => panic!("unexpected error kind: {other}"),
            Ok(_) => panic!("garbage icon bytes must not produce a tray"),
        }
    }

    /// Live test: needs a desktop session with a StatusNotifierWatcher.
    #[test]
    #[ignore = "requires a running tray host (DBus session + SNI watcher)"]
    fn spawn_tray_never_panics_headless() {
        let (tx, _rx) = channel();
        match spawn_tray(ONE_PIXEL_PNG, tx) {
            Ok(handle) => assert!(handle.is_available()),
            Err(PlatformError::Tray(_)) => assert!(!tray_available()),
            Err(other) => panic!("unexpected error kind: {other}"),
        }
    }
}
