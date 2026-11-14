mod capture;
mod clipboard;
mod commands;
mod export;
mod hotkeys;
mod tray;
mod windows;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Close-guard state: force-quit flag, editor-dirty flag, minimize-to-tray
        // preference. All three are driven by the commands in `windows.rs`.
        .manage(windows::AppFlags::new())
        // Global-hotkey state: the live backend, the granted combos and the
        // handles that keep the portal session alive (see `hotkeys.rs`).
        .manage(hotkeys::HotkeyState::new())
        .invoke_handler(tauri::generate_handler![
            commands::capture_screenshot,
            commands::save_png,
            commands::copy_png,
            commands::get_version,
            commands::set_fullscreen,
            commands::default_save_dir,
            commands::pick_save_dir,
            commands::save_png_to_dir,
            export::save_image,
            windows::force_close,
            windows::set_dirty,
            windows::is_dirty,
            windows::set_minimize_to_tray,
            windows::get_minimize_to_tray,
            // Floating quick-capture widget
            windows::open_widget,
            windows::close_widget,
            windows::toggle_widget,
            windows::widget_capture,
            // Wayland GlobalShortcuts portal (degrades to backend="none")
            hotkeys::register_hotkeys,
            hotkeys::get_hotkey_status,
            hotkeys::unregister_hotkeys
        ])
        .setup(|app| {
            // Order matters: the close guard must exist before the window can be
            // closed, and the tray is the only way back in after minimizing.
            windows::register_close_guard(app);
            // A missing appindicator/desktop integration is not fatal: log it and
            // carry on. `tray::is_available()` then keeps close-to-tray disabled
            // so the window can never hide with no way back.
            if let Err(err) = tray::init_tray(app) {
                eprintln!("capicorn: system tray unavailable ({err}); continuing without it");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Capicorn");
}