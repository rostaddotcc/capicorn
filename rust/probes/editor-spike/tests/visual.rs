//! Headless visual gate: render the real editor UI offscreen via egui_kittest
//! (wgpu software/hardware adapter, no window needed) and snapshot it.
//! Bless once with UPDATE_SNAPSHOTS=true, then vision-inspect the PNG.

use editor_spike::app::{SpikeApp, apply_theme, editor_ui};
use egui_kittest::Harness;

#[test]
fn editor_spike_visual() {
    let app = SpikeApp::seeded();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_accessibility_check(false) // custom-painted icon buttons carry tooltips, not accesskit labels
        .with_context_setup(apply_theme)
        .build_ui_state(|ui, app: &mut SpikeApp| editor_ui(ui, app), app);
    harness.run_steps(5);
    harness.snapshot("editor_spike");
}
