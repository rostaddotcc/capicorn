//! W36 gate: the no-document canvas shows the live empty-state card — icon
//! mark, "No capture yet", the PrintScreen guidance with key chips and the
//! auto-save note — centered, in both themes (live style.css:652-730).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w36-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn harness(dir: &std::path::Path, theme: Theme) -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.push_settings(SettingsView {
        show_widget: false,
        save_dir: Some(dir.to_string_lossy().to_string()),
        ..SettingsView::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(move |ctx| apply_theme(ctx, theme))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn empty_card(theme: Theme, tag: &str, snapshot: &str) {
    let dir = scratch(tag);
    let mut h = harness(&dir, theme);
    h.run_steps(2);
    // new()/seeded() always create a default document — close it to reach
    // the no-doc state the empty card lives in.
    h.state().sess().dispatch("doc.close", json!({})).unwrap();
    h.run_steps(2);
    let _ = h.get_by_label("No capture yet");
    let _ = h.get_by_label_contains("PrintScreen");
    let _ = h.get_by_label_contains("saved to your save folder");
    h.snapshot(snapshot);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_state_card_light() {
    empty_card(Theme::Light, "light", "w36_empty_state");
}

#[test]
fn empty_state_card_dark() {
    empty_card(Theme::Dark, "dark", "w36_empty_state_dark");
}
