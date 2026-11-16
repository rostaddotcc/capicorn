#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L6 egui shell — thin: every mutation dispatches an engine command.
//! W2b: full 13-tool parity shell — one-row icon toolbar (horizontal scroll on
//! overflow, never a second row), contextual properties rail, crop Enter/Esc
//! UX, freehand brush, auto-numbered steps, callout/magnify/bucket/spotlight,
//! and a floating quick font bar while text is being typed.

use std::path::PathBuf;
use std::sync::mpsc;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Key, Layout, Modifiers, Pos2, Rect, Response,
    Sense, Shape, Stroke, StrokeKind, pos2, vec2,
};
use snagcraft_doc::{AnnotObject, BlurMode, ShapeKind};
use snagcraft_engine::Session;
use snagcraft_geom::{Point, Rect as GRect};
use snagcraft_raster::Surface;

pub const ACCENT: Color32 = Color32::from_rgb(0x22, 0x66, 0xb3);
const PANEL_FILL: Color32 = Color32::from_rgb(243, 244, 246);
const BORDER: Color32 = Color32::from_rgb(209, 213, 219);
const FG: Color32 = Color32::from_rgb(31, 41, 55);
const FG_MUTED: Color32 = Color32::from_rgb(107, 114, 128);

/// Live 8-swatch rail palette (`app.ts:330-339`): (name, rgb).
pub const PALETTE: [(&str, [u8; 3]); 8] = [
    ("Red", [0xff, 0x3b, 0x30]),
    ("Orange", [0xff, 0x95, 0x00]),
    ("Yellow", [0xff, 0xcc, 0x00]),
    ("Green", [0x34, 0xc7, 0x59]),
    ("Blue", [0x00, 0x7a, 0xff]),
    ("Purple", [0x8b, 0x7c, 0xf6]),
    ("Black", [0x1c, 0x1c, 0x1e]),
    ("White", [0xff, 0xff, 0xff]),
];

/// The 10 bundled OFL families (`app.ts:471-487`).
pub const FAMILIES: [&str; 10] = [
    "Inter",
    "Lato",
    "Source Serif 4",
    "Playfair Display",
    "Oswald",
    "Archivo Black",
    "JetBrains Mono",
    "Fira Code",
    "Caveat",
    "Pacifico",
];

/// Live stroke clamp (`app.ts:731`).
pub const STROKE_MIN: f32 = 1.0;
pub const STROKE_MAX: f32 = 12.0;
/// Brush nib (`tools.ts:1824-1916`, doc contract 2..=44).
pub const BRUSH_MIN: f32 = 2.0;
pub const BRUSH_MAX: f32 = 44.0;
/// Text style size clamp (`app.ts:738`).
pub const FONT_MIN: f32 = 8.0;
pub const FONT_MAX: f32 = 200.0;
/// Magnify zoom (`tools.ts:193-195`).
pub const ZOOM_MIN: f32 = 1.5;
pub const ZOOM_MAX: f32 = 10.0;
pub const ZOOM_DEFAULT: f32 = 3.0;
/// Blur radius (`blur.ts:20-22`).
pub const BLUR_MIN: u32 = 2;
pub const BLUR_MAX: u32 = 60;
pub const BLUR_DEFAULT: u32 = 12;
/// Step badge diameter (`editor.ts:136-137`, rail label "Ring width").
pub const STEP_DIAM_MIN: f32 = 16.0;
pub const STEP_DIAM_MAX: f32 = 64.0;
pub const STEP_DIAM_DEFAULT: f32 = 30.0;
/// Callout caption size (`editor.ts:140-141`).
pub const CALLOUT_FONT_MIN: f32 = 12.0;
pub const CALLOUT_FONT_MAX: f32 = 120.0;
/// Callout box defaults (`tools.ts:1296-1300`).
const CALLOUT_DEFAULT_W: f32 = 240.0;
const CALLOUT_DEFAULT_H: f32 = 112.0;
const CALLOUT_MIN_W: f32 = 140.0;
const CALLOUT_MIN_H: f32 = 72.0;
/// Freehand sample cap (`tools.ts:1790`).
const BRUSH_MAX_POINTS: usize = 1200;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Crop,
    Arrow,
    Text,
    Shape,
    Blur,
    Bucket,
    Brush,
    Spotlight,
    Highlighter,
    Callout,
    Step,
    Magnify,
}

impl Tool {
    /// Toolbar order and bare-letter routing (`app.ts:5156-5170`).
    pub const ALL: [Tool; 13] = [
        Tool::Select,
        Tool::Crop,
        Tool::Arrow,
        Tool::Text,
        Tool::Shape,
        Tool::Blur,
        Tool::Bucket,
        Tool::Brush,
        Tool::Spotlight,
        Tool::Highlighter,
        Tool::Callout,
        Tool::Step,
        Tool::Magnify,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select (V)",
            Self::Crop => "Crop (C)",
            Self::Arrow => "Arrow (A)",
            Self::Text => "Text (T)",
            Self::Shape => "Shape (R)",
            Self::Blur => "Blur (B)",
            Self::Bucket => "Bucket (G)",
            Self::Brush => "Brush (E)",
            Self::Spotlight => "Spotlight (L)",
            Self::Highlighter => "Highlighter (H)",
            Self::Callout => "Callout (K)",
            Self::Step => "Step (S)",
            Self::Magnify => "Magnify (M)",
        }
    }

    pub fn key(self) -> Key {
        match self {
            Self::Select => Key::V,
            Self::Crop => Key::C,
            Self::Arrow => Key::A,
            Self::Text => Key::T,
            Self::Shape => Key::R,
            Self::Blur => Key::B,
            Self::Bucket => Key::G,
            Self::Brush => Key::E,
            Self::Spotlight => Key::L,
            Self::Highlighter => Key::H,
            Self::Callout => Key::K,
            Self::Step => Key::S,
            Self::Magnify => Key::M,
        }
    }

    /// Rail hint, verbatim intent of the live `PROPS` table (`app.ts:348-380`).
    pub fn hint(self) -> &'static str {
        match self {
            Self::Select => {
                "Click an annotation to select it, then drag to move. Delete removes it."
            }
            Self::Crop => "Drag to frame the shot, then press Enter to crop (Esc cancels).",
            Self::Arrow => "Drag to draw an arrow. Width sets how heavy it looks.",
            Self::Text => "Click where the text should start, then type.",
            Self::Shape => "Drag to draw the chosen outline.",
            Self::Blur => "Drag over anything private — intensity sets how hard it hides.",
            Self::Bucket => "Click a closed area to flood-fill it with the current color.",
            Self::Brush => "Drag to draw freehand — size sets the nib.",
            Self::Spotlight => "Drag to light a region; everything outside it is dimmed.",
            Self::Highlighter => "Swipe across text to mark it.",
            Self::Callout => "Drag a speech bubble (or click for the default box).",
            Self::Step => "Click to drop the next numbered step — numbers stay in order.",
            Self::Magnify => "",
        }
    }
}

fn shape_name(kind: ShapeKind) -> &'static str {
    match kind {
        ShapeKind::Rect => "Rect",
        ShapeKind::Ellipse => "Ellipse",
        ShapeKind::Triangle => "Triangle",
        ShapeKind::Line => "Line",
        ShapeKind::Star => "Star",
    }
}

fn blur_mode_name(mode: BlurMode) -> &'static str {
    match mode {
        BlurMode::Gaussian => "gaussian",
        BlurMode::Pixelate => "pixelate",
        BlurMode::Blackout => "blackout",
    }
}

