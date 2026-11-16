//! W0 viewport probe: transparent frameless egui window (widget-shaped pill).
//! Checks transparency, drag (StartDrag), stacking behaviour on the target
//! compositor. On KDE Wayland: the desktop must show through the corners,
//! the pill must drag from anywhere, Esc quits.
//!
//! Usage: viewport-probe [--selftest --shot-out out.png]

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

const RED: Color32 = Color32::from_rgb(214, 40, 40);

fn pill_color() -> Color32 {
    Color32::from_rgba_unmultiplied(28, 30, 36, 235)
}

struct Probe {
    selftest: Option<(PathBuf, u32, bool)>,
    dragging_hint: bool,
    info: String,
}

impl Probe {
    fn new() -> Self {
        let wayland = std::env::var("WAYLAND_DISPLAY").ok();
        let x11 = std::env::var("DISPLAY").ok();
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "?".into());
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "?".into());
        let info = format!(
            "session={session} desktop={desktop} wayland={} x11={}",
            wayland.as_deref().unwrap_or("-"),
            x11.as_deref().unwrap_or("-"),
        );
        Self {
            selftest: None,
            dragging_hint: false,
            info,
        }
    }
}

impl eframe::App for Probe {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Fully transparent root: only the pill is painted.
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let avail = ui.available_rect_before_wrap();
                let pill = Rect::from_min_size(
                    pos2(avail.left() + 4.0, avail.center().y - 28.0),
                    vec2(avail.width() - 8.0, 56.0),
                );
                let painter = ui.painter();
                painter.rect_filled(pill, CornerRadius::same(28), pill_color());
                painter.rect_stroke(
                    pill,
                    CornerRadius::same(28),
                    Stroke::new(1.0, Color32::from_white_alpha(40)),
                    StrokeKind::Outside,
                );

                // Red Rec button.
                let rec_center = pos2(pill.left() + 34.0, pill.center().y);
                let rec = ui.interact(
                    Rect::from_center_size(rec_center, vec2(40.0, 40.0)),
                    ui.scope_id().with("rec"),
                    Sense::click(),
                );
                let rec_fill = if rec.hovered() {
                    RED.linear_multiply(1.15)
                } else {
                    RED
                };
                painter.circle_filled(rec_center, 16.0, rec_fill);
                painter.circle_stroke(
                    rec_center,
                    16.0,
                    Stroke::new(1.5, Color32::from_white_alpha(70)),
                );
                if rec.clicked() {
                    self.dragging_hint = !self.dragging_hint;
                }

                painter.text(
                    pos2(pill.left() + 62.0, pill.center().y - 9.0),
                    egui::Align2::LEFT_CENTER,
                    "SnagCraft viewport probe",
                    egui::FontId::proportional(13.0),
                    Color32::from_rgb(235, 238, 245),
                );
                painter.text(
                    pos2(pill.left() + 62.0, pill.center().y + 9.0),
                    egui::Align2::LEFT_CENTER,
                    &self.info,
                    egui::FontId::proportional(10.0),
                    Color32::from_rgb(160, 168, 185),
                );
                painter.text(
                    pos2(pill.right() - 12.0, pill.center().y),
                    egui::Align2::RIGHT_CENTER,
                    if self.dragging_hint {
                        "drag me ⇢ Esc quits"
                    } else {
                        "Esc quits"
                    },
                    egui::FontId::proportional(10.0),
                    Color32::from_rgb(160, 168, 185),
                );

                // Drag anywhere on the pill (Wayland-native StartDrag).
                let drag = ui.interact(pill, ui.scope_id().with("pill-drag"), Sense::drag());
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });

        if let Some((out, frames, requested)) = &mut self.selftest {
            *frames += 1;
            if *frames == 12 && !*requested {
                *requested = true;
                let out = out.clone();
                ctx.request_screenshot(move |img: Arc<egui::ColorImage>| {
                    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
                    let mut bytes = Vec::with_capacity(img.pixels.len() * 4);
                    for p in &img.pixels {
                        bytes.extend_from_slice(&p.to_array());
                    }
                    match image::save_buffer(&out, &bytes, w, h, image::ExtendedColorType::Rgba8) {
                        Ok(()) => tracing::info!("probe screenshot saved: {}", out.display()),
                        Err(e) => tracing::error!("probe screenshot failed: {e}"),
                    }
                });
            }
            if *frames >= 30 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // fully transparent surface
    }
}

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut selftest = false;
    let mut shot_out = PathBuf::from("viewport-selftest.png");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--selftest" => selftest = true,
            "--shot-out" => {
                if let Some(p) = args.next() {
                    shot_out = PathBuf::from(p);
                }
            }
            _ => {}
        }
    }

    let mut probe = Probe::new();
    if selftest {
        probe.selftest = Some((shot_out, 0, false));
    }
    tracing::info!("{}", probe.info);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([292.0, 64.0])
            .with_transparent(true)
            .with_decorations(false)
            .with_title("SnagCraft viewport probe"),
        ..Default::default()
    };
    eframe::run_native(
        "snagcraft-viewport-probe",
        options,
        Box::new(|_cc| Ok(Box::new(probe))),
    )
}
