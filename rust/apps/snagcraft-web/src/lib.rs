#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! SnagCraft web entry point (W5d).
//!
//! The browser build is the *shell-less* shell: `eframe::WebRunner` owns one
//! canvas (`#snagcraft_canvas`, declared in `index.html`) and pumps the same
//! `snagcraft_ui_egui::app_ui` the desktop window runs. `SnagCraftApp`
//! already implements `eframe::App`, so no extra wrapper is needed.
//!
//! What differs from the desktop shell is decided by the ui-egui `native`
//! feature, which the web app turns off: no tray, no global hotkeys, no screen
//! capture, no rfd file dialogs and no floating widget viewport. Open is a
//! dropped file, export is a Blob download, settings live in memory.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast as _;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::JsValue;

/// Host builds of this browser artifact are an empty compile-check stub (see
/// `start` below). Naming the browser-half dependencies keeps cargo's
/// `unused_dependencies` lint quiet without pulling wasm-only APIs in.
#[cfg(not(target_arch = "wasm32"))]
#[allow(unused_imports)]
mod host_stub {
    use console_error_panic_hook as _;
    use snagcraft_ui_egui as _;
    use wasm_bindgen_futures as _;
    use web_sys as _;
}

/// The canvas id `index.html` declares and `WebRunner` binds to.
#[cfg(target_arch = "wasm32")]
const CANVAS_ID: &str = "snagcraft_canvas";

/// `wasm_bindgen(start)` — runs once when the module is instantiated.
///
/// The runner is async, and a wasm start function cannot be, so the work is
/// handed to `wasm-bindgen-futures`. A failure is logged to the console rather
/// than thrown: a dead tab with a stack trace is worse than a message.
///
/// Everything below is wasm32-only: `eframe::WebRunner`/`WebOptions` do not
/// exist on a host build, and the crate is a browser artifact by definition —
/// the empty host build keeps `cargo check --workspace` (and its clippy) green.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    tracing::info!("snagcraft-web booting");
    wasm_bindgen_futures::spawn_local(async {
        if let Err(err) = run().await {
            web_sys::console::error_1(&err);
        }
    });
    Ok(())
}

/// Find the canvas and hand it to eframe.
#[cfg(target_arch = "wasm32")]
async fn run() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id(CANVAS_ID)
        .ok_or_else(|| JsValue::from_str("missing <canvas id=\"snagcraft_canvas\">"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| JsValue::from_str("#snagcraft_canvas is not a canvas"))?;

    // Settings are in memory on the web (`doc.save`/`settings.set` never touch
    // the filesystem), so there is no persistence hook to install here. The
    // browser has no settings file to read either, so the in-memory default is
    // the dark theme the page itself is drawn in.
    let app_creator: eframe::AppCreator<'static> = Box::new(|cc| {
        // egui 0.37 keeps one style per theme preference; pin the preference to
        // dark first so `apply_theme`'s visuals are the ones that resolve.
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        snagcraft_ui_egui::apply_theme(&cc.egui_ctx, snagcraft_ui_egui::Theme::Dark);
        let mut app = snagcraft_ui_egui::SnagCraftApp::new();
        let mut settings = app.settings.clone();
        settings.theme = snagcraft_ui_egui::Theme::Dark;
        app.push_settings(settings);
        app.status = "Ready — drop an image here to annotate it.".into();
        Ok(Box::new(app))
    });

    eframe::WebRunner::new()
        .start(canvas, eframe::WebOptions::default(), app_creator)
        .await
}