fn rect_json(r: GRect) -> serde_json::Value {
    serde_json::json!({"x": r.x, "y": r.y, "w": r.w, "h": r.h})
}

/// Shadow UI (direction/distance/opacity) → stored offsets (`app.ts:643-657`).
pub fn shadow_offsets(angle_deg: f32, distance: f32) -> (f32, f32) {
    let radians = angle_deg.to_radians();
    (
        (radians.cos() * distance).round(),
        (radians.sin() * distance).round(),
    )
}

/// Callout width drives the caption size (`tools.ts:1387-1391`): the smaller of
/// the rail setting and a quarter of the box width, never below 12.
pub fn callout_font_size(rect_w: f32, rail: f32) -> f32 {
    let rail = if rail.is_finite() {
        rail.clamp(CALLOUT_FONT_MIN, CALLOUT_FONT_MAX)
    } else {
        CALLOUT_FONT_MIN
    };
    let by_width = (rect_w / 4.0).round();
    rail.min(by_width).max(CALLOUT_FONT_MIN)
}

/// Auto-numbering (`editor.ts:1464-1476`): highest existing step + 1.
pub fn next_step_number(objects: &[AnnotObject]) -> u32 {
    objects
        .iter()
        .filter_map(|o| match o {
            AnnotObject::Step { number, .. } => Some(*number),
            _ => None,
        })
        .max()
        .map_or(1, |n| n.saturating_add(1))
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
    pub shape_kind: ShapeKind,
    pub blur_mode: BlurMode,
    pub blur_radius: u32,
    pub font_family: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub shadow_enabled: bool,
    pub shadow_angle: f32,
    pub shadow_distance: f32,
    pub shadow_opacity: f32,
    pub shadow_blur: f32,
    pub brush_width: f32,
    pub step_diameter: f32,
    pub callout_size: f32,
    pub magnify_zoom: f32,
    pub bucket_tolerance: u8,
    pub spotlight_dim: f32,
    pub status: String,
    pub selftest: Option<SelfTest>,
    dirty: bool,
    tex: Option<egui::TextureHandle>,
    drag: Option<Drag>,
    drag_cur: Option<Point>,
    /// In-progress crop frame (image coordinates); survives pointer release.
    pub crop: Option<GRect>,
    brush_pts: Vec<Point>,
    editing: Option<TextEdit>,
    pub selected: Option<u64>,
    capture_rx: Option<mpsc::Receiver<Result<Surface, String>>>,
}

impl SnagCraftApp {
    pub fn new() -> Self {
        let mut session = Session::new();
        let _ = session.dispatch("doc.new", serde_json::json!({"w": 1000, "h": 640}));
        Self {
            session,
            tool: Tool::Arrow,
            color: Color32::from_rgb(0xff, 0x3b, 0x30),
            stroke_width: 3.0,
            shape_kind: ShapeKind::Rect,
            blur_mode: BlurMode::Gaussian,
            blur_radius: BLUR_DEFAULT,
            font_family: "Inter".to_string(),
            font_size: 32.0,
            bold: false,
            italic: false,
            shadow_enabled: false,
            shadow_angle: 90.0,
            shadow_distance: 4.0,
            shadow_opacity: 45.0,
            shadow_blur: 4.0,
            brush_width: 6.0,
            step_diameter: STEP_DIAM_DEFAULT,
            callout_size: 24.0,
            magnify_zoom: ZOOM_DEFAULT,
            bucket_tolerance: 16,
            spotlight_dim: 0.6,
            status: "Ready — Capture the screen or open an image.".into(),
            selftest: None,
            dirty: true,
            tex: None,
            drag: None,
            drag_cur: None,
            crop: None,
            brush_pts: Vec::new(),
            editing: None,
            selected: None,
            capture_rx: None,
        }
    }

