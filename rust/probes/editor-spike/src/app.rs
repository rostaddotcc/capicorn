//! Thin egui shell for the W0 editor spike. Classic one-row toolbar,
//! light theme, blue accent #2266b3. All editing logic lives in `scene`.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::scene::{self, Annotation, Editor, Img, Pt};

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
    Create { start: Pt },
    Move { idx: usize },
}

struct TextEdit {
    pos: Pt,
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

pub struct SpikeApp {
    pub ed: Editor,
    pub tool: Tool,
    pub color: Color32,
    pub stroke_width: f32,
    pub blur_radius: u32,
    pub font_size: f32,
    pub status: String,
    pub selftest: Option<SelfTest>,
    tex: Option<egui::TextureHandle>,
    tex_base: Option<Arc<Img>>,
    drag: Option<Drag>,
    drag_cur: Option<Pt>,
    editing: Option<TextEdit>,
    selected: Option<usize>,
    font_bytes: Option<(Vec<u8>, u32)>,
}

impl SpikeApp {
    pub fn new(base: Img) -> Self {
        let font_bytes = egui::FontDefinitions::default()
            .font_data
            .get("Ubuntu-Light")
            .map(|fd| (fd.font.as_ref().as_ref().to_vec(), fd.index));
        Self {
            ed: Editor::new(base),
            tool: Tool::Arrow,
            color: Color32::from_rgb(220, 40, 40),
            stroke_width: 4.0,
            blur_radius: 8,
            font_size: 28.0,
            status: "Ready — drag on the image to annotate.".into(),
            selftest: None,
            tex: None,
            tex_base: None,
            drag: None,
            drag_cur: None,
            editing: None,
            selected: None,
            font_bytes,
        }
    }

    /// Seed the demo scene used by --selftest and the kittest snapshot.
    pub fn seeded() -> Self {
        let mut app = Self::new(scene::synthetic_image(900, 600));
        app.ed.push_ann(Annotation::Arrow {
            from: [120.0, 480.0],
            to: [330.0, 250.0],
            color: [220, 40, 40, 255],
            width: 5.0,
        });
        app.ed.push_ann(Annotation::Rect {
            rect: [420.0, 140.0, 260.0, 170.0],
            color: [34, 102, 179, 255],
            width: 4.0,
        });
        app.ed.push_ann(Annotation::Text {
            pos: [424.0, 96.0],
            text: "SnagCraft W0 spike".into(),
            color: [20, 24, 30, 255],
            size: 28.0,
        });
        app.ed.blur_region([596.0, 330.0, 230.0, 180.0], 8);
        app
    }

    pub fn load_path(&mut self, path: &std::path::Path) {
        match image::open(path) {
            Ok(img) => {
                let (w, h) = (img.width(), img.height());
                if w > scene::MAX_DIM || h > scene::MAX_DIM {
                    self.status = format!("Image too large ({w}×{h}); max {}.", scene::MAX_DIM);
                    return;
                }
                self.ed = Editor::new(img.to_rgba8());
                self.selected = None;
                self.editing = None;
                self.drag = None;
                self.status = format!("Loaded {} ({w}×{h})", path.display());
            }
            Err(e) => self.status = format!("Could not open {}: {e}", path.display()),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        {
            self.load_path(&path);
        }
    }

    fn save_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("snagcraft-spike.png")
            .add_filter("PNG", &["png"])
            .save_file()
        else {
            return;
        };
        self.save_png(&path);
    }

