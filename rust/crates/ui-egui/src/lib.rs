#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L6 egui shell — thin: every mutation dispatches an engine command.
//! W1 tool subset: Select, Arrow, Rect, Text, Blur (full 13-tool parity = W2).

use std::path::PathBuf;
use std::sync::mpsc;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use snagcraft_engine::Session;
use snagcraft_geom::{Point, Rect as GRect};
use snagcraft_raster::Surface;

pub const ACCENT: Color32 = Color32::from_rgb(0x22, 0x66, 0xb3);
const PANEL_FILL: Color32 = Color32::from_rgb(243, 244, 246);
const BORDER: Color32 = Color32::from_rgb(209, 213, 219);
const FG: Color32 = Color32::from_rgb(31, 41, 55);
const FG_MUTED: Color32 = Color32::from_rgb(107, 114, 128);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Arrow,
    Text,
    Rect,
    Blur,
}

impl Tool {
    pub const ALL: [Tool; 5] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Text,
        Tool::Rect,
        Tool::Blur,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select / move (V)",
            Self::Arrow => "Arrow (A)",
            Self::Text => "Text (T)",
            Self::Rect => "Rectangle (R)",
            Self::Blur => "Blur region (B) — drag a box",
        }
    }
    pub fn key(self) -> egui::Key {
        match self {
            Self::Select => egui::Key::V,
            Self::Arrow => egui::Key::A,
            Self::Text => egui::Key::T,
            Self::Rect => egui::Key::R,
            Self::Blur => egui::Key::B,
        }
    }
}

enum Drag {
    Create { start: Point },
    Move { id: u64, start: Point },
}
struct TextEdit {
    pos: Point,
    buf: String,
    focus_requested: bool,
}

pub struct SelfTest {
    pub shot_out: PathBuf,
    frames: u32,
    requested: bool,
}
impl SelfTest {
    pub fn new(shot_out: PathBuf) -> Self {
        Self {
            shot_out,
            frames: 0,
            requested: false,
        }
    }
}

pub struct SnagCraftApp {
    pub session: Session,
    pub tool: Tool,
    pub color: Color32,
    pub stroke_width: f32,
    pub blur_radius: u32,
    pub font_size: f32,
    pub status: String,
    pub selftest: Option<SelfTest>,
    dirty: bool,
    tex: Option<egui::TextureHandle>,
    drag: Option<Drag>,
    drag_cur: Option<Point>,
    editing: Option<TextEdit>,
    selected: Option<u64>,
    capture_rx: Option<mpsc::Receiver<Result<Surface, String>>>,
}

impl SnagCraftApp {
    pub fn new() -> Self {
        let font_bytes = egui::FontDefinitions::default()
            .font_data
            .get("Ubuntu-Light")
            .map(|fd| (fd.font.as_ref().as_ref().to_vec(), fd.index));
        let mut session = Session::new();
        if let Some((bytes, idx)) = &font_bytes {
            session.set_font(bytes.clone(), *idx);
        }
        let _ = session.dispatch("doc.new", serde_json::json!({"w": 1000, "h": 640}));
        Self {
            session,
            tool: Tool::Arrow,
            color: Color32::from_rgb(220, 40, 40),
            stroke_width: 4.0,
            blur_radius: 8,
            font_size: 28.0,
            status: "Ready — Capture the screen or open an image.".into(),
            selftest: None,
            dirty: true,
            tex: None,
            drag: None,
            drag_cur: None,
            editing: None,
            selected: None,
            capture_rx: None,
        }
    }