    /// Seeded demo scene (selftest + kittest snapshot), built through commands:
    /// one of every object kind the engine renders.
    pub fn seeded() -> Self {
        let mut app = Self::new();
        // Detailed base so blur/spotlight/magnify have visible work to do.
        app.session.load_surface(synthetic_base(900, 600));
        let shadow = serde_json::json!({
            "enabled": true, "color": [0, 0, 0, 115], "blur": 6,
            "offset_x": 3.0, "offset_y": 4.0});
        let add = |app: &mut Self, obj: serde_json::Value| {
            let _ = app
                .session
                .dispatch("edit.add", serde_json::json!({"object": obj}));
        };
        // Spotlight first: it dims the base outside its hole, later objects stay bright.
        add(
            &mut app,
            serde_json::json!({"kind": "Spotlight", "id": 0,
                "rect": {"x": 40.0, "y": 40.0, "w": 380.0, "h": 250.0}, "dim": 0.45}),
        );
        let _ = app.session.dispatch(
            "edit.blur",
            serde_json::json!({"rect": {"x": 700.0, "y": 60.0, "w": 150.0, "h": 90.0},
                "mode": "gaussian", "radius": 12}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Highlighter", "id": 0,
                "rect": {"x": 170.0, "y": 218.0, "w": 240.0, "h": 24.0},
                "color": [255, 204, 0, 255]}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Arrow", "id": 0,
                "from": {"x": 110.0, "y": 470.0}, "to": {"x": 330.0, "y": 292.0},
                "color": [255, 59, 48, 255], "width": 6.0, "shadow": shadow}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Text", "id": 0,
                "pos": {"x": 420.0, "y": 96.0}, "text": "SnagCraft W2 (Rust)",
                "color": [20, 24, 30, 255], "size": 30.0, "bold": true,
                "italic": false, "family": "Inter", "shadow": shadow}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Shape", "id": 0, "shape": "Rect",
                "rect": {"x": 420.0, "y": 150.0, "w": 260.0, "h": 150.0},
                "color": [34, 102, 179, 255], "width": 4.0, "shadow": shadow}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Shape", "id": 0, "shape": "Star",
                "rect": {"x": 700.0, "y": 180.0, "w": 86.0, "h": 82.0},
                "color": [255, 204, 0, 255], "width": 3.0, "shadow": shadow}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Callout", "id": 0,
                "rect": {"x": 70.0, "y": 330.0, "w": 230.0, "h": 110.0},
                "text": "Callout (K)", "color": [28, 32, 40, 255], "size": 28.0,
                "family": "Inter", "shadow": shadow}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Step", "id": 0, "pos": {"x": 560.0, "y": 430.0},
                "number": 1, "color": [0, 122, 255, 255], "radius": 16.0, "shadow": shadow}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "Magnify", "id": 0,
                "rect": {"x": 310.0, "y": 320.0, "w": 170.0, "h": 120.0}, "zoom": 3.0}),
        );
        add(
            &mut app,
            serde_json::json!({"kind": "BrushStroke", "id": 0,
                "points": [{"x": 140.0, "y": 520.0}, {"x": 200.0, "y": 540.0},
                    {"x": 270.0, "y": 522.0}, {"x": 340.0, "y": 548.0}, {"x": 400.0, "y": 528.0}],
                "color": [139, 124, 246, 255], "width": 9.0}),
        );
        app.tool = Tool::Shape;
        app.shadow_enabled = true;
        app.dirty = true;
        app
    }

    /// True while an inline text editor is open (tests + quick-bar gating).
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// The inline editor's current buffer (empty when not editing).
    pub fn text_buffer(&self) -> &str {
        self.editing.as_ref().map_or("", |e| e.buf.as_str())
    }

    /// True while a create/move drag is in flight.
    pub fn drag_active(&self) -> bool {
        self.drag.is_some()
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

    /// Copy the flattened composite to the system clipboard (Ctrl+C / menu).
    fn copy_to_clipboard(&mut self) {
        match self
            .session
            .dispatch("clipboard.copy", serde_json::json!({}))
        {
            Ok(v) => {
                self.status = format!(
                    "Copied {}×{} image to clipboard — paste anywhere.",
                    v["width"], v["height"]
                );
            }
            Err(e) => self.status = format!("Copy failed: {e}"),
        }
    }

    /// Paste an image from the system clipboard (Ctrl+V / menu).
    fn paste_from_clipboard(&mut self) {
        match self
            .session
            .dispatch("clipboard.paste", serde_json::json!({}))
        {
            Ok(v) => {
                if v["pasted"].as_bool().unwrap_or(false) {
                    self.dirty = true;
                    self.selected = None;
                    self.status = format!("Pasted {}×{} from clipboard.", v["width"], v["height"]);
                } else {
                    self.status = "Clipboard has no image to paste.".into();
                }
            }
            Err(e) => self.status = format!("Paste failed: {e}"),
        }
    }

    fn color_json(&self) -> serde_json::Value {
        serde_json::json!(self.color.to_array())
    }

    /// Shadow UI state → stored `Shadow` JSON (`color` black at the rail opacity).
    pub fn shadow_value(&self) -> serde_json::Value {
        let opacity = self.shadow_opacity.clamp(2.0, 100.0) / 100.0;
        let alpha = (opacity * 255.0).round().clamp(0.0, 255.0) as u8;
        let (offset_x, offset_y) = shadow_offsets(self.shadow_angle, self.shadow_distance);
        serde_json::json!({
            "enabled": self.shadow_enabled,
            "color": [0, 0, 0, alpha],
            "blur": self.shadow_blur.round().clamp(0.0, 60.0) as u32,
            "offset_x": offset_x,
            "offset_y": offset_y,
        })
    }

    fn add_object(&mut self, obj: serde_json::Value) {
        self.dispatch("edit.add", serde_json::json!({"object": obj}));
    }

    fn select_tool(&mut self, tool: Tool) {
        if self.tool == tool {
            return;
        }
        self.commit_text();
        self.drag = None;
        self.drag_cur = None;
        self.brush_pts.clear();
        self.crop = None;
        self.tool = tool;
        self.status = format!("{} — {}", tool.label(), tool.hint());
    }

    fn commit_text(&mut self) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let text = edit.buf.trim().to_string();
        if text.is_empty() {
            return;
        }
        let color = self.color_json();
        let shadow = self.shadow_value();
        let size = self.font_size;
        let bold = self.bold;
        let italic = self.italic;
        let family = self.font_family.clone();
        self.add_object(serde_json::json!({
            "kind": "Text", "id": 0,
            "pos": {"x": edit.pos.x, "y": edit.pos.y},
            "text": text, "color": color, "size": size,
            "bold": bold, "italic": italic, "family": family, "shadow": shadow,
        }));
    }

    /// Commit a freshly dragged box (or click-default) for the active tool.
    fn create_from_drag(&mut self, start: Point, end: Point) {
        let (dx, dy) = (end.x - start.x, end.y - start.y);
        let dist = (dx * dx + dy * dy).sqrt();
        let rect = GRect::from_corners(start, end);

        match self.tool {
            Tool::Arrow => {
                if dist < 3.0 {
                    return;
                }
                let (color, width, shadow) =
                    (self.color_json(), self.stroke_width, self.shadow_value());
                self.add_object(serde_json::json!({
                    "kind": "Arrow", "id": 0,
                    "from": {"x": start.x, "y": start.y}, "to": {"x": end.x, "y": end.y},
                    "color": color, "width": width, "shadow": shadow}));
            }
            Tool::Shape => {
                if rect.w.max(rect.h) < 2.0 {
                    return;
                }
                let (color, width, shadow, shape) = (
                    self.color_json(),
                    self.stroke_width,
                    self.shadow_value(),
                    shape_name(self.shape_kind),
                );
                self.add_object(serde_json::json!({
                    "kind": "Shape", "id": 0, "shape": shape, "rect": rect_json(rect),
                    "color": color, "width": width, "shadow": shadow}));
            }
            Tool::Blur => {
                if rect.w < 2.0 || rect.h < 2.0 {
                    return;
                }
                let (mode, radius) = (blur_mode_name(self.blur_mode), self.blur_radius);
                self.dispatch(
                    "edit.blur",
                    serde_json::json!({"rect": rect_json(rect), "mode": mode, "radius": radius}),
                );
            }
            Tool::Spotlight => {
                if rect.w < 2.0 || rect.h < 2.0 {
                    return;
                }
                let dim = self.spotlight_dim.clamp(0.0, 1.0);
                self.add_object(serde_json::json!({
                    "kind": "Spotlight", "id": 0, "rect": rect_json(rect), "dim": dim}));
            }
            Tool::Highlighter => {
                if rect.w < 2.0 || rect.h < 2.0 {
                    return;
                }
                let color = self.color_json();
                self.add_object(serde_json::json!({
                    "kind": "Highlighter", "id": 0, "rect": rect_json(rect), "color": color}));
            }
            Tool::Callout => {
                let small = dx.abs() < 8.0 || dy.abs() < 8.0;
                let (w, h) = if small {
                    (CALLOUT_DEFAULT_W, CALLOUT_DEFAULT_H)
                } else {
                    (rect.w.max(CALLOUT_MIN_W), rect.h.max(CALLOUT_MIN_H))
                };
                let box_rect = GRect::new(rect.x, rect.y, w, h);
                let size = callout_font_size(w, self.callout_size);
                let (color, shadow) = (self.color_json(), self.shadow_value());
                let family = self.font_family.clone();
                self.add_object(serde_json::json!({
                    "kind": "Callout", "id": 0, "rect": rect_json(box_rect),
                    "text": "Callout", "color": color, "size": size,
                    "family": family, "shadow": shadow}));
            }
            Tool::Magnify => {
                if rect.w < 2.0 || rect.h < 2.0 {
                    return;
                }
                let zoom = self.magnify_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
                self.add_object(serde_json::json!({
                    "kind": "Magnify", "id": 0, "rect": rect_json(rect), "zoom": zoom}));
            }
            _ => {}
        }
    }

    fn add_step(&mut self, pos: Point) {
        let number = self
            .session
            .doc()
            .map_or(1, |d| next_step_number(&d.objects));
        let radius = (self.step_diameter.clamp(STEP_DIAM_MIN, STEP_DIAM_MAX)) / 2.0;
        let (color, shadow) = (self.color_json(), self.shadow_value());
        self.add_object(serde_json::json!({
            "kind": "Step", "id": 0, "pos": {"x": pos.x, "y": pos.y},
            "number": number, "color": color, "radius": radius, "shadow": shadow}));
    }

    fn add_brush_stroke(&mut self) {
        let pts = std::mem::take(&mut self.brush_pts);
        if pts.is_empty() {
            return;
        }
        let points: Vec<serde_json::Value> = pts
            .iter()
            .map(|p| serde_json::json!({"x": p.x, "y": p.y}))
            .collect();
        let (color, width) = (self.color_json(), self.brush_width);
        self.add_object(serde_json::json!({
            "kind": "BrushStroke", "id": 0, "points": points,
            "color": color, "width": width}));
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

/// One context-menu row: full-width button + muted shortcut hint.
fn menu_row(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        if ui.button(label).clicked() {
            clicked = true;
        }
        if !shortcut.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(shortcut);
            });
        }
    });
    clicked
}

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::light();
    v.panel_fill = PANEL_FILL;
    v.window_fill = Color32::WHITE;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_rgb(249, 250, 251);
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.slider_trailing_fill = true;
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

