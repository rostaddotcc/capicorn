//! W14 interaction gates: select-mode resize handles, double-click inline
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! re-edit of Text/Callout, crop plain-click reset, and the brush nib preview.
//! Pointer paths are exercised for real (kittest drag/click events), because
//! seeded-command snapshots cannot catch pointer-path bugs.

use egui_kittest::Harness;
use snagcraft_doc::AnnotObject;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, Tool, app_ui, apply_theme};

fn harness() -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::new();
    app.settings = SettingsView {
        show_widget: false,
        ..SettingsView::default()
    };
    Harness::builder()
        .with_size(egui::vec2(1200.0, 900.0))
        // 60 fps simulation: the default 250 ms step puts two consecutive
        // clicks outside egui's double-click window.
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
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
        .sess()
        .doc()
        .map(|d| d.objects.clone())
        .unwrap_or_default()
}

/// Calibrate the image→screen mapping UNDER SELECT (the properties rail
/// height differs per tool, so the canvas origin shifts with the active tool).
/// A move only yields scale (translation-invariant); the absolute offset comes
/// from a resize: drag the BR corner to a known screen point and read back the
/// new image corner. Returns (scale, screen offset).
fn calibrate(h: &mut Harness<'static, SnagCraftApp>) -> (f32, egui::Vec2) {
    press(h, egui::Key::R);
    drag(h, egui::pos2(480.0, 380.0), egui::pos2(660.0, 470.0));
    let rect = objects(h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Shape { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("calibration shape");
    let scale = 180.0 / rect.w;
    // Creation maps the press point directly: offset under the Shape rail.
    let off_r = egui::vec2(480.0 - rect.x * scale, 380.0 - rect.y * scale);

    press(h, egui::Key::V);
    let centre = egui::pos2(
        off_r.x + (rect.x + rect.w / 2.0) * scale,
        off_r.y + (rect.y + rect.h / 2.0) * scale,
    );
    click(h, centre);
    assert!(
        h.state().selected.is_some(),
        "calibration must select the shape"
    );
    // Approximate BR (the rail shift is a few px; the handle tolerance is 10).
    let br = egui::pos2(
        off_r.x + (rect.x + rect.w) * scale,
        off_r.y + (rect.y + rect.h) * scale,
    );
    let to = br + egui::vec2(60.0, 40.0);
    drag(h, br, to);
    let r2 = objects(h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Shape { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("resized shape");
    assert!(
        r2.w > rect.w + 1.0,
        "calibration drag must hit the corner handle"
    );
    let offset = egui::vec2(to.x - (r2.x + r2.w) * scale, to.y - (r2.y + r2.h) * scale);
    (scale, offset)
}

fn to_screen(scale: f32, offset: egui::Vec2, p: snagcraft_geom::Point) -> egui::Pos2 {
    egui::pos2(offset.x + p.x * scale, offset.y + p.y * scale)
}

#[test]
fn select_mode_corner_handle_resizes_the_object() {
    let mut h = harness();
    h.run_steps(2);
    let (scale, offset) = calibrate(&mut h);
    let before = objects(&h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Shape { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("shape");

    // Select the shape by clicking its centre, then drag the bottom-right
    // corner handle 60×40 screen px outward.
    press(&mut h, egui::Key::V);
    let centre = to_screen(
        scale,
        offset,
        snagcraft_geom::Point::new(before.x + before.w / 2.0, before.y + before.h / 2.0),
    );
    click(&mut h, centre);
    assert!(h.state().selected.is_some(), "shape must be selected");
    let br = to_screen(
        scale,
        offset,
        snagcraft_geom::Point::new(before.x + before.w, before.y + before.h),
    );
    drag(&mut h, br, br + egui::vec2(60.0, 40.0));

    let after = objects(&h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Shape { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("shape still there");
    assert!(
        (after.w - before.w - 60.0 / scale).abs() < 2.0 / scale,
        "width must grow by the drag: {before:?} -> {after:?}"
    );
    assert!(
        (after.h - before.h - 40.0 / scale).abs() < 2.0 / scale,
        "height must grow by the drag: {before:?} -> {after:?}"
    );
    // The anchored corner stays put.
    assert!((after.x - before.x).abs() < 0.5 && (after.y - before.y).abs() < 0.5);

    // Undo restores the original rect (resize is a history step).
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Z);
    h.run_steps(1);
    let undone = objects(&h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Shape { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("shape");
    assert!(
        (undone.w - before.w).abs() < 0.5,
        "undo must restore the width"
    );
}

#[test]
fn double_click_reedits_text_in_place_and_empty_removes_it() {
    let mut h = harness();
    h.run_steps(2);
    let (scale, offset) = calibrate(&mut h);

    // Place a caption with the Text tool.
    press(&mut h, egui::Key::T);
    let text_at = egui::pos2(500.0, 400.0);
    click(&mut h, text_at);
    assert!(
        h.state().is_editing(),
        "text tool must open the inline editor"
    );
    h.event(egui::Event::Text("hello".into()));
    h.run_steps(1);
    press(&mut h, egui::Key::Enter);
    assert!(!h.state().is_editing(), "Enter commits the caption");
    let img_pos = objects(&h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Text { pos, text, .. } if text == "hello" => Some(*pos),
            _ => None,
        })
        .expect("committed text object");

    // Double-click it in Select mode: the inline editor reopens WITH the text.
    press(&mut h, egui::Key::V);
    let sp = to_screen(scale, offset, img_pos) + egui::vec2(6.0, 6.0);
    h.run_steps(30); // let the placement click expire out of the double-click window
    click(&mut h, sp);
    click(&mut h, sp);
    assert!(
        h.state().is_editing(),
        "double-click must reopen the editor"
    );
    assert_eq!(
        h.state().text_buffer(),
        "hello",
        "existing text must be loaded"
    );

    // Append and commit: the SAME object is updated, not a new one added.
    let count_before = objects(&h).len();
    h.event(egui::Event::Text("!!".into()));
    h.run_steps(1);
    press(&mut h, egui::Key::Enter);
    let texts: Vec<String> = objects(&h)
        .iter()
        .filter_map(|o| match o {
            AnnotObject::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        objects(&h).len(),
        count_before,
        "re-edit must not add an object"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("hello") && t.contains("!!")),
        "the object's text must be updated: {texts:?}"
    );

    // Re-edit again, empty the box, commit: the object is removed (live
    // settle parity — an empty caption is not content).
    let pos2 = objects(&h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Text { pos, .. } => Some(*pos),
            _ => None,
        })
        .expect("text object");
    let sp2 = to_screen(scale, offset, pos2) + egui::vec2(6.0, 6.0);
    h.run_steps(30); // fresh double-click window (no triple-click counting)
    click(&mut h, sp2);
    click(&mut h, sp2);
    assert!(h.state().is_editing());
    // Empty the box one character at a time (deterministic caret-at-end).
    for _ in 0..8 {
        press(&mut h, egui::Key::Backspace);
    }
    assert_eq!(h.state().text_buffer(), "", "box must be empty");
    press(&mut h, egui::Key::Enter);
    assert!(
        !objects(&h)
            .iter()
            .any(|o| matches!(o, AnnotObject::Text { .. })),
        "emptying the re-edit must remove the object"
    );
}

#[test]
fn double_click_reedits_a_callout() {
    let mut h = harness();
    h.run_steps(2);
    let (scale, offset) = calibrate(&mut h);

    press(&mut h, egui::Key::K);
    drag(&mut h, egui::pos2(500.0, 500.0), egui::pos2(700.0, 580.0));
    let (rect, text) = objects(&h)
        .iter()
        .find_map(|o| match o {
            AnnotObject::Callout { rect, text, .. } => Some((*rect, text.clone())),
            _ => None,
        })
        .expect("callout");

    press(&mut h, egui::Key::V);
    let centre = to_screen(
        scale,
        offset,
        snagcraft_geom::Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
    );
    click(&mut h, centre);
    click(&mut h, centre);
    assert!(
        h.state().is_editing(),
        "double-click must reopen the callout text"
    );
    assert_eq!(h.state().text_buffer(), text);
}

#[test]
fn crop_plain_click_resets_and_enter_needs_eight_pixels() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::C);

    // A 3px drag is below the live MIN_DRAG=8: the frame resets on release.
    drag(&mut h, egui::pos2(500.0, 390.0), egui::pos2(503.0, 392.0));
    assert!(h.state().crop.is_none(), "tiny drag must reset the crop");

    // A real drag leaves a pending frame; Enter commits it.
    drag(&mut h, egui::pos2(480.0, 380.0), egui::pos2(640.0, 460.0));
    assert!(h.state().crop.is_some(), "real drag must leave a frame");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().crop.is_none(), "Enter commits the frame");

    // A degenerate 5px frame cannot commit (live MIN_SIZE=8) and is dropped.
    press(&mut h, egui::Key::C);
    h.state_mut().crop = Some(snagcraft_geom::Rect::new(10.0, 10.0, 5.0, 5.0));
    h.run_steps(1);
    press(&mut h, egui::Key::Enter);
    assert!(h.state().crop.is_none(), "sub-8px frame must not linger");
}

#[test]
fn brush_rail_shows_the_nib_preview() {
    let mut h = harness();
    h.run_steps(2);
    press(&mut h, egui::Key::E);
    assert_eq!(h.state().tool, Tool::Brush);
    h.state_mut().brush_width = 20.0;
    h.run_steps(3);
    h.snapshot("w14_brush_nib");
}
