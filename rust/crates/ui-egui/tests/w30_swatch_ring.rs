//! W30 gate: the palette's extreme swatches must stay visible — the black
//! swatch on the dark rail and the white swatch on the light rail both had
//! ~1.5:1 hairline rings (invisible). Pixel-probe the ring around the black
//! swatch in BOTH themes and require ~3:1 against the rail surface.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use egui_kittest::Harness;
use snagcraft_ui_egui::{SettingsView, SnagCraftApp, Theme, app_ui, apply_theme};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "snagcraft-w30-swatch-{tag}-{}-{:?}",
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

fn lum(c: [u8; 3]) -> f64 {
    let ch = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
}

fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (lum(a), lum(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Find the black swatch (PALETTE "Black" = 1c1c1e): a ~20px-wide solid block
/// of EXACTLY that fill in the chrome rows (y < 120), present on ≥10
/// consecutive rows. The tight match matters in dark mode: the rail panel
/// (30,33,38) is only a few units away and would merge with a loose threshold.
fn find_black_swatch(img: &image::RgbaImage) -> Option<(u32, u32)> {
    let (w, h) = img.dimensions();
    let dark = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        p[0].abs_diff(28) <= 2 && p[1].abs_diff(28) <= 2 && p[2].abs_diff(30) <= 2
    };
    for y in 30..h.min(120) {
        let mut x = 0;
        while x < w {
            if dark(x, y) {
                let start = x;
                while x < w && dark(x, y) {
                    x += 1;
                }
                let len = x - start;
                if (16..=26).contains(&len) {
                    let mut rows = 0;
                    for yy in y..h.min(120) {
                        if (start..start + len).all(|xx| dark(xx, yy)) {
                            rows += 1;
                        } else {
                            break;
                        }
                    }
                    if rows >= 10 {
                        // Re-measure at the middle row: the top rows sit in
                        // the rounded corner, where the fill edge is inset.
                        let ymid = y + rows / 2;
                        let cx = start + len / 2;
                        let mut l = cx;
                        while l > 0 && dark(l - 1, ymid) {
                            l -= 1;
                        }
                        return Some((l, ymid));
                    }
                }
            } else {
                x += 1;
            }
        }
    }
    None
}

fn probe(theme: Theme, rail: [u8; 3], tag: &str) {
    let dir = scratch(tag);
    let mut h = harness(&dir, theme);
    h.run_steps(2);
    let img = h.render().expect("render");
    let (x0, ymid) = find_black_swatch(&img)
        .unwrap_or_else(|| panic!("black swatch not found in the {tag} rail"));
    // The 1px ring sits immediately outside the fill (StrokeKind::Outside).
    let ring = img.get_pixel(x0 - 1, ymid).0;
    let ratio = contrast([ring[0], ring[1], ring[2]], rail);
    assert!(
        ratio >= 2.8,
        "{tag}: black-swatch ring {ring:?} vs rail {rail:?} = {ratio:.2}:1 (need ~3:1)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn black_swatch_ring_reads_on_the_dark_rail() {
    probe(Theme::Dark, [35, 38, 44], "dark");
}

#[test]
fn swatch_rings_read_on_the_light_rail() {
    probe(Theme::Light, [255, 255, 255], "light");
}