    /// Seeded demo scene (selftest + kittest snapshot), built through commands.
    pub fn seeded() -> Self {
        let mut app = Self::new();
        // Detailed base so the blur region has visible work to do (white-on-white
        // blur would be invisible). Same idea as the W0 spike's synthetic image.
        app.session.load_surface(synthetic_base(900, 600));
        let _ = app.session.dispatch("edit.add", serde_json::json!({"object": {
            "kind": "Arrow", "id": 0, "from": {"x": 120.0, "y": 480.0}, "to": {"x": 330.0, "y": 250.0},
            "color": [220, 40, 40, 255], "width": 5.0}}));
        let _ = app.session.dispatch("edit.add", serde_json::json!({"object": {
            "kind": "Shape", "id": 0, "shape": "Rect", "rect": {"x": 420.0, "y": 140.0, "w": 260.0, "h": 170.0},
            "color": [34, 102, 179, 255], "width": 4.0}}));
        let _ = app.session.dispatch(
            "edit.add",
            serde_json::json!({"object": {
            "kind": "Text", "id": 0, "pos": {"x": 424.0, "y": 96.0}, "text": "SnagCraft W1 (Rust)",
            "color": [20, 24, 30, 255], "size": 28.0, "bold": false, "italic": false}}),
        );
        let _ = app.session.dispatch("edit.blur", serde_json::json!({
            "rect": {"x": 596.0, "y": 330.0, "w": 230.0, "h": 180.0}, "mode": "gaussian", "radius": 8}));
        app.dirty = true;
        app
    }

    pub fn start_capture(&mut self) {
        if self.capture_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.capture_rx = Some(rx);
        self.status = "Capturing… answer the OS picker if one appears.".into();
        std::thread::spawn(move || {
            let result = snagcraft_capture::capture_full_blocking().map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn poll_capture(&mut self) {
        let Some(rx) = &self.capture_rx else { return };
        match rx.try_recv() {
            Ok(Ok(surface)) => {
                let (w, h) = (surface.width(), surface.height());
                self.session.load_surface(surface);
                self.capture_rx = None;
                self.dirty = true;
                self.selected = None;
                self.status = format!("Captured {w}×{h} — annotate away.");
            }
            Ok(Err(e)) => {
                self.capture_rx = None;
                self.status = format!("Capture failed: {e}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.capture_rx = None;
                self.status = "Capture thread ended unexpectedly.".into();
            }
        }
    }

    fn dispatch(&mut self, cmd: &str, params: serde_json::Value) {
        match self.session.dispatch(cmd, params) {
            Ok(_) => self.dirty = true,
            Err(e) => self.status = format!("{cmd}: {e}"),
        }
    }

    fn rgba(&self) -> [u8; 4] {
        self.color.to_array()
    }

    fn commit_text(&mut self) {
        if let Some(edit) = self.editing.take() {
            let text = edit.buf.trim().to_string();
            if !text.is_empty() {
                self.dispatch(
                    "edit.add",
                    serde_json::json!({"object": {
                    "kind": "Text", "id": 0, "pos": {"x": edit.pos.x, "y": edit.pos.y},
                    "text": text, "color": self.rgba(), "size": self.font_size,
                    "bold": false, "italic": false}}),
                );
            }
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        {
            self.dispatch(
                "doc.open",
                serde_json::json!({"path": path.to_string_lossy()}),
            );
            self.status = format!("Opened {}", path.display());
        }
    }

    fn save_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("capture.snagcraft")
            .add_filter("SnagCraft", &["snagcraft"])
            .save_file()
        {
            self.dispatch(
                "doc.save",
                serde_json::json!({"path": path.to_string_lossy()}),
            );
            self.status = format!("Saved {}", path.display());
        }
    }

    fn export_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("capture.png")
            .add_filter("PNG", &["png"])
            .save_file()
        {
            self.dispatch(
                "doc.export",
                serde_json::json!({"path": path.to_string_lossy(), "format": "png"}),
            );
            self.status = format!("Exported {}", path.display());
        }
    }
}

impl Default for SnagCraftApp {
    fn default() -> Self {
        Self::new()
    }
}

/// Gradient + checker + white plate + dark disc: a base with detail for demos.
fn synthetic_base(w: u32, h: u32) -> Surface {
    let mut img = Surface::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let check = ((x / 8) + (y / 8)) % 2 == 0;
            let boost = if check { 22u8 } else { 0u8 };
            img.put_pixel(
                x,
                y,
                image::Rgba([
                    (40.0 + 180.0 * fx) as u8 + boost,
                    (70.0 + 120.0 * fy) as u8 + boost,
                    (160.0 + 60.0 * (1.0 - fx)) as u8 + boost,
                    255,
                ]),
            );
        }
    }
    for y in (h / 5)..(h / 5 + h / 4) {
        for x in (w / 6)..(w / 6 + w / 3) {
            img.put_pixel(x, y, image::Rgba([245, 245, 245, 255]));
        }
    }
    let (cx, cy, rad) = (w as f32 * 0.72, h as f32 * 0.62, (w.min(h) as f32) * 0.16);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d < rad {
                img.put_pixel(x, y, image::Rgba([30, 30, 34, 255]));
            }
        }
    }
    img
}

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::light();
    v.panel_fill = PANEL_FILL;
    v.window_fill = Color32::WHITE;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_rgb(249, 250, 251);
    v.selection.bg_fill = ACCENT.linear_multiply(0.25);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(4);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.bg_stroke = Stroke::new(1.5, ACCENT);
    ctx.set_visuals(v);
}

