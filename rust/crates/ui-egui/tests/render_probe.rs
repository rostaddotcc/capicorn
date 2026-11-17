//! Focused render probes: text + blur must survive session.render().

use snagcraft_ui_egui::SnagCraftApp;

#[test]
fn seeded_render_contains_text_pixels() {
    let app = SnagCraftApp::seeded();
    let img = app.sess().render().unwrap();
    // Text "SnagCraft W1 (Rust)" at (424,96), size 28, near-black.
    let mut dark = 0;
    for y in 90..140 {
        for x in 420..760 {
            let p = img.get_pixel(x, y).0;
            if p[0] < 90 && p[1] < 90 && p[2] < 90 && p[3] > 200 {
                dark += 1;
            }
        }
    }
    assert!(dark > 100, "expected text glyph pixels, found {dark}");
}

#[test]
fn cli_style_session_without_font_skips_text_safely() {
    let mut s = snagcraft_engine::Session::new();
    s.dispatch("doc.new", serde_json::json!({"w": 100, "h": 50}))
        .unwrap();
    s.dispatch("edit.add", serde_json::json!({"object": {"kind":"Text","id":0,"pos":{"x":5.0,"y":5.0},"text":"x","color":[0,0,0,255],"size":20.0,"bold":false,"italic":false}})).unwrap();
    let img = s.render().unwrap(); // must not panic without a font
    assert_eq!(img.dimensions(), (100, 50));
}
