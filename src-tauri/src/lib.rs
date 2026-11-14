mod capture;
mod clipboard;
mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::capture_screenshot,
            commands::save_png,
            commands::copy_png,
            commands::get_version,
            commands::set_fullscreen
        ])
        .run(tauri::generate_context!())
        .expect("error while running Capicorn");
}