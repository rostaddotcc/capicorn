//! W32 gates (rostad's report): the text quick bar vanished the moment the
//! font dropdown was clicked — focus left the inline TextEdit, lost_focus
//! committed the caption and tore the bar down. Focus moving INTO the bar
//! (font/size/B/I) must keep the editor alive; a real click-away commits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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

fn open_editor(h: &mut Harness<'static, SnagCraftApp>) {
    h.run_steps(2);
    press(h, egui::Key::T);
    click(h, egui::pos2(540.0, 400.0));
    assert!(h.state().is_editing(), "click opens the inline editor");
    h.event(egui::Event::Text("Lens".into()));
    h.run_steps(1);
    assert_eq!(h.state().text_buffer(), "Lens");
}

/// The quick bar paints below the chrome (y > 150); the Text rail carries a
/// twin font combo up top — pick the bar's copy by position. The combo button
/// has no accesskit label (its family is the current_text_value), so query
/// by role.
fn bar_combo<'a>(h: &'a Harness<'static, SnagCraftApp>) -> egui_kittest::Node<'a> {
    h.get_all_by_role(accesskit::Role::ComboBox)
        .into_iter()
        .find(|n| n.rect().min.y > 150.0)
        .expect("quick bar font combo not found")
}

fn bar_node<'a>(
    h: &'a Harness<'static, SnagCraftApp>,
    label: &'static str,
) -> egui_kittest::Node<'a> {
    h.get_all_by_label(label)
        .into_iter()
        .find(|n| n.rect().min.y > 150.0)
        .unwrap_or_else(|| panic!("quick bar node {label:?} not found"))
}

#[test]
fn font_dropdown_click_keeps_the_editor_and_commits_with_the_chosen_family() {
    let mut h = harness();
    open_editor(&mut h);

    // THE reported bug: clicking the font dropdown closed the whole popup.
    bar_combo(&h).click();
    h.run_steps(2);
    assert!(
        h.state().is_editing(),
        "clicking the font dropdown must keep the editor open"
    );
    assert_eq!(h.state().text_buffer(), "Lens", "nothing committed yet");

    // The dropdown is actually usable: pick a family.
    h.get_by_label("Lato").click();
    h.run_steps(2);
    assert!(h.state().is_editing(), "picking a family keeps editing");
    assert_eq!(h.state().font_family, "Lato");

    // A real click-away still commits — with the chosen family.
    click(&mut h, egui::pos2(520.0, 420.0));
    assert!(!h.state().is_editing(), "click-away commits the caption");
    let objs = h
        .state()
        .sess()
        .doc()
        .map(|d| d.objects.clone())
        .unwrap_or_default();
    match objs.last() {
        Some(AnnotObject::Text { text, family, .. }) => {
            assert_eq!(text, "Lens");
            assert_eq!(family, "Lato", "the picked family reaches the object");
        }
        other => panic!("expected a Text object, got {other:?}"),
    }
}

#[test]
fn bold_toggle_in_the_quick_bar_keeps_the_editor_open() {
    let mut h = harness();
    open_editor(&mut h);
    let bold_before = h.state().bold;
    bar_node(&h, "B").click();
    h.run_steps(2);
    assert!(
        h.state().is_editing(),
        "clicking the bar's B toggle must keep the editor open"
    );
    assert_ne!(h.state().bold, bold_before, "the toggle still works");
    assert_eq!(h.state().text_buffer(), "Lens");
}

#[test]
fn clicking_the_canvas_while_the_bar_had_focus_still_commits() {
    let mut h = harness();
    open_editor(&mut h);
    // Open the font popup, then click the canvas: the first outside click
    // only closes the popup (standard dropdown behavior) — the editor must
    // survive it. The next canvas click commits: the TextEdit is not focused
    // (focus sat on the bar), so lost_focus cannot fire — the canvas click
    // path must commit instead of leaving the editor open forever.
    bar_combo(&h).click();
    h.run_steps(2);
    assert!(h.state().is_editing());
    click(&mut h, egui::pos2(520.0, 420.0));
    assert!(
        h.state().is_editing(),
        "the popup-closing click must not commit the caption"
    );
    click(&mut h, egui::pos2(520.0, 420.0));
    assert!(
        !h.state().is_editing(),
        "click-away commits even when focus sat on the quick bar"
    );
}