    fn save_png(&mut self, path: &std::path::Path) {
        let font = self
            .font_bytes
            .as_ref()
            .and_then(|(bytes, idx)| ab_glyph::FontRef::try_from_slice_and_index(bytes, *idx).ok());
        let img = scene::composite(&self.ed.doc, font.as_ref());
        let (w, h) = img.dimensions();
        match image::save_buffer(path, img.as_raw(), w, h, image::ExtendedColorType::Rgba8) {
            Ok(()) => self.status = format!("Saved {}", path.display()),
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    fn commit_text(&mut self) {
        if let Some(edit) = self.editing.take() {
            let text = edit.buf.trim().to_string();
            if !text.is_empty() {
                let c = self.color.to_array();
                self.ed.push_ann(Annotation::Text {
                    pos: edit.pos,
                    text,
                    color: c,
                    size: self.font_size,
                });
            }
        }
    }

    fn rgba(&self) -> [u8; 4] {
        self.color.to_array()
    }
}

/// Classic light theme with the SnagCraft blue accent.
pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::light();
    v.panel_fill = PANEL_FILL;
    v.window_fill = Color32::WHITE;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_rgb(249, 250, 251);
    v.selection.bg_fill = ACCENT.linear_multiply(0.25);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.corner_radius = CornerRadius::same(4);
    v.widgets.inactive.corner_radius = CornerRadius::same(4);
    v.widgets.hovered.corner_radius = CornerRadius::same(4);
    v.widgets.active.corner_radius = CornerRadius::same(4);
    v.widgets.open.corner_radius = CornerRadius::same(4);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.bg_stroke = Stroke::new(1.5, ACCENT);
    ctx.set_visuals(v);
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
}

fn icon(painter: &egui::Painter, rect: Rect, tool: Tool, color: Color32) {
    let c = rect.center();
    let s = rect.width() * 0.30;
    let stroke = Stroke::new(1.8, color);
    match tool {
        Tool::Select => {
            // Cursor triangle.
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
            let head = scene::arrow_head([from.x, from.y], [to.x, to.y], s * 0.9);
            painter.line_segment(
                [pos2(head[0][0], head[0][1]), pos2(head[1][0], head[1][1])],
                stroke,
            );
            painter.line_segment(
                [pos2(head[0][0], head[0][1]), pos2(head[2][0], head[2][1])],
                stroke,
            );
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
            // Droplet: filled circle + fading ring.
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

/// The whole editor UI in one function — shared by the eframe shell and kittest.
pub fn editor_ui(ui: &mut egui::Ui, app: &mut SpikeApp) {
    let ctx = ui.ctx().clone();

    // ---- toolbar (one row, tools left / actions right) ----
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
                if ui
                    .button("Open")
                    .on_hover_text("Open image (PNG/JPEG/WebP)")
                    .clicked()
                {
                    app.open_dialog();
                }
                if ui
                    .button("Save")
                    .on_hover_text("Export annotated PNG")
                    .clicked()
                {
                    app.save_dialog();
                }
                ui.add_enabled_ui(app.ed.history.can_undo(), |ui| {
                    if ui.button("Undo").on_hover_text("Ctrl+Z").clicked() {
                        app.ed.undo();
                    }
                });
                ui.add_enabled_ui(app.ed.history.can_redo(), |ui| {
                    if ui.button("Redo").on_hover_text("Ctrl+Shift+Z").clicked() {
                        app.ed.redo();
                    }
                });
            });
        });

