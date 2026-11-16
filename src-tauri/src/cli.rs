//! OS integration: single instance + launch requests.
//!
//! SnagCraft behaves like a system application, not a web page:
//! - a second launch never opens a second window — it focuses the running one
//!   (tauri-plugin-single-instance forwards the new argv to [`dispatch`]);
//! - `snagcraft --capture` / `--region` asks the running instance to capture
//!   (this is what the GNOME/KDE panel companions in `integrations/` call);
//! - an image path in argv (file association / "Open with SnagCraft") is loaded
//!   into the editor.
//!
//! The FIRST launch is polled by the frontend via [`launch_request`] once its
//! event listeners are up; later launches arrive as `cli-capture` / `cli-open`
//! events emitted by [`dispatch`].

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// What the OS asked this launch to do. Both fields are optional: a plain
/// launch (or a bare second instance) only brings the window forward.
#[derive(Default, Serialize)]
pub struct LaunchRequest {
    /// Capture mode: "full" or "region".
    pub capture: Option<String>,
    /// Image file to open (file association / "Open with").
    pub file: Option<String>,
}

const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "avif", "tif", "tiff",
];

fn is_image_path(arg: &str) -> bool {
    let path = std::path::Path::new(arg);
    if !path.is_file() {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Parse CLI arguments (without argv[0]). Unknown flags are ignored so OS
/// wrappers (AppImage runtimes, installers) can pass their own flags safely.
pub fn parse_args(args: &[String]) -> LaunchRequest {
    let mut req = LaunchRequest::default();
    for arg in args {
        match arg.as_str() {
            "--capture" | "--capture-full" => req.capture = Some("full".into()),
            "--region" | "--capture-region" => req.capture = Some("region".into()),
            _ => {
                if req.file.is_none() && !arg.starts_with('-') && is_image_path(arg) {
                    req.file = Some(arg.clone());
                }
            }
        }
    }
    req
}

/// Bring the main window forward and hand the request to the frontend.
pub fn dispatch(app: &AppHandle, req: &LaunchRequest) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
    if let Some(mode) = &req.capture {
        let _ = app.emit_to("main", "cli-capture", serde_json::json!({ "mode": mode }));
    } else if let Some(path) = &req.file {
        let _ = app.emit_to("main", "cli-open", serde_json::json!({ "path": path }));
    }
}

/// Frontend polls this once at startup: what did the FIRST launch ask for?
#[tauri::command]
pub fn launch_request() -> LaunchRequest {
    parse_args(&std::env::args().skip(1).collect::<Vec<_>>())
}

/// `--help` / `--version` are answered before the GUI starts.
pub fn early_exit(args: &[String]) -> bool {
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "SnagCraft {} — screen capture, annotation and recording\n\n\
                     Usage: snagcraft [OPTIONS] [IMAGE]\n\n\
                     Options:\n  \
                       --capture    Capture the full screen (in the running instance)\n  \
                       --region     Capture a region (in the running instance)\n  \
                       -h, --help     Print this help\n  \
                       -V, --version  Print the version\n\n\
                     IMAGE is opened in the editor (file association / Open with).",
                    env!("CARGO_PKG_VERSION")
                );
                return true;
            }
            "--version" | "-V" => {
                println!("SnagCraft {}", env!("CARGO_PKG_VERSION"));
                return true;
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capture_flags() {
        let req = parse_args(&["--capture".into()]);
        assert_eq!(req.capture.as_deref(), Some("full"));
        let req = parse_args(&["--region".into()]);
        assert_eq!(req.capture.as_deref(), Some("region"));
    }

    #[test]
    fn ignores_unknown_flags() {
        let req = parse_args(&["--appimage-mount".into(), "--foo=bar".into()]);
        assert!(req.capture.is_none());
        assert!(req.file.is_none());
    }

    #[test]
    fn picks_existing_image_file() {
        let dir = std::env::temp_dir().join("snagcraft-cli-test");
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("shot.png");
        std::fs::write(&img, b"not really a png").unwrap();
        let req = parse_args(&[img.to_string_lossy().into_owned()]);
        assert_eq!(req.file.as_deref(), Some(img.to_str().unwrap()));
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, b"hello").unwrap();
        let req = parse_args(&[txt.to_string_lossy().into_owned()]);
        assert!(req.file.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
