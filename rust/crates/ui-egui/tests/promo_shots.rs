//! Promo shot renderer for the SnagCraft launch film (snagcraft-promo).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Renders REAL app UI states through the production egui shell — one PNG per
//! story beat, plus the engine's own flat export for the payoff shot. Ignored
//! by default; run explicitly:
//!
//! ```text
//! PROMO_BASE=<readme-hero.png> PROMO_OUT=<assets-dir> \
//!   cargo test -p snagcraft-ui-egui --test promo_shots -- --ignored --nocapture
//! ```
//!
//! The base is a real capture (the project's GitHub README hero); every
//! annotation is added through the real engine commands, so each frame is the
//! product performing, never a mockup. Coordinates were measured against a
//! 100px grid overlay on the 1050x612 base (see the film's SPEC.md).

use egui_kittest::Harness;
use serde_json::json;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, Tool, app_ui, apply_theme};

fn out_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("PROMO_OUT").expect("PROMO_OUT must be set"))
}

fn base() -> image::RgbaImage {
    let path = std::env::var("PROMO_BASE").expect("PROMO_BASE must be set");
    image::open(&path)
        .unwrap_or_else(|e| panic!("cannot open base {path}: {e}"))
        .to_rgba8()
}

fn shadow() -> serde_json::Value {
    json!({"enabled": true, "color": [0, 0, 0, 115], "blur": 6,
        "offset_x": 3.0, "offset_y": 4.0})
}

fn add(app: &mut SnagCraftApp, obj: serde_json::Value) {
    app.sess()
        .dispatch("edit.add", json!({"object": obj}))
        .unwrap_or_else(|e| panic!("edit.add failed: {e}"));
}

/// Fresh app with the real capture loaded, widget off.
fn app_with_base() -> SnagCraftApp {
    let mut app = SnagCraftApp::new();
    app.sess().load_surface(base());
    app.settings = SettingsView {
        show_widget: false,
        ..SettingsView::default()
    };
    app.status = "Opened readme-hero.png".to_string();
    app
}

/// Cumulative annotation scene, measured against the base grid:
/// heading (155,62,345,92) · bold tagline (155,125,595,143) · workflow words
/// capture(787-843,190-208) annotate(873-943,190-208) share(155-197,215-233) ·
/// status blockquote (175,278,957,365) · Why paragraph (155,455,957,515).
/// `level` 1..=5 adds the beats in story order; the spotlight goes in first so
/// every later object stays bright above its dim.
fn scene(app: &mut SnagCraftApp, level: u8) {
    if level >= 2 {
        // Spotlight the hero: heading + tagline stay lit, the rest dims.
        add(
            app,
            json!({"kind": "Spotlight", "id": 0,
                "rect": {"x": 128.0, "y": 48.0, "w": 880.0, "h": 130.0}, "dim": 0.35}),
        );
        // Blur the noise: the competitor names in the Why paragraph. The
        // rect fully covers the callout that lands on it (no letter bleed).
        app.sess()
            .dispatch(
                "edit.blur",
                json!({"rect": {"x": 140.0, "y": 430.0, "w": 870.0, "h": 100.0},
                    "mode": "gaussian", "radius": 12}),
            )
            .expect("edit.blur");
    }
    if level >= 1 {
        // Horizontal arrow through the empty band right of the heading —
        // no text crossed, head lands on the heading's right edge.
        add(
            app,
            json!({"kind": "Arrow", "id": 0,
                "from": {"x": 900.0, "y": 84.0}, "to": {"x": 352.0, "y": 84.0},
                "color": [255, 59, 48, 255], "width": 6.0, "shadow": shadow()}),
        );
    }
    if level >= 3 {
        // Callout sits ON the blurred noise — the claim over the clutter.
        add(
            app,
            json!({"kind": "Callout", "id": 0,
                "rect": {"x": 560.0, "y": 440.0, "w": 420.0, "h": 90.0},
                "text": "One lightweight app. The whole flow.",
                "color": [28, 32, 40, 255], "size": 26.0, "family": "Inter",
                "shadow": {"enabled": false}}),
        );
        // Quiet honest note: the README says Tauri; the Rust edition is egui.
        add(
            app,
            json!({"kind": "Text", "id": 0,
                "pos": {"x": 700.0, "y": 555.0}, "text": "Rust + egui",
                "color": [90, 98, 110, 255], "size": 22.0, "bold": false,
                "italic": true, "family": "Inter"}),
        );
    }
    if level >= 4 {
        // Step badges beside/above the three workflow words, never on letters.
        for (n, x, y) in [(1, 815.0, 176.0), (2, 908.0, 176.0), (3, 140.0, 224.0)] {
            add(
                app,
                json!({"kind": "Step", "id": 0, "pos": {"x": x, "y": y},
                    "number": n, "color": [0, 122, 255, 255], "radius": 12.0,
                    "shadow": shadow()}),
            );
        }
        // Highlighter on the bold promise line.
        add(
            app,
            json!({"kind": "Highlighter", "id": 0,
                "rect": {"x": 152.0, "y": 122.0, "w": 448.0, "h": 26.0},
                "color": [255, 204, 0, 255]}),
        );
    }
    if level >= 5 {
        // Magnify the honest detail: the loupe samples a zoom-sized window
        // around its centre — wide and short so the whole 'Status: alpha,
        // Linux-first.' line fits at 2x without the next line leaking in.
        add(
            app,
            json!({"kind": "Magnify", "id": 0,
                "rect": {"x": 30.0, "y": 262.0, "w": 480.0, "h": 50.0}, "zoom": 2.0}),
        );
        // Star in the clear right margin, below the tagline's first line.
        add(
            app,
            json!({"kind": "Shape", "id": 0, "shape": "Star",
                "rect": {"x": 940.0, "y": 130.0, "w": 60.0, "h": 56.0},
                "color": [255, 204, 0, 255], "width": 3.0, "shadow": shadow()}),
        );
        // Purple brush underline beneath the heading.
        add(
            app,
            json!({"kind": "BrushStroke", "id": 0,
                "points": [{"x": 155.0, "y": 100.0}, {"x": 220.0, "y": 104.0},
                    {"x": 285.0, "y": 98.0}, {"x": 345.0, "y": 102.0}],
                "color": [139, 124, 246, 255], "width": 7.0}),
        );
    }
}