    // ---- status bar ----
    egui::Panel::bottom("status")
        .frame(
            egui::Frame::new()
                .fill(PANEL_FILL)
                .inner_margin(egui::Margin::symmetric(8, 3))
                .stroke(Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (w, h) = app.ed.doc.base.dimensions();
                ui.label(
                    egui::RichText::new(format!("{w}×{h}"))
                        .color(FG_MUTED)
                        .size(11.0),
                );
                ui.separator();
                ui.label(egui::RichText::new(&app.status).color(FG_MUTED).size(11.0));
            });
        });

    // ---- keyboard shortcuts (inert while typing) ----
    if app.editing.is_none() {
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::CTRL, egui::Key::Z) {
                if i.modifiers.shift {
                    app.ed.redo();
                } else {
                    app.ed.undo();
                }
            }
            if i.consume_key(egui::Modifiers::CTRL, egui::Key::Y) {
                app.ed.redo();
            }
            if (i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
                && let Some(idx) = app.selected
            {
                app.ed.delete_ann(idx);
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

    // ---- drag & drop image load ----
    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    if let Some(file) = dropped.first() {
        app.load_path(file.path());
    }

    // ---- canvas ----
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(226, 229, 233))
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            let (iw, ih) = app.ed.doc.base.dimensions();
            if iw == 0 || ih == 0 {
                ui.label("Empty image.");
                return;
            }
            let scale = (avail.width() / iw as f32)
                .min(avail.height() / ih as f32)
                .clamp(0.01, 8.0);
            let size = vec2(iw as f32 * scale, ih as f32 * scale);
            let origin = avail.center() - size * 0.5;
            let img_rect = Rect::from_min_size(origin, size);
            let to_screen = |p: Pt| pos2(origin.x + p[0] * scale, origin.y + p[1] * scale);
            let to_image = |p: Pos2| -> Pt { [(p.x - origin.x) / scale, (p.y - origin.y) / scale] };

            // Keep the GPU texture in sync with the (Arc-shared) base image.
            let need_upload = match (&app.tex, &app.tex_base) {
                (Some(_), Some(prev)) => !Arc::ptr_eq(prev, &app.ed.doc.base),
                _ => true,
            };
            if need_upload {
                let base = &app.ed.doc.base;
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [base.width() as usize, base.height() as usize],
                    base.as_raw(),
                );
                app.tex =
                    Some(ctx.load_texture("spike-base", color_img, egui::TextureOptions::LINEAR));
                app.tex_base = Some(app.ed.doc.base.clone());
            }
            if let Some(tex) = &app.tex {
                ui.painter().image(
                    tex.id(),
                    img_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            ui.painter().rect_stroke(
                img_rect,
                CornerRadius::same(0),
                Stroke::new(1.0, BORDER),
                StrokeKind::Outside,
            );

            // Draw committed annotations.
            let painter = ui.painter();
            for (idx, ann) in app.ed.doc.anns.iter().enumerate() {
                draw_ann(painter, ann, &to_screen, scale);
                if app.selected == Some(idx) {
                    let b = ann.bounds();
                    let r = Rect::from_min_size(
                        to_screen([b[0], b[1]]),
                        vec2(b[2] * scale, b[3] * scale),
                    );
                    painter.rect_stroke(
                        r.expand(3.0),
                        CornerRadius::same(3),
                        Stroke::new(1.0, ACCENT),
                        StrokeKind::Outside,
                    );
                }
            }

            // Interaction surface.
            let resp = ui.interact(
                img_rect,
                ui.scope_id().with("canvas"),
                Sense::click_and_drag(),
            );
            if resp.hovered() && app.editing.is_none() {
                ctx.set_cursor_icon(match app.tool {
                    Tool::Select => egui::CursorIcon::Default,
                    _ => egui::CursorIcon::Crosshair,
                });
            }
            let hover_img = resp.hover_pos().map(&to_image);

            match app.tool {
                Tool::Select => {
                    if resp.clicked()
                        && let Some(p) = hover_img
                    {
                        app.selected = app.ed.doc.anns.iter().rposition(|a| {
                            let b = a.bounds();
                            p[0] >= b[0]
                                && p[0] <= b[0] + b[2]
                                && p[1] >= b[1]
                                && p[1] <= b[1] + b[3]
                        });
                    }
                    if resp.drag_started()
                        && let (Some(p), Some(idx)) = (hover_img, app.selected)
                    {
                        let b = app.ed.doc.anns.get(idx).map(|a| a.bounds());
                        let inside = b.is_some_and(|b| {
                            p[0] >= b[0]
                                && p[0] <= b[0] + b[2]
                                && p[1] >= b[1]
                                && p[1] <= b[1] + b[3]
                        });
                        if inside {
                            app.ed.history.record(&app.ed.doc);
                            app.drag = Some(Drag::Move { idx });
                        }
                    }
                    if let Some(Drag::Move { idx }) = app.drag {
                        let d = resp.drag_delta();
                        if d != Vec2::ZERO
                            && let Some(ann) = app.ed.doc.anns.get(idx).cloned()
                            && let Some(slot) = app.ed.doc.anns.get_mut(idx)
                        {
                            *slot = ann.shifted([d.x / scale, d.y / scale]);
                        }
                    }
                    if resp.drag_stopped() {
                        app.drag = None;
                    }
                }
                Tool::Text => {
                    if resp.clicked()
                        && let Some(p) = hover_img
                    {
                        app.commit_text();
                        app.editing = Some(TextEdit {
                            pos: p,
                            buf: String::new(),
                            focus_requested: false,
                        });
                    }
                }
                Tool::Arrow | Tool::Rect | Tool::Blur => {
                    if resp.drag_started()
                        && let Some(p) = hover_img
                    {
                        app.drag = Some(Drag::Create { start: p });
                        app.drag_cur = Some(p);
                    }
                    if matches!(app.drag, Some(Drag::Create { .. }))
                        && let Some(p) = hover_img.or(resp.interact_pointer_pos().map(&to_image))
                    {
                        app.drag_cur = Some(p);
                    }
                    if resp.drag_stopped()
                        && let (Some(Drag::Create { start }), Some(end)) =
                            (app.drag.take(), app.drag_cur.take())
                    {
                        let dx = end[0] - start[0];
                        let dy = end[1] - start[1];
                        if (dx * dx + dy * dy).sqrt() >= 3.0 {
                            match app.tool {
                                Tool::Arrow => app.ed.push_ann(Annotation::Arrow {
                                    from: start,
                                    to: end,
                                    color: app.rgba(),
                                    width: app.stroke_width,
                                }),
                                Tool::Rect => {
                                    let rect = [
                                        start[0].min(end[0]),
                                        start[1].min(end[1]),
                                        dx.abs(),
                                        dy.abs(),
                                    ];
                                    app.ed.push_ann(Annotation::Rect {
                                        rect,
                                        color: app.rgba(),
                                        width: app.stroke_width,
                                    });
                                }
                                Tool::Blur => {
                                    let rect = [
                                        start[0].min(end[0]),
                                        start[1].min(end[1]),
                                        dx.abs(),
                                        dy.abs(),
                                    ];
                                    app.ed.blur_region(rect, app.blur_radius);
                                    app.status =
                                        format!("Blurred region (radius {})", app.blur_radius);
                                }
                                _ => {}
                            }
                        }
                    }
                    // Live preview.
                    if let (Some(Drag::Create { start }), Some(cur)) = (&app.drag, app.drag_cur) {
                        let preview = if app.tool == Tool::Arrow {
                            Annotation::Arrow {
                                from: *start,
                                to: cur,
                                color: app.rgba(),
                                width: app.stroke_width,
                            }
                        } else {
                            Annotation::Rect {
                                rect: [
                                    start[0].min(cur[0]),
                                    start[1].min(cur[1]),
                                    (cur[0] - start[0]).abs(),
                                    (cur[1] - start[1]).abs(),
                                ],
                                color: if app.tool == Tool::Blur {
                                    Color32::from_rgb(90, 90, 90).to_array()
                                } else {
                                    app.rgba()
                                },
                                width: app.stroke_width,
                            }
                        };
                        draw_ann(ui.painter(), &preview, &to_screen, scale);
                    }
                }
            }

            // Inline text editing (no placeholder — the caret is the invitation).
            if let Some(edit) = &mut app.editing {
                let sp = to_screen(edit.pos);
                let edit_rect = Rect::from_min_size(sp, vec2(260.0, app.font_size * 1.5 + 8.0));
                let id = ui.scope_id().with("spike-text-edit");
                if !edit.focus_requested {
                    ctx.memory_mut(|m| m.request_focus(id));
                    edit.focus_requested = true;
                }
                let resp = ui.put(
                    edit_rect,
                    egui::TextEdit::singleline(&mut edit.buf)
                        .id(id)
                        .font(egui::FontId::proportional(app.font_size))
                        .text_color(app.color),
                );
                if resp.lost_focus() {
                    app.commit_text();
                }
            }
        });

    // ---- selftest: screenshot the real window, then quit ----
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

