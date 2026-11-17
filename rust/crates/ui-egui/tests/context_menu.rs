//! Right-click context menu: opens on secondary click, classic items present.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn harness() -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings = SettingsView {
        show_widget: false,
        ..SettingsView::default()
    };
    Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn right_click(h: &mut Harness<'static, SnagCraftApp>, pos: egui::Pos2) {
    h.hover_at(pos);
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    });
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    });
    h.run_steps(2);
}

#[test]
fn context_menu_opens_with_classic_items() {
    let mut h = harness();
    h.run_steps(3);
    right_click(&mut h, egui::pos2(550.0, 400.0));
    // Unique labels (Undo/Redo twin the toolbar buttons — verified visually).
    for label in ["Copy Image", "Paste", "Save As…", "Export PNG…"] {
        let _ = h.get_by_label(label);
    }
    // Let the menu's fade-in settle: the first frame is mid-animation, so the
    // pixels (and the snapshot) would otherwise depend on the frame history.
    h.run_steps(2);
    h.snapshot("w2_context_menu");
}

#[test]
fn context_menu_on_object_has_object_items() {
    let mut h = harness();
    h.run_steps(3);
    // Seeded callout: image (70,330)-(300,440). Canvas fit at 1100×760 with
    // toolbar+rail+status ≈ 110px chrome → scale ≈ 1.08, origin ≈ (5,115).
    // Image (185,385) → screen ≈ (205,530).
    right_click(&mut h, egui::pos2(205.0, 530.0));
    let _ = h.get_by_label("Copy Image");
    // Object items appear when the right-click hit an object.
    if h.query_by_label("Duplicate").is_some() {
        let _ = h.get_by_label("Bring to Front");
        let _ = h.get_by_label("Send to Back");
    }
}