// ------------------------------------------------------------------ painting

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
        Tool::Crop => {
            // Four corner marks framing the shot.
            let r = Rect::from_center_size(c, vec2(s * 1.7, s * 1.7));
            let k = s * 0.6;
            let corners = [
                (r.left(), r.top(), 1.0, 1.0),
                (r.right(), r.top(), -1.0, 1.0),
                (r.left(), r.bottom(), 1.0, -1.0),
                (r.right(), r.bottom(), -1.0, -1.0),
            ];
            for (x, y, sx, sy) in corners {
                painter.line_segment([pos2(x, y), pos2(x + sx * k, y)], stroke);
                painter.line_segment([pos2(x, y), pos2(x, y + sy * k)], stroke);
            }
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
        Tool::Shape => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(s * 1.7, s * 1.3)),
                CornerRadius::same(1),
                stroke,
                StrokeKind::Middle,
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
        Tool::Bucket => {
            // Tilted bucket body + a falling drop.
            let body =
                Rect::from_center_size(pos2(c.x - s * 0.1, c.y + s * 0.35), vec2(s * 1.3, s));
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Middle);
            painter.line_segment(
                [
                    pos2(body.left() + 2.0, body.top() + 2.0),
                    pos2(c.x - s * 1.0, c.y - s * 0.95),
                ],
                stroke,
            );
            painter.circle_filled(pos2(c.x + s * 0.85, c.y - s * 0.5), 2.0, color);
        }
        Tool::Brush => {
            // Diagonal nib with a chisel tip.
            painter.line_segment(
                [
                    pos2(c.x - s * 0.9, c.y + s * 0.9),
                    pos2(c.x + s * 0.25, c.y - s * 0.25),
                ],
                Stroke::new(3.0, color),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x + s * 0.2, c.y - s * 0.1),
                    pos2(c.x + s * 0.95, c.y - s * 0.95),
                    pos2(c.x + s * 0.6, c.y + s * 0.15),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Tool::Spotlight => {
            // Beam from a source down to a lit pool.
            let apex = pos2(c.x - s * 0.95, c.y - s * 0.95);
            let pool = pos2(c.x + s * 0.3, c.y + s * 0.45);
            let radius = vec2(s * 0.7, s * 0.42);
            painter.line_segment([apex, pos2(pool.x - radius.x, pool.y)], stroke);
            painter.line_segment([apex, pos2(pool.x + radius.x, pool.y)], stroke);
            painter.add(Shape::ellipse_filled(
                pool,
                radius,
                color.linear_multiply(0.22),
            ));
            painter.add(Shape::ellipse_stroke(pool, radius, stroke));
        }
        Tool::Highlighter => {
            // Horizontal marker band.
            let band = Rect::from_center_size(pos2(c.x, c.y + s * 0.15), vec2(s * 1.9, s * 0.62));
            painter.rect_filled(band, CornerRadius::same(2), color.linear_multiply(0.45));
            painter.rect_stroke(
                band,
                CornerRadius::same(2),
                Stroke::new(1.2, color),
                StrokeKind::Middle,
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x - s * 0.95, c.y + s * 0.5),
                    pos2(c.x - s * 0.35, c.y + s * 0.5),
                    pos2(c.x - s * 0.6, c.y + s * 0.95),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Tool::Callout => {
            let body =
                Rect::from_min_size(pos2(c.x - s * 0.95, c.y - s * 0.95), vec2(s * 1.9, s * 1.2));
            painter.rect_stroke(body, CornerRadius::same(3), stroke, StrokeKind::Middle);
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(body.left() + s * 0.3, body.bottom()),
                    pos2(body.left() + s * 0.85, body.bottom()),
                    pos2(body.left() + s * 0.3, body.bottom() + s * 0.45),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Tool::Step => {
            painter.circle_stroke(c, s * 0.95, stroke);
            painter.text(
                c,
                Align2::CENTER_CENTER,
                "1",
                FontId::proportional(13.0),
                color,
            );
        }
        Tool::Magnify => {
            let lens = pos2(c.x - s * 0.2, c.y - s * 0.2);
            painter.circle_stroke(lens, s * 0.8, stroke);
            painter.line_segment(
                [
                    pos2(c.x + s * 0.42, c.y + s * 0.42),
                    pos2(c.x + s * 0.95, c.y + s * 0.95),
                ],
                Stroke::new(2.4, color),
            );
        }
    }
}

fn shape_icon(painter: &egui::Painter, rect: Rect, kind: ShapeKind, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    match kind {
        ShapeKind::Rect => {
            painter.rect_stroke(rect, CornerRadius::same(1), stroke, StrokeKind::Middle);
        }
        ShapeKind::Ellipse => {
            painter.add(Shape::ellipse_stroke(
                rect.center(),
                rect.size() * 0.5,
                stroke,
            ));
        }
        ShapeKind::Triangle => {
            painter.add(Shape::closed_line(
                vec![
                    pos2(rect.center().x, rect.top()),
                    rect.right_bottom(),
                    rect.left_bottom(),
                ],
                stroke,
            ));
        }
        ShapeKind::Line => {
            painter.line_segment([rect.left_bottom(), rect.right_top()], stroke);
        }
        ShapeKind::Star => {
            let pts = snagcraft_doc::star_outline(GRect::new(
                rect.min.x,
                rect.min.y,
                rect.width(),
                rect.height(),
            ));
            painter.add(Shape::closed_line(
                pts.iter().map(|p| pos2(p.x, p.y)).collect(),
                stroke,
            ));
        }
    }
}

fn tool_button(ui: &mut egui::Ui, tool: Tool, active: bool) -> Response {
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

// ------------------------------------------------------------------- the rail

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(FG_MUTED).size(11.5));
}

fn palette(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    for (name, rgb) in PALETTE {
        let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        let selected = app.color.to_array()[..3] == rgb;
        let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        ui.painter().rect_filled(rect, CornerRadius::same(5), color);
        let ring = if selected { ACCENT } else { BORDER };
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(5),
            Stroke::new(if selected { 2.0 } else { 1.0 }, ring),
            StrokeKind::Outside,
        );
        if resp.on_hover_text(name).clicked() {
            app.color = color;
        }
    }
    ui.color_edit_button_srgba(&mut app.color)
        .on_hover_text("Custom color");
    ui.separator();
}

fn size_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    tooltip: &str,
) {
    ui.label(egui::RichText::new(label).color(FG_MUTED).size(11.0))
        .on_hover_text(tooltip);
    ui.add_sized(
        vec2(132.0, 18.0),
        egui::Slider::new(value, range)
            .step_by(1.0)
            .integer()
            .suffix(" px")
            .trailing_fill(true),
    )
    .on_hover_text(tooltip);
}

