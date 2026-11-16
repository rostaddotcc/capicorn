//! Parity gates for the W2b shell: shortcuts, pointer creation, crop UX,
//! auto-numbering and the shadow/callout derivations.

use egui_kittest::Harness;
use snagcraft_doc::{AnnotObject, ShapeKind};
use snagcraft_ui_egui::{
    SnagCraftApp, Tool, app_ui, apply_theme, callout_font_size, next_step_number, shadow_offsets,
};

fn harness() -> Harness<'static, SnagCraftApp> {
    Harness::builder()
        .with_size(egui::vec2(1200.0, 900.0))
        .with_accessibility_check(false)
        .with_context_setup(apply_theme)
        .build_ui_state(
            |ui, app: &mut SnagCraftApp| app_ui(ui, app),
            SnagCraftApp::new(),
        )
}

fn press(h: &mut Harness<'static, SnagCraftApp>, key: egui::Key) {
    h.key_press(key);
    h.run_steps(1);
}

fn drag(h: &mut Harness<'static, SnagCraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
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

fn objects(h: &Harness<'static, SnagCraftApp>) -> Vec<AnnotObject> {
    h.state()
        .session
        .doc()
        .map(|d| d.objects.clone())
        .unwrap_or_default()
}

#[test]
fn bare_letters_route_all_thirteen_tools() {
    let mut h = harness();
    h.run_steps(2);
    let cases = [
        (egui::Key::V, Tool::Select),
        (egui::Key::C, Tool::Crop),
        (egui::Key::A, Tool::Arrow),
        (egui::Key::T, Tool::Text),
        (egui::Key::R, Tool::Shape),
        (egui::Key::B, Tool::Blur),
        (egui::Key::G, Tool::Bucket),
        (egui::Key::E, Tool::Brush),
        (egui::Key::L, Tool::Spotlight),
        (egui::Key::H, Tool::Highlighter),
        (egui::Key::K, Tool::Callout),
        (egui::Key::S, Tool::Step),
        (egui::Key::M, Tool::Magnify),
    ];
    for (key, tool) in cases {
        press(&mut h, key);
        assert_eq!(h.state().tool, tool, "{key:?} should route to {tool:?}");
    }
    // Tool::ALL order is the live toolbar order.
    assert_eq!(Tool::ALL.len(), 13);
    assert_eq!(Tool::ALL[0], Tool::Select);
    assert_eq!(Tool::ALL[12], Tool::Magnify);
}

#[test]
fn drag_tools_create_their_objects() {
    let cases = [
        (egui::Key::A, "Arrow"),
        (egui::Key::R, "Shape"),
        (egui::Key::B, "Blur"),
        (egui::Key::L, "Spotlight"),
        (egui::Key::H, "Highlighter"),
        (egui::Key::K, "Callout"),
        (egui::Key::M, "Magnify"),
    ];
    for (key, label) in cases {
        let mut h = harness();
        h.run_steps(2);
        press(&mut h, key);
        drag(&mut h, egui::pos2(480.0, 380.0), egui::pos2(660.0, 470.0));
        let objs = objects(&h);
        assert_eq!(objs.len(), 1, "{label}: expected one object");
        let kind = match &objs[0] {
            AnnotObject::Arrow { .. } => "Arrow",
            AnnotObject::Shape { .. } => "Shape",
            AnnotObject::Blur { .. } => "Blur",
            AnnotObject::Spotlight { .. } => "Spotlight",
            AnnotObject::Highlighter { .. } => "Highlighter",
            AnnotObject::Callout { .. } => "Callout",
            AnnotObject::Magnify { .. } => "Magnify",
            _ => "other",
        };
        assert_eq!(kind, label, "{label} tool created {kind}");
    }
}

#[test]
fn shape_tool_uses_the_rail_variant() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::R);
    h.state_mut().shape_kind = ShapeKind::Star;
    h.run_steps(1);
    drag(&mut h, egui::pos2(480.0, 380.0), egui::pos2(600.0, 470.0));
    match &objects(&h)[0] {
        AnnotObject::Shape { shape, .. } => assert_eq!(*shape, ShapeKind::Star),
        other => panic!("expected star shape, got {other:?}"),
    }
}

#[test]
fn clicking_drops_auto_numbered_steps() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::S);
    click(&mut h, egui::pos2(520.0, 400.0));
    click(&mut h, egui::pos2(620.0, 430.0));
    let numbers: Vec<u32> = objects(&h)
        .iter()
        .filter_map(|o| match o {
            AnnotObject::Step { number, .. } => Some(*number),
            _ => None,
        })
        .collect();
    assert_eq!(numbers, vec![1, 2]);
    assert_eq!(next_step_number(&[]), 1);
}

#[test]
fn brush_drag_adds_a_freehand_stroke() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::E);
    drag(&mut h, egui::pos2(460.0, 380.0), egui::pos2(700.0, 460.0));
    match &objects(&h)[0] {
        AnnotObject::BrushStroke { points, width, .. } => {
            assert!(points.len() >= 2, "freehand stroke needs samples");
            assert_eq!(*width, h.state().brush_width);
        }
        other => panic!("expected brush stroke, got {other:?}"),
    }
}