fn render(app: SnagCraftApp, name: &str) {
    let mut h = Harness::builder()
        .with_size(egui::vec2(2800.0, 1800.0))
        .with_accessibility_check(false)
        .with_context_setup(|ctx| apply_theme(ctx, Theme::Dark))
        .build_ui_state(|ui, app: &mut SnagCraftApp| app_ui(ui, app), app);
    h.run_steps(5);
    let img = h.render().expect("offscreen render");
    let path = out_dir().join(name);
    img.save(&path)
        .unwrap_or_else(|e| panic!("save {path:?}: {e}"));
    println!("wrote {}", path.display());
}

/// Beat 1 — the workspace established: real capture, no annotations yet.
#[test]
#[ignore]
fn promo_shot_01_clean() {
    let mut app = app_with_base();
    app.tool = Tool::Select;
    render(app, "shot-01-editor-clean.png");
}

/// Beat 2 — the arrow draws itself to the heading.
#[test]
#[ignore]
fn promo_shot_02_arrow() {
    let mut app = app_with_base();
    scene(&mut app, 1);
    app.tool = Tool::Arrow;
    render(app, "shot-02-arrow.png");
}

/// Beat 3 — blur the noise, spotlight the hero.
#[test]
#[ignore]
fn promo_shot_03_blur_spotlight() {
    let mut app = app_with_base();
    scene(&mut app, 2);
    app.tool = Tool::Spotlight;
    render(app, "shot-03-blur-spotlight.png");
}

/// Beat 4 — the callout lands on the blurred noise.
#[test]
#[ignore]
fn promo_shot_04_callout() {
    let mut app = app_with_base();
    scene(&mut app, 3);
    app.tool = Tool::Callout;
    render(app, "shot-04-callout.png");
}

/// Beat 5 — numbered steps on the workflow words, highlighter on the promise.
#[test]
#[ignore]
fn promo_shot_05_steps() {
    let mut app = app_with_base();
    scene(&mut app, 4);
    app.tool = Tool::Step;
    render(app, "shot-05-steps.png");
}

/// Beat 6 — the full annotated hero. Also writes the engine's REAL flat
/// export (the payoff shot's content).
#[test]
#[ignore]
fn promo_shot_06_full_and_export() {
    let mut app = app_with_base();
    scene(&mut app, 5);
    app.tool = Tool::Magnify;
    app.shadow_enabled = true;

    let export = out_dir().join("export-result.png");
    app.sess()
        .dispatch(
            "doc.export",
            json!({"path": export.to_string_lossy(), "format": "png"}),
        )
        .expect("doc.export");
    println!("wrote {}", export.display());

    render(app, "shot-06-full.png");
}