fn shape_picker(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    for kind in [
        ShapeKind::Rect,
        ShapeKind::Ellipse,
        ShapeKind::Triangle,
        ShapeKind::Line,
        ShapeKind::Star,
    ] {
        let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
        let active = app.shape_kind == kind;
        let fill = if active {
            ACCENT.linear_multiply(0.16)
        } else if resp.hovered() {
            Color32::from_rgb(229, 231, 235)
        } else {
            Color32::WHITE
        };
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(
                if active { 1.5 } else { 1.0 },
                if active { ACCENT } else { BORDER },
            ),
            StrokeKind::Outside,
        );
        shape_icon(
            ui.painter(),
            rect.shrink(7.0),
            kind,
            if active { ACCENT } else { FG },
        );
        if resp.on_hover_text(shape_name(kind)).clicked() {
            app.shape_kind = kind;
        }
    }
    ui.separator();
}

fn font_controls(ui: &mut egui::Ui, app: &mut SnagCraftApp, salt: &str) {
    egui::ComboBox::from_id_salt(salt)
        .width(126.0)
        .selected_text(egui::RichText::new(&app.font_family).size(12.0))
        .show_ui(ui, |ui| {
            for family in FAMILIES {
                ui.selectable_value(&mut app.font_family, family.to_string(), family);
            }
        });
    ui.add_sized(
        vec2(132.0, 18.0),
        egui::Slider::new(&mut app.font_size, FONT_MIN..=FONT_MAX)
            .step_by(1.0)
            .integer()
            .suffix(" px")
            .trailing_fill(true),
    )
    .on_hover_text("Font size");
    if ui
        .selectable_label(app.bold, egui::RichText::new("B").strong())
        .on_hover_text("Bold")
        .clicked()
    {
        app.bold = !app.bold;
    }
    if ui
        .selectable_label(app.italic, egui::RichText::new("I").italics())
        .on_hover_text("Italic")
        .clicked()
    {
        app.italic = !app.italic;
    }
}

fn shadow_controls(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    ui.separator();
    ui.checkbox(&mut app.shadow_enabled, "Shadow")
        .on_hover_text("Drop shadow behind this annotation — sliders appear below");
}

/// Tools whose objects carry a shadow (mirrors compose::shadow_of).
fn tool_has_shadow(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Arrow | Tool::Shape | Tool::Text | Tool::Callout | Tool::Step
    )
}

/// Expanded shadow row: all four sliders visible at once (no hidden scroll).
fn shadow_row(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        ui.label(egui::RichText::new("Shadow").color(FG_MUTED).size(11.0));
        shadow_slider(
            ui,
            "Angle",
            &mut app.shadow_angle,
            0.0..=360.0,
            1.0,
            "°",
            "Shadow angle (0 = right, 90 = down)",
        );
        shadow_slider(
            ui,
            "Dist",
            &mut app.shadow_distance,
            0.0..=50.0,
            0.5,
            " px",
            "Shadow distance",
        );
        shadow_slider(
            ui,
            "Opacity",
            &mut app.shadow_opacity,
            2.0..=100.0,
            1.0,
            " %",
            "Shadow opacity",
        );
        shadow_slider(
            ui,
            "Blur",
            &mut app.shadow_blur,
            0.0..=60.0,
            0.5,
            " px",
            "Shadow blur",
        );
    });
}

/// One labeled shadow slider (compact, classic).
fn shadow_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    suffix: &str,
    tooltip: &str,
) {
    ui.label(egui::RichText::new(label).color(FG_MUTED).size(11.0));
    ui.add_sized(
        vec2(118.0, 18.0),
        egui::Slider::new(value, range)
            .step_by(step)
            .integer()
            .suffix(suffix)
            .trailing_fill(true),
    )
    .on_hover_text(tooltip);
}

/// Contextual properties rail: only the active tool's own groups (`app.ts:348-380`).
fn rail(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        let hint_text = app.tool.hint();
        if !hint_text.is_empty() {
            hint(ui, hint_text);
            ui.separator();
        }
        match app.tool {
            Tool::Select | Tool::Crop => {}
            Tool::Arrow => {
                palette(ui, app);
                size_control(
                    ui,
                    "Width",
                    &mut app.stroke_width,
                    STROKE_MIN..=STROKE_MAX,
                    "Stroke width",
                );
                shadow_controls(ui, app);
            }
            Tool::Shape => {
                shape_picker(ui, app);
                palette(ui, app);
                size_control(
                    ui,
                    "Border",
                    &mut app.stroke_width,
                    STROKE_MIN..=STROKE_MAX,
                    "Stroke width",
                );
                shadow_controls(ui, app);
            }
            Tool::Text => {
                palette(ui, app);
                font_controls(ui, app, "rail-font");
                shadow_controls(ui, app);
            }
            Tool::Blur => {
                egui::ComboBox::from_id_salt("rail-blur-mode")
                    .width(104.0)
                    .selected_text(blur_mode_name(app.blur_mode))
                    .show_ui(ui, |ui| {
                        for mode in [BlurMode::Gaussian, BlurMode::Pixelate, BlurMode::Blackout] {
                            ui.selectable_value(&mut app.blur_mode, mode, blur_mode_name(mode));
                        }
                    });
                ui.label(egui::RichText::new("Radius").color(FG_MUTED).size(11.0));
                ui.add_sized(
                    vec2(132.0, 18.0),
                    egui::Slider::new(&mut app.blur_radius, BLUR_MIN..=BLUR_MAX)
                        .trailing_fill(true),
                )
                .on_hover_text("Blur radius (2-60, raised to 10 under the hood for legibility)");
            }
            Tool::Bucket => {
                palette(ui, app);
                ui.label(egui::RichText::new("Tolerance").color(FG_MUTED).size(11.0));
                ui.add_sized(
                    vec2(132.0, 18.0),
                    egui::Slider::new(&mut app.bucket_tolerance, 0..=255).trailing_fill(true),
                )
                .on_hover_text("Flood-fill tolerance");
            }
            Tool::Brush => {
                palette(ui, app);
                size_control(
                    ui,
                    "Brush size",
                    &mut app.brush_width,
                    BRUSH_MIN..=BRUSH_MAX,
                    "Nib width",
                );
            }
            Tool::Spotlight => {
                ui.label(egui::RichText::new("Dim").color(FG_MUTED).size(11.0));
                ui.add_sized(
                    vec2(132.0, 18.0),
                    egui::Slider::new(&mut app.spotlight_dim, 0.0..=1.0)
                        .step_by(0.01)
                        .fixed_decimals(2)
                        .trailing_fill(true),
                )
                .on_hover_text("How hard everything outside the lit region is dimmed");
            }
            Tool::Highlighter => {
                palette(ui, app);
                hint(
                    ui,
                    "Ink only — the 0.36 multiply alpha is fixed engine-side.",
                );
            }
            Tool::Callout => {
                palette(ui, app);
                egui::ComboBox::from_id_salt("rail-callout-font")
                    .width(126.0)
                    .selected_text(egui::RichText::new(&app.font_family).size(12.0))
                    .show_ui(ui, |ui| {
                        for family in FAMILIES {
                            ui.selectable_value(&mut app.font_family, family.to_string(), family);
                        }
                    });
                size_control(
                    ui,
                    "Outline",
                    &mut app.callout_size,
                    CALLOUT_FONT_MIN..=CALLOUT_FONT_MAX,
                    "Callout caption size",
                );
                shadow_controls(ui, app);
            }
            Tool::Step => {
                palette(ui, app);
                size_control(
                    ui,
                    "Ring width",
                    &mut app.step_diameter,
                    STEP_DIAM_MIN..=STEP_DIAM_MAX,
                    "Step badge diameter",
                );
                shadow_controls(ui, app);
            }
            Tool::Magnify => {
                ui.label(egui::RichText::new("Zoom").color(FG_MUTED).size(11.0));
                ui.add_sized(
                    vec2(132.0, 18.0),
                    egui::Slider::new(&mut app.magnify_zoom, ZOOM_MIN..=ZOOM_MAX)
                        .step_by(0.1)
                        .fixed_decimals(1)
                        .suffix("×")
                        .trailing_fill(true),
                )
                .on_hover_text("Lens zoom (1.5-10×)");
            }
        }
    });
    if app.shadow_enabled && tool_has_shadow(app.tool) {
        shadow_row(ui, app);
    }
}