fn icon(painter: &egui::Painter, rect: Rect, tool: Tool, color: Color32) {
    let c = rect.center();
    let s = rect.width() * 0.30;
    let stroke = Stroke::new(1.8, color);
    match tool {
        Tool::Select => {
            let tip = pos2(c.x - s * 0.55, c.y - s * 0.75);
            let a = pos2(tip.x, tip.y + s * 1.5);
            let b = pos2(tip.x + s * 0.55, tip.y + s * 1.05);
            let d = pos2(tip.x + s * 1.05, tip.y + s * 0.85);
            painter.line_segment([tip, a], stroke);
            painter.line_segment([tip, d], stroke);
            painter.line_segment([a, b], stroke);
            painter.line_segment([b, d], stroke);
        }
        Tool::Arrow => {
            let from = pos2(c.x - s * 0.8, c.y + s * 0.8);
            let to = pos2(c.x + s * 0.7, c.y - s * 0.7);
            painter.line_segment([from, to], stroke);
            painter.line_segment([to, pos2(to.x - s * 0.7, to.y)], stroke);
            painter.line_segment([to, pos2(to.x, to.y + s * 0.7)], stroke);
        }
        Tool::Text => {
            painter.line_segment(
                [
                    pos2(c.x - s * 0.7, c.y - s * 0.7),
                    pos2(c.x + s * 0.7, c.y - s * 0.7),
                ],
                stroke,
            );
            painter.line_segment([pos2(c.x, c.y - s * 0.7), pos2(c.x, c.y + s * 0.8)], stroke);
        }
        Tool::Rect => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(s * 1.7, s * 1.3)),
                CornerRadius::same(1),
                stroke,
                StrokeKind::Outside,
            );
        }
        Tool::Blur => {
            painter.circle_filled(pos2(c.x, c.y + s * 0.15), s * 0.55, color);
            painter.circle_stroke(
                pos2(c.x, c.y + s * 0.15),
                s * 0.95,
                Stroke::new(1.2, color.linear_multiply(0.5)),
            );
        }
    }
}

fn tool_button(ui: &mut egui::Ui, tool: Tool, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let fill = if active {
        ACCENT.linear_multiply(0.16)
    } else if resp.hovered() {
        Color32::from_rgb(229, 231, 235)
    } else {
        Color32::WHITE
    };
    let border = if active { ACCENT } else { BORDER };
    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(if active { 1.5 } else { 1.0 }, border),
        StrokeKind::Outside,
    );
    icon(ui.painter(), rect, tool, if active { ACCENT } else { FG });
    resp.on_hover_text(tool.label())
}

