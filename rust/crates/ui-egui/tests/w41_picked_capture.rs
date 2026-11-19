//! W41 gates: the restored Tauri window/monitor system-picker capture.
//!
//! The Rust port had dropped `capture_window`/`capture_monitor` (the ScreenCast
//! single-frame grab that shows the OS window/screen picker); "Choose screen…"
//! on Linux was a dead-end status line. These gates pin the wiring: the picked
//! methods arm the shared capture pipeline with the right picker status, the
//! Linux monitor path opens the system picker instead of the dead-end, and the
//! in-flight guard holds. The hide/settle/restore + Done landing themselves are
//! the shared `poll_capture` path already gated by w29_capture_hide; the live
//! portal round trip needs an unlocked desktop (capture crate's ignored test).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use snagcraft_ui_egui::{SettingsView, SnagCraftApp};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w41-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app(dir: &std::path::Path) -> SnagCraftApp {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    app
}

/// The window picker arms the shared capture pipeline and tells the user a
/// picker is coming (the Screenshot portal never shows one — this is the
/// ScreenCast path). Asserted synchronously: the worker thread is spawned but
/// the live portal answer needs an unlocked desktop, so we do not wait on it.
#[test]
fn window_picked_arms_the_capture_with_picker_status() {
    let dir = scratch("window");
    let mut a = app(&dir);
    a.capture_window_picked();
    assert!(a.capture_in_flight(), "the picked-capture worker was armed");
    assert_eq!(a.status, "Pick a window to capture…");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn monitor_picked_arms_the_capture_with_picker_status() {
    let dir = scratch("monitor");
    let mut a = app(&dir);
    a.capture_monitor_picked();
    assert!(a.capture_in_flight());
    assert_eq!(a.status, "Pick a screen to capture…");
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Choose screen…" on Linux (no in-app output enumeration) must open the
/// ScreenCast system picker — not the old dead-end status line.
#[test]
#[cfg(target_os = "linux")]
fn choose_screen_on_linux_opens_the_system_picker() {
    let dir = scratch("choose");
    let mut a = app(&dir);
    a.open_monitor_picker();
    assert!(
        a.capture_in_flight(),
        "Linux Choose screen… arms the system-picker capture"
    );
    assert_eq!(a.status, "Pick a screen to capture…");
    assert!(
        !a.status.contains("start a recording"),
        "the W25 dead-end status line is gone"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A second picked capture while one is in flight is ignored (the shared
/// capture_rx guard), so two picker dialogs can never stack.
#[test]
fn a_second_picked_capture_while_in_flight_is_ignored() {
    let dir = scratch("guard");
    let mut a = app(&dir);
    let _tx = a.seed_capture_channel();
    a.status = "already capturing".into();
    a.capture_window_picked();
    assert_eq!(
        a.status, "already capturing",
        "the guard kept the first capture"
    );
    a.capture_monitor_picked();
    assert_eq!(a.status, "already capturing");
    let _ = std::fs::remove_dir_all(&dir);
}