#[test]
fn bucket_click_floods_the_base() {
    let mut h = harness();
    h.run_steps(2);
    h.state_mut().color = egui::Color32::from_rgb(255, 0, 0);
    press(&mut h, egui::Key::G);
    click(&mut h, egui::pos2(560.0, 420.0));
    let doc = h.state().session.doc().expect("doc");
    let (w, h_) = doc.base.dimensions();
    let mut red = 0;
    for y in 0..h_ {
        for x in 0..w {
            if doc.base.get_pixel(x, y).0 == [255, 0, 0, 255] {
                red += 1;
            }
        }
    }
    assert!(
        red > 10_000,
        "flood fill should repaint the white canvas: {red}"
    );
}

#[test]
fn text_click_types_then_commits_on_enter() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::T);
    h.state_mut().bold = true;
    h.state_mut().font_size = 44.0;
    h.run_steps(1);
    click(&mut h, egui::pos2(540.0, 400.0));
    assert!(
        h.state().is_editing(),
        "click should open the inline editor"
    );
    h.event(egui::Event::Text("Lens".into()));
    h.run_steps(1);
    assert_eq!(h.state().text_buffer(), "Lens");
    press(&mut h, egui::Key::Enter);
    assert!(
        !h.state().is_editing(),
        "Enter commits and closes the editor"
    );
    match &objects(&h)[0] {
        AnnotObject::Text {
            text,
            bold,
            size,
            family,
            ..
        } => {
            assert_eq!(text, "Lens");
            assert!(bold, "rail bold must reach the object");
            assert_eq!(*size, 44.0);
            assert_eq!(family, "Inter");
        }
        other => panic!("expected text object, got {other:?}"),
    }
}

#[test]
fn escape_cancels_the_inline_editor() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::T);
    click(&mut h, egui::pos2(540.0, 400.0));
    h.event(egui::Event::Text("nope".into()));
    h.run_steps(1);
    press(&mut h, egui::Key::Escape);
    assert!(!h.state().is_editing(), "Esc must drop the editor");
    assert!(objects(&h).is_empty(), "cancelled text never lands");
}

#[test]
fn crop_commits_on_enter_and_cancels_on_escape() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::C);
    drag(&mut h, egui::pos2(480.0, 380.0), egui::pos2(640.0, 460.0));
    press(&mut h, egui::Key::Enter);
    let (w, hh) = h
        .state()
        .session
        .doc()
        .map(|d| (d.size.w, d.size.h))
        .expect("doc");
    assert!(
        w < 1000 && hh < 640,
        "crop should shrink the canvas: {w}×{hh}"
    );

    // Escape drops a pending frame; Enter then does nothing.
    press(&mut h, egui::Key::C);
    drag(&mut h, egui::pos2(500.0, 390.0), egui::pos2(600.0, 440.0));
    press(&mut h, egui::Key::Escape);
    press(&mut h, egui::Key::Enter);
    let after = h.state().session.doc().map(|d| (d.size.w, d.size.h));
    assert_eq!(after, Some((w, hh)), "Esc must cancel the crop");
}

#[test]
fn delete_removes_the_selection() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::A);
    drag(&mut h, egui::pos2(480.0, 380.0), egui::pos2(660.0, 470.0));
    assert_eq!(objects(&h).len(), 1);
    // Select the arrow: its bounds straddle the drag path's midpoint.
    press(&mut h, egui::Key::V);
    click(&mut h, egui::pos2(570.0, 425.0));
    press(&mut h, egui::Key::Delete);
    assert!(objects(&h).is_empty(), "Delete must remove the selection");
}

#[test]
fn shadow_ui_derives_offsets_and_alpha() {
    assert_eq!(shadow_offsets(90.0, 4.0), (0.0, 4.0));
    assert_eq!(shadow_offsets(0.0, 10.0), (10.0, 0.0));
    assert_eq!(shadow_offsets(180.0, 10.0), (-10.0, 0.0));
    assert_eq!(shadow_offsets(270.0, 6.0), (0.0, -6.0));

    let mut app = SnagCraftApp::new();
    app.shadow_enabled = true;
    app.shadow_angle = 180.0;
    app.shadow_distance = 10.0;
    app.shadow_opacity = 50.0;
    app.shadow_blur = 7.0;
    let v = app.shadow_value();
    assert_eq!(v["offset_x"], -10.0);
    assert_eq!(v["offset_y"], 0.0);
    assert_eq!(v["blur"], 7);
    assert_eq!(v["color"][3], 128);
    assert_eq!(v["enabled"], true);
}

#[test]
fn callout_caption_size_follows_the_box_width() {
    // tools.ts:1387-1391 — min(rail, width/4), floored at 12.
    assert_eq!(callout_font_size(240.0, 24.0), 24.0);
    assert_eq!(callout_font_size(2000.0, 24.0), 24.0);
    assert_eq!(callout_font_size(80.0, 24.0), 20.0);
    assert_eq!(callout_font_size(20.0, 24.0), 12.0);
    assert_eq!(callout_font_size(2000.0, 500.0), 120.0);
    assert_eq!(callout_font_size(400.0, f32::NAN), 12.0);
}
