//! System tray icon + menu.
//!
//! Linux reality (libayatana-appindicator3): mouse *events* on the tray icon are
//! not emitted at all, so a click handler would never fire. Every action
//! therefore lives in the MENU, and `show_menu_on_left_click(true)` makes the
//! left click open that menu — the only interaction the platform gives us.
//!
//! Menu items:
//! - `Show Capicorn` — show/unminimize + focus the main window
//! - `Capture`       — emit `tray-capture`; the frontend starts the capture flow
//! - `Quit`          — set the force-quit flag, then exit (bypasses the close guard)

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager, Runtime};

/// Whether a tray icon actually exists in this session. Hide-to-tray is only
/// safe when it does — otherwise the window would vanish with no way back.
static TRAY_READY: AtomicBool = AtomicBool::new(false);

pub fn is_available() -> bool {
    TRAY_READY.load(Ordering::SeqCst)
}

/// Small tray icon. 32x32 is the AppIndicator standard size.
const TRAY_ICON_PNG: &[u8] = include_bytes!("../icons/32x32.png");

const ID_SHOW: &str = "tray-show";
const ID_CAPTURE: &str = "tray-capture-item";
const ID_QUIT: &str = "tray-quit";

/// Show the main window from anywhere (tray click, second launch, …).
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// Build the tray icon and menu. Called once from `lib.rs` setup.
pub fn init_tray<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id(ID_SHOW, "Show Capicorn").build(app)?;
    let capture = MenuItemBuilder::with_id(ID_CAPTURE, "Capture").build(app)?;
    let quit = MenuItemBuilder::with_id(ID_QUIT, "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .items(&[&show, &capture, &quit])
        .build()?;

    let icon = tauri::image::Image::from_bytes(TRAY_ICON_PNG)?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("Capicorn — screen capture")
        .menu(&menu)
        // Linux emits no tray click events; the menu is the interaction, so open
        // it on a normal left click instead of requiring right click.
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_SHOW => show_main(app),
            ID_CAPTURE => {
                // Frontend owns the capture flow (window hide, countdown,
                // editor); the tray only signals intent.
                let _ = app.emit("tray-capture", ());
            }
            ID_QUIT => crate::windows::request_quit(app),
            _ => {}
        })
        .build(app)?;

    TRAY_READY.store(true, Ordering::SeqCst);
    Ok(())
}