fn draw_ann(
    painter: &egui::Painter,
    ann: &Annotation,
    to_screen: &impl Fn(Pt) -> Pos2,
    scale: f32,
) {
    match ann {
        Annotation::Arrow {
            from,
            to,
            color,
            width,
        } => {
            let c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
            let a = to_screen(*from);
            let b = to_screen(*to);
            let w = width * scale;
            painter.line_segment([a, b], Stroke::new(w, c));
            let head_len = (w * 4.0).clamp(8.0, 48.0);
            let head = scene::arrow_head([a.x, a.y], [b.x, b.y], head_len);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(head[0][0], head[0][1]),
                    pos2(head[1][0], head[1][1]),
                    pos2(head[2][0], head[2][1]),
                ],
                c,
                Stroke::NONE,
            ));
        }
        Annotation::Rect { rect, color, width } => {
            let c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
            let r = Rect::from_min_size(
                to_screen([rect[0], rect[1]]),
                vec2(rect[2] * scale, rect[3] * scale),
            );
            painter.rect_stroke(
                r,
                CornerRadius::same(0),
                Stroke::new(width * scale, c),
                StrokeKind::Outside,
            );
        }
        Annotation::Text {
            pos,
            text,
            color,
            size,
        } => {
            let c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
            painter.text(
                to_screen(*pos),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(size * scale),
                c,
            );
        }
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        editor_ui(ui, self);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Opaque editor; the viewport probe overrides this for transparency.
        [0.886, 0.898, 0.914, 1.0]
    }
}