// ------------------------------------------------------------------ shortcuts

fn handle_keys(ctx: &egui::Context, app: &mut SnagCraftApp) {
    if app.editing.is_some() {
        // While typing, Esc throws the inline editor away; keys belong to the box.
        let cancel = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if cancel {
            app.editing = None;
        }
        return;
    }

    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::CTRL, Key::Z) {
            if i.modifiers.shift {
                app.dispatch("edit.redo", serde_json::json!({}));
            } else {
                app.dispatch("edit.undo", serde_json::json!({}));
            }
        }
        if i.consume_key(Modifiers::CTRL, Key::Y) {
            app.dispatch("edit.redo", serde_json::json!({}));
        }
        if i.consume_key(Modifiers::CTRL, Key::C) {
            app.copy_to_clipboard();
        }
        if i.consume_key(Modifiers::CTRL, Key::V) {
            app.paste_from_clipboard();
        }
        if i.consume_key(Modifiers::CTRL, Key::D)
            && let Some(id) = app.selected
        {
            app.dispatch("edit.duplicate", serde_json::json!({"id": id}));
        }
        if i.consume_key(Modifiers::NONE, Key::Enter)
            && let Some(frame) = app.crop.take()
            && frame.w >= 2.0
            && frame.h >= 2.0
        {
            app.selected = None;
            app.dispatch("edit.crop", serde_json::json!({"rect": rect_json(frame)}));
        }
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            app.drag = None;
            app.drag_cur = None;
            app.crop = None;
            app.brush_pts.clear();
            app.selected = None;
        }
        if (i.consume_key(Modifiers::NONE, Key::Delete)
            || i.consume_key(Modifiers::NONE, Key::Backspace))
            && let Some(id) = app.selected
        {
            app.dispatch("edit.remove", serde_json::json!({"id": id}));
            app.selected = None;
        }
        for tool in Tool::ALL {
            if i.consume_key(Modifiers::NONE, tool.key()) {
                app.select_tool(tool);
            }
        }
    });
}

