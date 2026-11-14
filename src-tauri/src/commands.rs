//! Tauri command surface — the locked contract with the frontend.
//!
//! | command              | args (camelCase)                     | returns                                  |
//! |----------------------|--------------------------------------|------------------------------------------|
//! | `capture_screenshot` | —                                    | `{ dataUrl, width, height }`             |
//! | `save_png`           | `dataUrl`, `suggestedName`           | saved path (`string`)                    |
//! | `copy_png`           | `dataUrl`                            | `void`                                   |
//! | `get_version`        | —                                    | crate version (`string`)                 |
//! | `set_fullscreen`     | `full`                               | `void`                                   |
//! | `default_save_dir`   | —                                    | `<pictures>/Capicorn` (`string`)         |
//! | `pick_save_dir`      | —                                    | chosen folder (`string`)                 |
//! | `save_png_to_dir`    | `dataUrl`, `dir`, `filename`         | saved path (`string`)                    |
//!
//! `dataUrl` is always a full `data:image/png;base64,…` URL. Errors become
//! rejected promises carrying the message string.

use serde::Serialize;
use tauri_plugin_dialog::DialogExt;

/// What `capture_screenshot` hands the frontend (serialized camelCase).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResult {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

/// How long to let the compositor redraw after hiding our own window before
/// grabbing the screen. Short enough to feel instant, long enough that the
/// window (and its shadow/animation) is genuinely gone — otherwise repeated
/// captures would nest the app inside its own screenshot.
const HIDE_SETTLE_MS: u64 = 220;

#[tauri::command]
pub async fn capture_screenshot(app: tauri::AppHandle) -> Result<CaptureResult, String> {
    use tauri::Manager as _;

    let win = app.get_webview_window("main");

    if let Some(win) = win.as_ref() {
        let _ = win.hide();
        tokio::time::sleep(std::time::Duration::from_millis(HIDE_SETTLE_MS)).await;
    }

    // Capture first, restore second, return last: the restore must happen on
    // both the success and the error path, so the window can never be left
    // hidden behind a failed screenshot.
    let captured = crate::capture::capture_full().await;

    if let Some(win) = win.as_ref() {
        let _ = win.show();
        let _ = win.set_focus();
    }

    let (data_url, width, height) = captured.map_err(|e| e.to_string())?;
    Ok(CaptureResult {
        data_url,
        width,
        height,
    })
}

/// Ask the user where to save, then write the PNG there and return the path.
///
/// `blocking_save_file` is safe here: async commands run off the Tauri main
/// thread, and the plugin hops to the main thread for the native dialog itself.
#[tauri::command]
pub async fn save_png(
    app: tauri::AppHandle,
    data_url: String,
    suggested_name: String,
) -> Result<String, String> {
    let png = crate::capture::decode_png_data_url(&data_url).map_err(|e| e.to_string())?;
    // Validate before showing a dialog so a corrupt payload never produces an
    // empty file the user thinks they saved.
    image::load_from_memory(&png).map_err(|e| format!("invalid PNG payload: {e}"))?;

    let file_name = if suggested_name.trim().is_empty() {
        "capicorn.png".to_string()
    } else {
        suggested_name
    };

    let chosen = app
        .dialog()
        .file()
        .add_filter("PNG image", &["png"])
        .set_file_name(&file_name)
        .blocking_save_file()
        .ok_or_else(|| "save cancelled".to_string())?;

    let path = chosen
        .into_path()
        .map_err(|e| format!("unsupported save location: {e}"))?;

    std::fs::write(&path, &png).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// The folder a screenshot lands in when the user picks "default save location":
/// `<Pictures>/Capicorn`. Purely a suggestion — nothing is created here, because
/// `save_png_to_dir` does the `create_dir_all` once a save actually happens.
#[tauri::command]
pub fn default_save_dir(app: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager as _;

    let resolver = app.path();
    let pictures = resolver
        .picture_dir()
        .or_else(|_| resolver.home_dir().map(|home| home.join("Pictures")))
        .map_err(|e| format!("cannot resolve a pictures directory: {e}"))?;

    Ok(pictures.join("Capicorn").to_string_lossy().into_owned())
}

/// Ask the user for a folder (no file selection) and hand the path back.
#[tauri::command]
pub async fn pick_save_dir(app: tauri::AppHandle) -> Result<String, String> {
    let chosen = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .ok_or_else(|| "cancelled".to_string())?;

    let path = chosen
        .into_path()
        .map_err(|e| format!("unsupported folder selection: {e}"))?;

    Ok(path.to_string_lossy().into_owned())
}

/// Write a screenshot into `dir` under `filename`, creating `dir` if needed.
///
/// The filename is untrusted frontend input, so anything that could escape the
/// target directory (separators, `..`, empty) falls back to `capicorn.png`.
#[tauri::command]
pub async fn save_png_to_dir(data_url: String, dir: String, filename: String) -> Result<String, String> {
    let png = crate::capture::decode_png_data_url(&data_url).map_err(|e| e.to_string())?;
    image::load_from_memory(&png).map_err(|e| format!("invalid PNG payload: {e}"))?;

    if dir.trim().is_empty() {
        return Err("no save directory given".to_string());
    }
    let dir = std::path::PathBuf::from(&dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let name = safe_filename(&filename);
    let path = dir.join(&name);
    std::fs::write(&path, &png).map_err(|e| format!("cannot write {}: {e}", path.display()))?;

    Ok(path.to_string_lossy().into_owned())
}

/// Reduce arbitrary user input to a single plain file name inside the target dir.
fn safe_filename(filename: &str) -> String {
    let trimmed = filename.trim();
    let hostile = trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0');
    if hostile {
        "capicorn.png".to_string()
    } else {
        trimmed.to_string()
    }
}

#[tauri::command]
pub async fn copy_png(data_url: String) -> Result<(), String> {
    crate::clipboard::copy_png_data_url(&data_url).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Toggle true fullscreen on the main window.
///
/// The frontend uses this so a whole-screen region capture isn't competing with
/// the window's own chrome. A missing window (already closing) is not an error:
/// there is simply nothing left to resize.
#[tauri::command]
pub fn set_fullscreen(app: tauri::AppHandle, full: bool) -> Result<(), String> {
    use tauri::Manager as _;

    if let Some(win) = app.get_webview_window("main") {
        win.set_fullscreen(full).map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_result_serializes_camel_case() {
        let value = serde_json::to_value(CaptureResult {
            data_url: "data:image/png;base64,AA".into(),
            width: 1,
            height: 2,
        })
        .expect("serialize CaptureResult");

        assert_eq!(value["dataUrl"], "data:image/png;base64,AA");
        assert_eq!(value["width"], 1);
        assert_eq!(value["height"], 2);
        assert!(value.get("data_url").is_none(), "frontend expects dataUrl");
    }

    #[test]
    fn version_is_reported() {
        assert!(!get_version().is_empty());
    }
}
