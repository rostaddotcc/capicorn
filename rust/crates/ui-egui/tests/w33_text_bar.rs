//! W33 gates: the text quick bar reaches live TextBar parity — it shows for
//! the EDITED or SELECTED caption, carries colour, and restyles the object
//! live (edit.restyle). New-caption placement keeps the W32 behavior.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use snagcraft_doc::AnnotObject;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn harness() -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::new();
    app.push_settings(SettingsView {
        show_widget: false,
        ..SettingsView::default()
    });
    Harness::builder()
        .with_size(egui::vec2(1200.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        // egui's own colour picker carries a 📋 copy button the bundled
        // fonts lack; the app default is Tofu (kittest sets Panic).
        .allow_missing_glyphs()
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn press(h: &mut Harness<'static, SnagCraftApp>, key: egui::Key) {
    h.key_press(key);
    h.run_steps(1);
}

fn click(h: &mut Harness<'static, SnagCraftApp>, pos: egui::Pos2) {
    h.hover_at(pos);
    h.run_steps(1);
    h.drag_at(pos);
    h.run_steps(1);
    h.drop_at(pos);
    h.run_steps(2);
}

fn add_caption(h: &mut Harness<'static, SnagCraftApp>) {
    h.state()
        .sess()
        .dispatch(
            "edit.add",
            json!({"object": {"kind": "Text", "id": 0,
                "pos": {"x": 300.0, "y": 300.0}, "text": "caption",
                "color": [0, 0, 0, 255], "size": 24.0,
                "bold": false, "italic": false, "family": "Inter"}}),
        )
        .unwrap();
}

fn text_obj(h: &Harness<'static, SnagCraftApp>) -> AnnotObject {
    h.state()
        .sess()
        .doc()
        .and_then(|d| {
            d.objects
                .iter()
                .find(|o| matches!(o, AnnotObject::Text { .. }))
                .cloned()
        })
        .expect("a Text object")
}

/// The bar paints below the chrome; the rail may carry twin widgets up top.
fn bar_node_by_role<'a>(
    h: &'a Harness<'static, SnagCraftApp>,
    role: accesskit::Role,
) -> egui_kittest::Node<'a> {
    h.get_all_by_role(role)
        .into_iter()
        .find(|n| n.rect().min.y > 150.0)
        .expect("quick bar widget not found")
}

#[test]
fn selected_caption_shows_the_bar_and_restyles_live() {
    let mut h = harness();
    h.run_steps(2);
    add_caption(&mut h);
    h.state_mut().selected = Some(1);
    h.run_steps(2);

    // The bar is up and its controls mirror the object's style.
    let _ = bar_node_by_role(&h, accesskit::Role::ComboBox);
    assert_eq!(h.state().font_family, "Inter");
    assert_eq!(h.state().font_size, 24.0);

    // Size stepper (+2 per click, live TextBar step) restyles the object.
    h.get_by_label("+").click();
    h.run_steps(2);
    match text_obj(&h) {
        AnnotObject::Text { size, .. } => assert_eq!(size, 26.0),
        other => panic!("expected Text, got {other:?}"),
    }

    // Bold toggle restyles the object.
    h.get_by_label("B").click();
    h.run_steps(2);
    match text_obj(&h) {
        AnnotObject::Text { bold, .. } => assert!(bold),
        other => panic!("expected Text, got {other:?}"),
    }

    // Font pick restyles the object (no editing involved).
    bar_node_by_role(&h, accesskit::Role::ComboBox).click();
    h.run_steps(2);
    h.get_by_label("Lato").click();
    h.run_steps(2);
    match text_obj(&h) {
        AnnotObject::Text { family, .. } => assert_eq!(family, "Lato"),
        other => panic!("expected Text, got {other:?}"),
    }
    assert!(!h.state().is_editing(), "selection styling is not editing");
    h.snapshot("w33_selection_bar");
}

#[test]
fn color_restyle_waits_for_the_picker_to_close() {
    let mut h = harness();
    h.run_steps(2);
    add_caption(&mut h);
    h.state_mut().selected = Some(1);
    h.run_steps(2);

    // Open the bar's colour picker (the rail is Select's — no twin well).
    bar_node_by_role(&h, accesskit::Role::ColorWell).click();
    h.run_steps(2);

    // Simulate the picker's output while it is open: NO restyle yet (the
    // live picker settles on close; per-frame dispatch would flood undo).
    h.state_mut().color = egui::Color32::from_rgb(255, 0, 0);
    h.run_steps(3);
    match text_obj(&h) {
        AnnotObject::Text { color, .. } => assert_eq!(color.r, 0, "no dispatch while open"),
        other => panic!("expected Text, got {other:?}"),
    }

    // Close the picker — the colour lands on the object.
    press(&mut h, egui::Key::Escape);
    h.run_steps(2);
    match text_obj(&h) {
        AnnotObject::Text { color, .. } => assert_eq!(color.r, 255, "dispatch on close"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn reediting_shows_the_bar_and_restyles_before_commit() {
    let mut h = harness();
    h.run_steps(2);
    // Place a caption through the real Text-tool flow.
    press(&mut h, egui::Key::T);
    click(&mut h, egui::pos2(500.0, 400.0));
    assert!(h.state().is_editing());
    h.event(egui::Event::Text("hello".into()));
    h.run_steps(1);
    press(&mut h, egui::Key::Enter);
    assert!(!h.state().is_editing());

    // Double-click it in Select mode -> re-edit WITH the bar (live parity).
    press(&mut h, egui::Key::V);
    click(&mut h, egui::pos2(500.0, 400.0));
    click(&mut h, egui::pos2(500.0, 400.0));
    assert!(h.state().is_editing(), "double-click re-edits the caption");
    let _ = bar_node_by_role(&h, accesskit::Role::ComboBox);

    // Bold in the bar restyles the object being re-edited, immediately.
    h.get_by_label("B").click();
    h.run_steps(2);
    match text_obj(&h) {
        AnnotObject::Text { bold, .. } => assert!(bold, "restyle lands during re-edit"),
        other => panic!("expected Text, got {other:?}"),
    }

    // Enter commits the text; the style stays.
    press(&mut h, egui::Key::Enter);
    assert!(!h.state().is_editing());
    match text_obj(&h) {
        AnnotObject::Text { text, bold, .. } => {
            assert_eq!(text, "hello");
            assert!(bold);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}
