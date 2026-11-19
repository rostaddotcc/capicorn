//! W7 parity gates: the OCR language picker (B4#1) and the OCR busy modal
//! (B4#2).
//!
//! Everything runs headless: the language list is injected so the picker is
//! deterministic, and `ocr.run` is exercised through the real off-thread worker
//! only when tesseract is actually installed.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("snagcraft-w7-ocr-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Deterministic shell: no host settings file, no floating widget.
fn harness() -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        save_dir: Some(scratch_dir().to_string_lossy().to_string()),
        ..SettingsView::default()
    };
    app.widget_expanded = true;
    app.ocr_available = Some(true);
    Harness::builder()
        .with_size(egui::vec2(900.0, 640.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

/// B4#1: the picker lists `ocr.languages` and the selection is persisted through
/// `settings.set` — the engine holds it, and `run_ocr` sends that code.
#[test]
fn ocr_language_picker_lists_and_persists_the_selection() {
    let mut h = harness();
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.ocr.languages = Some(vec!["eng".to_string(), "swe".to_string()]);
        app.ocr.open = true;
    }
    h.run_steps(2);

    // The window owns the picker; `eng` is the default selection.
    let _ = h.get_by_label("Language");
    let picker = h.get_by_value("English (eng)");
    picker.click();
    h.run_steps(2);
    h.get_by_label_contains("Swedish (swe)").click();
    h.run_steps(2);

    assert_eq!(h.state().settings.ocr_lang, "swe", "panel mirror updated");
    assert_eq!(h.state().selected_ocr_lang(), "swe");
    assert_eq!(
        h.state().sess().settings().ocr_lang,
        "swe",
        "settings.set reached the engine's settings"
    );
    h.snapshot("w7_ocr_language_picker");
}

/// A language the panel persisted but the picker list lacks is still shown, so
/// the selected code is never invisible.
#[test]
fn a_persisted_language_outside_the_list_is_still_selectable() {
    let mut h = harness();
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.settings.ocr_lang = "swe".to_string();
        app.ocr.languages = Some(vec!["eng".to_string()]);
        app.ocr.open = true;
    }
    h.run_steps(2);
    let _ = h.get_by_value("Swedish (swe)");
    // An invalid code is never forwarded to the engine.
    h.state_mut().set_ocr_lang("swe;rm".to_string());
    assert!(
        h.state().status.contains("not a valid code"),
        "the rejection is reported: {}",
        h.state().status
    );
    assert_eq!(
        h.state().sess().settings().ocr_lang,
        "eng",
        "a hostile code never reaches settings.set"
    );
}

/// B4#2: while `ocr.run` is in flight the window shows progress and Extract is
/// disabled; the result clears the modal and shows the text.
#[test]
fn ocr_busy_modal_shows_progress_and_clears_on_result() {
    let mut h = harness();
    h.run_steps(2);
    {
        let app = h.state_mut();
        app.ocr.open = true;
        app.ocr.busy = true;
        app.status = "busy modal".to_string();
    }
    h.run_steps(2);

    h.get_by_label_contains("Reading text");
    h.snapshot("w7_ocr_busy");

    // Extract is disabled: a click must not start another run.
    h.get_by_label("Extract").click();
    h.run_steps(1);
    assert!(h.state().ocr.busy, "still busy after a disabled click");
    assert_eq!(
        h.state().status,
        "busy modal",
        "a disabled Extract must not call run_ocr"
    );

    // The worker's result lands: progress is gone, the text is shown.
    {
        let app = h.state_mut();
        app.ocr.busy = false;
        app.ocr.text = "SnagCraft read this line".to_string();
    }
    h.run_steps(2);
    assert!(!h.state().ocr.busy);
    assert_eq!(
        h.query_all_by_label_contains("Reading text").count(),
        0,
        "the progress row is cleared with the busy flag"
    );
    let _ = h.get_by_label_contains("characters");
}

/// The real worker path: `run_ocr` returns immediately (busy), the text arrives
/// through the poll channel, and the busy flag clears. Skipped without tesseract.
#[test]
fn ocr_runs_off_thread_and_polls_the_result() {
    let mut h = harness();
    h.run_steps(2);
    if !h.state_mut().ocr_ready() {
        return; // no tesseract on this host
    }

    h.state_mut().run_ocr();
    assert!(h.state().ocr.busy, "run_ocr arms the busy state at once");
    assert!(h.state().ocr.open, "the window opens for the progress row");

    // Bounded wait for the worker: the UI keeps stepping (that is what polls).
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().ocr.busy {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !h.state().ocr.busy,
        "the busy flag must clear once the worker answers"
    );
    assert!(
        h.state().status.contains("Found")
            || h.state().status.contains("No text found")
            || h.state().status.contains("OCR failed"),
        "the result or its error must be reported (W21 live wording): {}",
        h.state().status
    );
}
