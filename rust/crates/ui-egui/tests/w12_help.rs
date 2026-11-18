// Tests are exempt from the no-unwrap/no-expect house rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! W12 parity long-tail: the keyboard-shortcuts sheet (`?`), the About modal
//! (File → About SnagCraft…), and the two editor residuals — a text caption
//! that commits once on settle instead of per keystroke, and a canvas zoom
//! that is a view, not a document edit.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use snagcraft_ui_egui::{APP_VERSION, SnagCraftApp, Theme, app_ui, apply_theme};

fn harness() -> Harness<'static, SnagCraftApp> {
    let mut app = SnagCraftApp::seeded();
    app.settings.show_widget = false;
    app.capture_enabled = false;
    Harness::builder()
        .with_size(egui::vec2(1100.0, 820.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Light))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app)
}

fn click(h: &mut Harness<'static, SnagCraftApp>, pos: egui::Pos2) {
    h.hover_at(pos);
    h.run_steps(1);
    h.drag_at(pos);
    h.run_steps(1);
    h.drop_at(pos);
    h.run_steps(2);
}

fn objects(h: &Harness<'static, SnagCraftApp>) -> usize {
    h.state().sess().doc().map_or(0, |d| d.objects.len())
}

/// `?` opens the sheet, it lists the bindings that really exist, Esc closes it.
#[test]
fn question_mark_opens_the_shortcuts_sheet_and_esc_closes() {
    let mut h = harness();
    h.run_steps(2);
    assert!(!h.state().shortcuts_open);

    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Questionmark);
    h.run_steps(2);
    assert!(h.state().shortcuts_open, "`?` must open the sheet");

    // The sheet states what the key handler consumes — no invented rows.
    h.get_by_label("Keyboard shortcuts");
    h.get_by_label("Ctrl + scroll");
    h.get_by_label("Zoom the canvas");
    h.get_by_label("Ctrl + O");
    h.get_by_label("Open an image");
    h.get_by_label("Ctrl + Shift + S");
    h.get_by_label("Delete / Backspace");
    h.get_by_label("?");

    h.snapshot("w12_shortcuts");

    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().shortcuts_open, "Esc must close the sheet");
}

/// File → About SnagCraft… opens the modal, the version is on it, Esc closes.
#[test]
fn about_modal_shows_name_version_and_repo() {
    let mut h = harness();
    h.run_steps(2);

    h.get_by_label("File").click();
    h.run_steps(3);
    h.get_by_label_contains("About SnagCraft").click();
    h.run_steps(2);
    assert!(h.state().about_open, "the menu row opens the About modal");

    h.get_by_label("SnagCraft");
    h.get_by_label(&format!("version {APP_VERSION}"));
    h.get_by_label_contains("https://github.com/rostaddotcc/snagcraft");
    // W19: the live license line (the W12 tick overclaimed it).
    h.get_by_label_contains("GPL-3.0-or-later");

    h.snapshot("w12_about");

    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().about_open, "Esc must close About");
}

/// A caption is committed once, when the box settles — typing five characters
/// leaves the document and its history untouched, Enter adds exactly one step.
#[test]
fn text_caption_commits_on_settle_not_per_keystroke() {
    let mut h = harness();
    h.run_steps(2);

    let before_objects = objects(&h);
    let before_depth = h.state().sess().history_depth();

    h.key_press(egui::Key::T);
    h.run_steps(1);
    click(&mut h, egui::pos2(560.0, 420.0));
    h.run_steps(2);
    assert!(h.state().is_editing(), "click opens the inline editor");

    h.event(egui::Event::Text("Abcd".into()));
    h.run_steps(2);
    h.event(egui::Event::Text("e".into()));
    h.run_steps(2);
    assert_eq!(h.state().text_buffer(), "Abcde");
    assert_eq!(
        objects(&h),
        before_objects,
        "typing must not create objects"
    );
    assert_eq!(
        h.state().sess().history_depth(),
        before_depth,
        "typing must not record history steps"
    );

    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(!h.state().is_editing(), "Enter settles the editor");
    assert_eq!(objects(&h), before_objects + 1, "one caption, one object");
    assert_eq!(
        h.state().sess().history_depth(),
        before_depth + 1,
        "one caption, one history step"
    );
}

/// Ctrl+scroll zoom is a view multiplier: clamped, never a document mutation.
#[test]
fn zoom_is_a_clamped_view_not_an_edit() {
    let mut h = harness();
    h.run_steps(2);
    let depth = h.state().sess().history_depth();
    assert_eq!(h.state().view_zoom, 1.0);

    h.state_mut().zoom_view(2.0);
    assert_eq!(h.state().view_zoom, 2.0);
    h.state_mut().zoom_view(100.0);
    assert_eq!(h.state().view_zoom, 4.0, "zoom is clamped at 400 %");
    h.state_mut().zoom_view(0.0001);
    assert_eq!(h.state().view_zoom, 0.25, "zoom is clamped at 25 %");
    h.state_mut().zoom_reset();
    assert_eq!(h.state().view_zoom, 1.0);

    assert_eq!(
        h.state().sess().history_depth(),
        depth,
        "zooming is not an edit"
    );
}