// ------------------------------------------------------------------- the shell

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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let capturing = app.capture_rx.is_some();
                    ui.add_enabled_ui(app.session.can_redo(), |ui| {
                        if ui.button("Redo").on_hover_text("Ctrl+Shift+Z").clicked() {
                            app.dispatch("edit.redo", serde_json::json!({}));
                        }
                    });
                    ui.add_enabled_ui(app.session.can_undo(), |ui| {
                        if ui.button("Undo").on_hover_text("Ctrl+Z").clicked() {
                            app.dispatch("edit.undo", serde_json::json!({}));
                        }
                    });
                    if ui
                        .button("Export")
                        .on_hover_text("Export flattened PNG")
                        .clicked()
                    {
                        app.export_dialog();
                    }
                    if ui
                        .button("Copy")
                        .on_hover_text("Copy image to clipboard (Ctrl+C)")
                        .clicked()
                    {
                        app.copy_to_clipboard();
                    }
                    if ui
                        .button("Save")
                        .on_hover_text("Save native .snagcraft (editable)")
                        .clicked()
                    {
                        app.save_dialog();
                    }
                    if ui.button("Open").on_hover_text("Open an image").clicked() {
                        app.open_dialog();
                    }
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

                    // Tools fill the rest of the row, left to right, one row only:
                    // overflow scrolls horizontally instead of wrapping.
                    let width = ui.available_width();
                    ui.allocate_ui_with_layout(
                        vec2(width, 32.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            egui::ScrollArea::horizontal()
                                .id_salt("toolbar-tools")
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                                        for tool in Tool::ALL {
                                            if tool_button(ui, tool, app.tool == tool).clicked() {
                                                app.select_tool(tool);
                                            }
                                        }
                                    });
                                });
                        },
                    );
                });
            });
        });

    egui::Panel::top("props")
        .frame(
            egui::Frame::new()
                .fill(Color32::WHITE)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .stroke(Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("props-scroll")
                .show(ui, |ui| rail(ui, app));
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
                ui.label(
                    egui::RichText::new(app.tool.label().split(" (").next().unwrap_or("Tool"))
                        .color(FG)
                        .size(11.0)
                        .strong(),
                );
                ui.separator();
                ui.label(egui::RichText::new(&app.status).color(FG_MUTED).size(11.0));
            });
        });

    handle_keys(&ctx, app);

    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    if let Some(file) = dropped.first() {
        let path = file.path().to_string_lossy().to_string();
        app.dispatch("doc.open", serde_json::json!({"path": path}));
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(226, 229, 233))
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            let Some(doc) = app.session.doc() else {
                ui.label("No document — Capture or Open to begin.");
                return;
            };
            let (iw, ih) = (doc.size.w, doc.size.h);
            if iw == 0 || ih == 0 {
                return;
            }
            let scale = (avail.width() / iw as f32)
                .min(avail.height() / ih as f32)
                .clamp(0.01, 8.0);
            let size = vec2(iw as f32 * scale, ih as f32 * scale);
            let origin = avail.center() - size * 0.5;
            let img_rect = Rect::from_min_size(origin, size);
            let to_screen = |p: Point| pos2(origin.x + p.x * scale, origin.y + p.y * scale);
            let to_image = |p: Pos2| Point::new((p.x - origin.x) / scale, (p.y - origin.y) / scale);
            let clamp_img =
                |p: Point| Point::new(p.x.clamp(0.0, iw as f32), p.y.clamp(0.0, ih as f32));

            // CPU composite → texture, cached until the next mutation.
            if (app.dirty || app.tex.is_none())
                && let Ok(img) = app.session.render()
            {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                app.tex = Some(ctx.load_texture(
                    "snagcraft-doc",
                    color_img,
                    egui::TextureOptions::LINEAR,
                ));
                app.dirty = false;
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

            // Selection outline (objects are baked into the composite).
            if let Some(id) = app.selected
                && let Some(obj) = app.session.doc().and_then(|d| d.get(id))
            {
                let b = obj.bounds();
                let r = Rect::from_min_size(
                    to_screen(Point::new(b.x, b.y)),
                    vec2(b.w * scale, b.h * scale),
                );
                ui.painter().rect_stroke(
                    r.expand(3.0),
                    CornerRadius::same(3),
                    Stroke::new(1.0, ACCENT),
                    StrokeKind::Outside,
                );
            }

            let resp = ui.interact(
                img_rect,
                ui.scope_id().with("canvas"),
                Sense::click_and_drag(),
            );
            if resp.hovered() && app.editing.is_none() {
                ctx.set_cursor_icon(match app.tool {
                    Tool::Select => egui::CursorIcon::Default,
                    Tool::Text => egui::CursorIcon::Text,
                    Tool::Bucket => egui::CursorIcon::PointingHand,
                    _ => egui::CursorIcon::Crosshair,
                });
            }
            let hover_img = resp.hover_pos().map(&to_image);
            // Where the button actually went down — hover_pos is already the
            // *moved* position on the frame egui first reports a drag.
            let press_img = ctx
                .input(|i| i.pointer.press_origin())
                .map(&to_image)
                .or(hover_img);
            let pointer_img = resp.interact_pointer_pos().map(&to_image).or(hover_img);

            match app.tool {
                Tool::Select => {
                    if resp.clicked()
                        && let Some(p) = hover_img
                    {
                        app.selected = app.session.doc().and_then(|d| d.hit_test(p));
                    }
                    if resp.drag_started()
                        && let (Some(p), Some(id)) = (press_img, app.selected)
                    {
                        let inside = app
                            .session
                            .doc()
                            .and_then(|d| d.get(id))
                            .is_some_and(|o| o.bounds().contains(p));
                        if inside {
                            app.drag = Some(Drag::Move { id, start: p });
                            app.drag_cur = Some(p);
                        }
                    }
                    if let Some(Drag::Move { .. }) = app.drag
                        && let Some(p) = pointer_img
                    {
                        app.drag_cur = Some(p);
                    }
                    if resp.drag_stopped()
                        && let (Some(Drag::Move { id, start }), Some(end)) =
                            (app.drag.take(), app.drag_cur.take())
                    {
                        let (dx, dy) = (end.x - start.x, end.y - start.y);
                        if dx.abs() > 0.5 || dy.abs() > 0.5 {
                            app.dispatch(
                                "edit.move",
                                serde_json::json!({"id": id, "dx": dx, "dy": dy}),
                            );
                        }
                    }
                    if let (Some(Drag::Move { id, start }), Some(cur)) = (&app.drag, app.drag_cur)
                        && let Some(obj) = app.session.doc().and_then(|d| d.get(*id))
                    {
                        let b = obj.bounds();
                        let r = Rect::from_min_size(
                            to_screen(Point::new(b.x + cur.x - start.x, b.y + cur.y - start.y)),
                            vec2(b.w * scale, b.h * scale),
                        );
                        ui.painter().rect_stroke(
                            r.expand(3.0),
                            CornerRadius::same(3),
                            Stroke::new(1.0, ACCENT.linear_multiply(0.6)),
                            StrokeKind::Outside,
                        );
                    }
                }
                Tool::Text => {
                    if resp.clicked()
                        && let Some(p) = hover_img
                        && app.editing.is_none()
                    {
                        app.commit_text();
                        app.editing = Some(TextEdit {
                            pos: p,
                            buf: String::new(),
                            focus_requested: false,
                        });
                    }
                }
                Tool::Crop => {
                    if resp.drag_started()
                        && let Some(p) = press_img
                    {
                        app.crop = Some(GRect::new(p.x, p.y, 0.0, 0.0));
                    }
                    if resp.dragged()
                        && let (Some(p), Some(start)) =
                            (pointer_img, app.crop.as_ref().map(|r| Point::new(r.x, r.y)))
                    {
                        app.crop = Some(GRect::from_corners(start, clamp_img(p)));
                    }
                }
                Tool::Brush => {
                    if resp.drag_started()
                        && let Some(p) = press_img
                    {
                        app.brush_pts = vec![clamp_img(p)];
                    }
                    if resp.dragged()
                        && let Some(p) = pointer_img
                    {
                        let p = clamp_img(p);
                        let spacing = (app.brush_width / 4.0).max(1.0);
                        let far_enough = app
                            .brush_pts
                            .last()
                            .is_none_or(|last| (p.x - last.x).hypot(p.y - last.y) >= spacing);
                        if far_enough && app.brush_pts.len() < BRUSH_MAX_POINTS {
                            app.brush_pts.push(p);
                        }
                    }
                    if resp.drag_stopped() {
                        app.add_brush_stroke();
                    }
                }
                Tool::Bucket => {
                    if resp.clicked()
                        && let Some(p) = hover_img
                    {
                        let color = app.color_json();
                        let tolerance = app.bucket_tolerance;
                        app.dispatch(
                            "edit.bucket",
                            serde_json::json!({"seed": {"x": p.x, "y": p.y},
                                "color": color, "tolerance": tolerance}),
                        );
                    }
                }
                Tool::Step => {
                    if resp.clicked()
                        && let Some(p) = hover_img
                    {
                        app.add_step(p);
                    }
                }
                Tool::Arrow
                | Tool::Shape
                | Tool::Blur
                | Tool::Spotlight
                | Tool::Highlighter
                | Tool::Callout
                | Tool::Magnify => {
                    if resp.drag_started()
                        && let Some(p) = press_img
                    {
                        app.drag = Some(Drag::Create { start: p });
                        app.drag_cur = Some(p);
                    }
                    if matches!(app.drag, Some(Drag::Create { .. }))
                        && let Some(p) = pointer_img
                    {
                        app.drag_cur = Some(p);
                    }
                    if resp.drag_stopped()
                        && let (Some(Drag::Create { start }), Some(end)) =
                            (app.drag.take(), app.drag_cur.take())
                    {
                        app.create_from_drag(start, end);
                    }
                }
            }

            // ---- live previews (screen space) -----------------------------
            if let (Some(Drag::Create { start }), Some(cur)) = (&app.drag, app.drag_cur) {
                let a = to_screen(*start);
                let b = to_screen(cur);
                let preview_rect = Rect::from_two_pos(a, b);
                match app.tool {
                    Tool::Arrow => {
                        let to = b;
                        let head = (app.stroke_width * 3.0 * scale).max(6.0);
                        let dir = (to - a).normalized();
                        let normal = vec2(-dir.y, dir.x);
                        ui.painter().line_segment(
                            [a, to],
                            Stroke::new(app.stroke_width * scale, app.color),
                        );
                        ui.painter().add(Shape::convex_polygon(
                            vec![
                                to,
                                to - dir * head + normal * head * 0.45,
                                to - dir * head - normal * head * 0.45,
                            ],
                            app.color,
                            Stroke::NONE,
                        ));
                    }
                    Tool::Shape => match app.shape_kind {
                        ShapeKind::Ellipse => {
                            ui.painter().add(Shape::ellipse_stroke(
                                preview_rect.center(),
                                preview_rect.size() * 0.5,
                                Stroke::new(app.stroke_width * scale, app.color),
                            ));
                        }
                        ShapeKind::Line => {
                            ui.painter().line_segment(
                                [a, b],
                                Stroke::new(app.stroke_width * scale, app.color),
                            );
                        }
                        ShapeKind::Triangle => {
                            ui.painter().add(Shape::closed_line(
                                vec![
                                    pos2(preview_rect.center().x, preview_rect.top()),
                                    preview_rect.right_bottom(),
                                    preview_rect.left_bottom(),
                                ],
                                Stroke::new(app.stroke_width * scale, app.color),
                            ));
                        }
                        ShapeKind::Rect | ShapeKind::Star => {
                            ui.painter().rect_stroke(
                                preview_rect,
                                CornerRadius::same(0),
                                Stroke::new(app.stroke_width * scale, app.color),
                                StrokeKind::Inside,
                            );
                        }
                    },
                    Tool::Blur => {
                        // Live parity: dashed outline while dragging (`tools.ts:790`).
                        ui.painter().rect_filled(
                            preview_rect,
                            CornerRadius::ZERO,
                            Color32::from_black_alpha(30),
                        );
                        let ink = Stroke::new(1.5, Color32::from_rgb(70, 70, 70));
                        let corners = [
                            preview_rect.left_top(),
                            preview_rect.right_top(),
                            preview_rect.right_bottom(),
                            preview_rect.left_bottom(),
                            preview_rect.left_top(),
                        ];
                        ui.painter()
                            .extend(Shape::dashed_line(&corners, ink, 6.0, 4.0));
                    }
                    Tool::Spotlight => {
                        dim_outside_preview(ui.painter(), img_rect, preview_rect, 0.45);
                        ui.painter().rect_stroke(
                            preview_rect,
                            CornerRadius::ZERO,
                            Stroke::new(1.5, ACCENT),
                            StrokeKind::Inside,
                        );
                    }
                    Tool::Highlighter => {
                        ui.painter().rect_filled(
                            preview_rect,
                            CornerRadius::same(1),
                            app.color.linear_multiply(0.36),
                        );
                    }
                    Tool::Callout => {
                        let w = preview_rect.width().max(CALLOUT_MIN_W * scale);
                        let h = preview_rect.height().max(CALLOUT_MIN_H * scale);
                        let r = Rect::from_min_size(preview_rect.min, vec2(w, h));
                        ui.painter()
                            .rect_filled(r, CornerRadius::same(8), Color32::WHITE);
                        ui.painter().rect_stroke(
                            r,
                            CornerRadius::same(8),
                            Stroke::new(1.5, app.color),
                            StrokeKind::Inside,
                        );
                    }
                    Tool::Magnify => {
                        ui.painter().rect_stroke(
                            preview_rect,
                            CornerRadius::same(10),
                            Stroke::new(2.0, Color32::from_rgb(28, 28, 30)),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            preview_rect.right_bottom() - vec2(6.0, 4.0),
                            Align2::RIGHT_BOTTOM,
                            format!("{:.1}×", app.magnify_zoom),
                            FontId::proportional(11.0),
                            Color32::WHITE,
                        );
                    }
                    _ => {}
                }
            }

            if let Some(frame) = app.crop {
                let r = Rect::from_min_size(
                    to_screen(Point::new(frame.x, frame.y)),
                    vec2(frame.w * scale, frame.h * scale),
                );
                dim_outside_preview(ui.painter(), img_rect, r, 0.55);
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::ZERO,
                    Stroke::new(1.5, Color32::WHITE),
                    StrokeKind::Inside,
                );
                let handle = 6.0;
                for corner in [
                    r.left_top(),
                    r.right_top(),
                    r.left_bottom(),
                    r.right_bottom(),
                ] {
                    let hr = Rect::from_center_size(corner, vec2(handle, handle));
                    ui.painter()
                        .rect_filled(hr, CornerRadius::ZERO, Color32::WHITE);
                }
            }

            if app.brush_pts.len() > 1 {
                let pts: Vec<Pos2> = app.brush_pts.iter().map(|p| to_screen(*p)).collect();
                ui.painter().add(Shape::line(
                    pts,
                    Stroke::new(app.brush_width * scale, app.color),
                ));
            }

            // ---- classic right-click context menu --------------------------
            if resp.secondary_clicked()
                && let Some(p) = pointer_img.or(hover_img)
            {
                app.selected = app.session.doc().and_then(|d| d.hit_test(p));
            }
            resp.context_menu(|ui| {
                ui.set_min_width(230.0);
                if menu_row(ui, "Copy Image", "Ctrl+C") {
                    app.copy_to_clipboard();
                }
                if menu_row(ui, "Paste", "Ctrl+V") {
                    app.paste_from_clipboard();
                }
                if let Some(id) = app.selected {
                    ui.separator();
                    if menu_row(ui, "Duplicate", "Ctrl+D") {
                        app.dispatch("edit.duplicate", serde_json::json!({"id": id}));
                    }
                    if menu_row(ui, "Delete", "Del") {
                        app.dispatch("edit.remove", serde_json::json!({"id": id}));
                        app.selected = None;
                    }
                    if menu_row(ui, "Bring to Front", "") {
                        app.dispatch("edit.reorder", serde_json::json!({"id": id, "to": "front"}));
                    }
                    if menu_row(ui, "Send to Back", "") {
                        app.dispatch("edit.reorder", serde_json::json!({"id": id, "to": "back"}));
                    }
                }
                ui.separator();
                ui.add_enabled_ui(app.session.can_undo(), |ui| {
                    if menu_row(ui, "Undo", "Ctrl+Z") {
                        app.dispatch("edit.undo", serde_json::json!({}));
                    }
                });
                ui.add_enabled_ui(app.session.can_redo(), |ui| {
                    if menu_row(ui, "Redo", "Ctrl+Shift+Z") {
                        app.dispatch("edit.redo", serde_json::json!({}));
                    }
                });
                ui.separator();
                if menu_row(ui, "Save As…", "") {
                    app.save_dialog();
                }
                if menu_row(ui, "Export PNG…", "") {
                    app.export_dialog();
                }
            });

            // ---- inline text editing ---------------------------------------
            if let Some(pos) = app.editing.as_ref().map(|e| e.pos) {
                let sp = to_screen(pos);
                let bar_h = 30.0;
                let bar_y = if sp.y - bar_h - 10.0 > ui.clip_rect().top() {
                    sp.y - bar_h - 8.0
                } else {
                    sp.y + app.font_size * 1.5 + 14.0
                };
                let bar_rect = Rect::from_min_size(pos2(sp.x, bar_y), vec2(340.0, bar_h));
                ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                    egui::Frame::new()
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, BORDER))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
                                font_controls(ui, app, "quick-font");
                            });
                        });
                });
            }

            if let Some(edit) = &mut app.editing {
                let sp = to_screen(edit.pos);
                let edit_rect = Rect::from_min_size(sp, vec2(320.0, app.font_size * 1.5 + 10.0));
                let id = ui.scope_id().with("text-edit");
                let resp = ui.put(
                    edit_rect,
                    egui::TextEdit::singleline(&mut edit.buf)
                        .id(id)
                        .font(egui::FontId::proportional(app.font_size))
                        .text_color(app.color)
                        .frame(
                            egui::Frame::new()
                                .fill(Color32::from_white_alpha(235))
                                .stroke(Stroke::new(1.0, ACCENT)),
                        ),
                );
                if !edit.focus_requested {
                    resp.request_focus();
                    edit.focus_requested = true;
                }
                if resp.lost_focus() {
                    app.commit_text();
                }
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

/// Darken `full` everywhere except `hole` (spotlight/crop previews).
fn dim_outside_preview(painter: &egui::Painter, full: Rect, hole: Rect, alpha: f32) {
    let hole = hole.intersect(full);
    let ink = Color32::from_black_alpha((alpha.clamp(0.0, 1.0) * 255.0) as u8);
    let strips = [
        Rect::from_min_max(full.min, pos2(full.max.x, hole.min.y)),
        Rect::from_min_max(pos2(full.min.x, hole.max.y), full.max),
        Rect::from_min_max(pos2(full.min.x, hole.min.y), pos2(hole.min.x, hole.max.y)),
        Rect::from_min_max(pos2(hole.max.x, hole.min.y), pos2(full.max.x, hole.max.y)),
    ];
    for strip in strips {
        if strip.is_positive() {
            painter.rect_filled(strip, CornerRadius::ZERO, ink);
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