/// The whole shell in one fn — shared by eframe and headless kittest.
pub fn app_ui(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    let ctx = ui.ctx().clone();
    app.poll_capture();

    egui::Panel::top("toolbar")
        .frame(
            egui::Frame::new()
                .fill(PANEL_FILL)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .stroke(Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                let capturing = app.capture_rx.is_some();
                let cap_btn = ui.add_enabled(
                    !capturing,
                    egui::Button::new(
                        egui::RichText::new("Capture")
                            .color(Color32::WHITE)
                            .strong(),
                    )
                    .fill(ACCENT),
                );
                if cap_btn
                    .on_hover_text("Capture the full screen via the OS portal")
                    .clicked()
                {
                    app.start_capture();
                }
                ui.separator();
                for tool in Tool::ALL {
                    if tool_button(ui, tool, app.tool == tool).clicked() {
                        app.commit_text();
                        app.tool = tool;
                    }
                }
                ui.separator();
                ui.color_edit_button_srgba(&mut app.color);
                ui.add(
                    egui::DragValue::new(&mut app.stroke_width)
                        .range(1.0..=12.0)
                        .speed(0.1)
                        .prefix("w "),
                )
                .on_hover_text("Stroke width");
                match app.tool {
                    Tool::Blur => {
                        ui.add(
                            egui::DragValue::new(&mut app.blur_radius)
                                .range(1..=64)
                                .speed(0.2)
                                .prefix("r "),
                        )
                        .on_hover_text("Blur radius");
                    }
                    Tool::Text => {
                        ui.add(
                            egui::DragValue::new(&mut app.font_size)
                                .range(8.0..=96.0)
                                .speed(0.5)
                                .prefix("px "),
                        )
                        .on_hover_text("Font size");
                    }
                    _ => {}
                }
                ui.separator();
                if ui.button("Open").clicked() {
                    app.open_dialog();
                }
                if ui
                    .button("Save")
                    .on_hover_text("Save native .snagcraft (editable)")
                    .clicked()
                {
                    app.save_dialog();
                }
                if ui
                    .button("Export")
                    .on_hover_text("Export flattened PNG")
                    .clicked()
                {
                    app.export_dialog();
                }
                ui.add_enabled_ui(app.session.can_undo(), |ui| {
                    if ui.button("Undo").on_hover_text("Ctrl+Z").clicked() {
                        app.dispatch("edit.undo", serde_json::json!({}));
                    }
                });
                ui.add_enabled_ui(app.session.can_redo(), |ui| {
                    if ui.button("Redo").on_hover_text("Ctrl+Shift+Z").clicked() {
                        app.dispatch("edit.redo", serde_json::json!({}));
                    }
                });
            });
        });

    egui::Panel::bottom("status")
        .frame(
            egui::Frame::new()
                .fill(PANEL_FILL)
                .inner_margin(egui::Margin::symmetric(8, 3))
                .stroke(Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(d) = app.session.doc() {
                    ui.label(
                        egui::RichText::new(format!("{}×{}", d.size.w, d.size.h))
                            .color(FG_MUTED)
                            .size(11.0),
                    );
                    ui.separator();
                }
                ui.label(egui::RichText::new(&app.status).color(FG_MUTED).size(11.0));
            });
        });

    if app.editing.is_none() {
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::CTRL, egui::Key::Z) {
                if i.modifiers.shift {
                    app.dispatch("edit.redo", serde_json::json!({}));
                } else {
                    app.dispatch("edit.undo", serde_json::json!({}));
                }
            }
            if i.consume_key(egui::Modifiers::CTRL, egui::Key::Y) {
                app.dispatch("edit.redo", serde_json::json!({}));
            }
            if (i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
                && let Some(id) = app.selected
            {
                app.dispatch("edit.remove", serde_json::json!({"id": id}));
                app.selected = None;
            }
            for tool in Tool::ALL {
                if i.consume_key(egui::Modifiers::NONE, tool.key()) {
                    app.tool = tool;
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                app.drag = None;
                app.selected = None;
            }
        });
    }

    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    if let Some(file) = dropped.first() {
        let path = file.path().to_string_lossy().to_string();
        app.dispatch("doc.open", serde_json::json!({"path": path}));
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(Color32::from_rgb(226, 229, 233)).inner_margin(egui::Margin::same(0)))
        .show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            let Some(doc) = app.session.doc() else {
                ui.label("No document — Capture or Open to begin.");
                return;
            };
            let (iw, ih) = (doc.size.w, doc.size.h);
            if iw == 0 || ih == 0 { return; }
            let scale = (avail.width() / iw as f32).min(avail.height() / ih as f32).clamp(0.01, 8.0);
            let size = vec2(iw as f32 * scale, ih as f32 * scale);
            let origin = avail.center() - size * 0.5;
            let img_rect = Rect::from_min_size(origin, size);
            let to_screen = |p: Point| pos2(origin.x + p.x * scale, origin.y + p.y * scale);
            let to_image = |p: Pos2| Point::new((p.x - origin.x) / scale, (p.y - origin.y) / scale);

            // CPU composite → texture, cached until the next mutation.
            if (app.dirty || app.tex.is_none())
                && let Ok(img) = app.session.render() {
                    let color_img = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                    app.tex = Some(ctx.load_texture("snagcraft-doc", color_img, egui::TextureOptions::LINEAR));
                    app.dirty = false;
                }
            if let Some(tex) = &app.tex {
                ui.painter().image(tex.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(img_rect, CornerRadius::same(0), Stroke::new(1.0, BORDER), StrokeKind::Outside);

            // Selection outline (objects are baked into the composite).
            if let Some(id) = app.selected
                && let Some(obj) = app.session.doc().and_then(|d| d.get(id)) {
                    let b = obj.bounds();
                    let r = Rect::from_min_size(to_screen(Point::new(b.x, b.y)), vec2(b.w * scale, b.h * scale));
                    ui.painter().rect_stroke(r.expand(3.0), CornerRadius::same(3), Stroke::new(1.0, ACCENT), StrokeKind::Outside);
                }

            let resp = ui.interact(img_rect, ui.scope_id().with("canvas"), Sense::click_and_drag());
            if resp.hovered() && app.editing.is_none() {
                ctx.set_cursor_icon(match app.tool { Tool::Select => egui::CursorIcon::Default, _ => egui::CursorIcon::Crosshair });
            }
            let hover_img = resp.hover_pos().map(&to_image);

            match app.tool {
                Tool::Select => {
                    if resp.clicked()
                        && let Some(p) = hover_img {
                            app.selected = app.session.doc().and_then(|d| d.hit_test(p));
                        }
                    if resp.drag_started()
                        && let (Some(p), Some(id)) = (hover_img, app.selected) {
                            let inside = app.session.doc().and_then(|d| d.get(id)).is_some_and(|o| o.bounds().contains(p));
                            if inside { app.drag = Some(Drag::Move { id, start: p }); app.drag_cur = Some(p); }
                        }
                    if let Some(Drag::Move { .. }) = app.drag
                        && let Some(p) = hover_img.or(resp.interact_pointer_pos().map(&to_image)) { app.drag_cur = Some(p); }
                    if resp.drag_stopped()
                        && let (Some(Drag::Move { id, start }), Some(end)) = (app.drag.take(), app.drag_cur.take()) {
                            let (dx, dy) = (end.x - start.x, end.y - start.y);
                            if dx.abs() > 0.5 || dy.abs() > 0.5 {
                                app.dispatch("edit.move", serde_json::json!({"id": id, "dx": dx, "dy": dy}));
                            }
                        }
                    // Live move preview: outline follows the pointer.
                    if let (Some(Drag::Move { id, start }), Some(cur)) = (&app.drag, app.drag_cur)
                        && let Some(obj) = app.session.doc().and_then(|d| d.get(*id)) {
                            let b = obj.bounds();
                            let r = Rect::from_min_size(to_screen(Point::new(b.x + cur.x - start.x, b.y + cur.y - start.y)), vec2(b.w * scale, b.h * scale));
                            ui.painter().rect_stroke(r.expand(3.0), CornerRadius::same(3), Stroke::new(1.0, ACCENT.linear_multiply(0.6)), StrokeKind::Outside);
                        }
                }
                Tool::Text => {
                    if resp.clicked()
                        && let Some(p) = hover_img {
                            app.commit_text();
                            app.editing = Some(TextEdit { pos: p, buf: String::new(), focus_requested: false });
                        }
                }
                Tool::Arrow | Tool::Rect | Tool::Blur => {
                    if resp.drag_started()
                        && let Some(p) = hover_img { app.drag = Some(Drag::Create { start: p }); app.drag_cur = Some(p); }
                    if matches!(app.drag, Some(Drag::Create { .. }))
                        && let Some(p) = hover_img.or(resp.interact_pointer_pos().map(&to_image)) { app.drag_cur = Some(p); }
                    if resp.drag_stopped()
                        && let (Some(Drag::Create { start }), Some(end)) = (app.drag.take(), app.drag_cur.take()) {
                            let dx = end.x - start.x;
                            let dy = end.y - start.y;
                            if (dx * dx + dy * dy).sqrt() >= 3.0 {
                                let rect = GRect::from_corners(start, end);
                                let rj = serde_json::json!({"x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h});
                                match app.tool {
                                    Tool::Arrow => app.dispatch("edit.add", serde_json::json!({"object": {
                                        "kind": "Arrow", "id": 0, "from": {"x": start.x, "y": start.y}, "to": {"x": end.x, "y": end.y},
                                        "color": app.rgba(), "width": app.stroke_width}})),
                                    Tool::Rect => app.dispatch("edit.add", serde_json::json!({"object": {
                                        "kind": "Shape", "id": 0, "shape": "Rect", "rect": rj, "color": app.rgba(), "width": app.stroke_width}})),
                                    Tool::Blur => app.dispatch("edit.blur", serde_json::json!({"rect": rj, "mode": "gaussian", "radius": app.blur_radius})),
                                    _ => {}
                                }
                            }
                        }
                    if let (Some(Drag::Create { start }), Some(cur)) = (&app.drag, app.drag_cur) {
                        let a = to_screen(*start);
                        let b = to_screen(cur);
                        let preview_color = if app.tool == Tool::Blur { Color32::from_rgb(90, 90, 90) } else { app.color };
                        if app.tool == Tool::Arrow {
                            ui.painter().line_segment([a, b], Stroke::new(app.stroke_width * scale, preview_color));
                        } else {
                            let r = Rect::from_two_pos(a, b);
                            ui.painter().rect_stroke(r, CornerRadius::same(0), Stroke::new(app.stroke_width * scale, preview_color), StrokeKind::Outside);
                        }
                    }
                }
            }

            if let Some(edit) = &mut app.editing {
                let sp = to_screen(edit.pos);
                let edit_rect = Rect::from_min_size(sp, vec2(260.0, app.font_size * 1.5 + 8.0));
                let id = ui.scope_id().with("text-edit");
                if !edit.focus_requested {
                    ctx.memory_mut(|m| m.request_focus(id));
                    edit.focus_requested = true;
                }
                let resp = ui.put(edit_rect, egui::TextEdit::singleline(&mut edit.buf).id(id)
                    .font(egui::FontId::proportional(app.font_size)).text_color(app.color));
                if resp.lost_focus() { app.commit_text(); }
            }
        });

    if let Some(st) = &mut app.selftest {
        st.frames += 1;
        if st.frames == 12 && !st.requested {
            st.requested = true;
            let out = st.shot_out.clone();
            ctx.request_screenshot(move |img| {
                let (w, h) = (img.size[0] as u32, img.size[1] as u32);
                let mut bytes = Vec::with_capacity(img.pixels.len() * 4);
                for p in &img.pixels {
                    bytes.extend_from_slice(&p.to_array());
                }
                match image::save_buffer(&out, &bytes, w, h, image::ExtendedColorType::Rgba8) {
                    Ok(()) => tracing::info!("selftest screenshot saved: {}", out.display()),
                    Err(e) => tracing::error!("selftest screenshot failed: {e}"),
                }
            });
        }
        if st.frames >= 30 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for SnagCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        app_ui(ui, self);
    }
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.886, 0.898, 0.914, 1.0]
    }
}
