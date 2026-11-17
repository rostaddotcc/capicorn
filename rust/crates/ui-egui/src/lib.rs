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
//! W3b: shell parity — settings panel (persisted through the app's io hook),
//! light/dark themes, dirty-tracking close guard, tray/hotkey/single-instance
//! plumbing and the transparent floating widget viewport.
//! W4d: the live layer — recording (widget Rec + HUD, region/format/audio),
//! scrolling capture, OCR result window, timelapse dialog, watermark export
//! and autosave-on-capture. Everything is an engine command dispatch.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
#[cfg(feature = "native")]
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "native")]
use egui::ViewportBuilder;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Key, Layout, Modifiers, Pos2, Rect, Response,
    Sense, Shape, Stroke, StrokeKind, ViewportCommand, ViewportId, pos2, vec2,
};
use snagcraft_doc::{AnnotObject, BlurMode, ShapeKind};
use snagcraft_engine::Session;
use snagcraft_geom::{Point, Rect as GRect};
#[cfg(feature = "native")]
use snagcraft_platform::{HotkeyEvent, TrayEvent};
use snagcraft_raster::Surface;

pub const ACCENT: Color32 = Color32::from_rgb(0x22, 0x66, 0xb3);
/// Dark-theme accent (`rust-rebuild-parity` §5).
pub const ACCENT_DARK: Color32 = Color32::from_rgb(0x3d, 0x7e, 0xc2);

const PANEL_FILL: Color32 = Color32::from_rgb(243, 244, 246);
const BORDER: Color32 = Color32::from_rgb(209, 213, 219);
const FG: Color32 = Color32::from_rgb(31, 41, 55);
const FG_MUTED: Color32 = Color32::from_rgb(107, 114, 128);
const DARK_PANEL: Color32 = Color32::from_rgb(30, 33, 38);
const DARK_SURFACE: Color32 = Color32::from_rgb(35, 38, 44);
const DARK_BORDER: Color32 = Color32::from_rgb(58, 63, 71);
const DARK_FG: Color32 = Color32::from_rgb(230, 232, 236);

// ---- theme-aware colours (the widgets read these, never a fixed light value) ----

fn pal_panel(ui: &egui::Ui) -> Color32 {
    ui.visuals().panel_fill
}

fn pal_surface(ui: &egui::Ui) -> Color32 {
    ui.visuals().window_fill
}

fn pal_border(ui: &egui::Ui) -> Color32 {
    ui.visuals().widgets.noninteractive.bg_stroke.color
}

fn pal_fg(ui: &egui::Ui) -> Color32 {
    ui.visuals().widgets.noninteractive.fg_stroke.color
}

fn pal_muted(ui: &egui::Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

fn pal_accent(ui: &egui::Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

fn pal_hover(ui: &egui::Ui) -> Color32 {
    ui.visuals().widgets.hovered.bg_fill
}

fn pal_canvas(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(21, 23, 27)
    } else {
        Color32::from_rgb(226, 229, 233)
    }
}

/// UI theme, mirroring `io::settings::Theme` (the shell does not link the io
/// crate — see the W3b manifest exception — so the pair is translated through
/// the settings JSON the engine hands back).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

impl Theme {
    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn from_json(value: Option<&serde_json::Value>) -> Self {
        match value.and_then(|v| v.as_str()) {
            Some("Dark") => Self::Dark,
            _ => Self::Light,
        }
    }
}

/// The settings the panel edits: a faithful view of `io::settings::Settings`
/// (same field names/types) with the theme as [`Theme`]. Every change is
/// written back through `settings.set`, so the engine stays the single owner.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsView {
    pub minimize_to_tray: bool,
    pub start_at_login: bool,
    pub show_widget: bool,
    pub autosave_captures: bool,
    pub capture_delay_secs: u32,
    pub theme: Theme,
    pub default_format: String,
    pub default_quality: u8,
    pub save_dir: Option<String>,
    pub filename_pattern: String,
    pub hotkey_full: String,
    pub hotkey_region: String,
    /// Export watermark text (empty = no watermark).
    pub watermark_text: String,
    /// Watermark ink opacity in percent (2..=100).
    pub watermark_opacity: u8,
    /// `bottom-right` | `bottom-left` | `top-right` | `top-left`.
    pub watermark_corner: String,
    /// Share/upload page opened by `upload.open` (empty = feature off).
    pub upload_url: String,
}

impl Default for SettingsView {
    fn default() -> Self {
        Self {
            minimize_to_tray: false,
            start_at_login: false,
            show_widget: true,
            autosave_captures: true,
            capture_delay_secs: 0,
            theme: Theme::Light,
            default_format: "png".to_string(),
            default_quality: 90,
            save_dir: None,
            filename_pattern: "SnagCraft-{date}-{time}".to_string(),
            hotkey_full: "Print".to_string(),
            hotkey_region: "SHIFT+Print".to_string(),
            watermark_text: String::new(),
            watermark_opacity: WATERMARK_OPACITY_DEFAULT,
            watermark_corner: "bottom-right".to_string(),
            upload_url: String::new(),
        }
    }
}

impl SettingsView {
    /// Tolerant reader: unknown/missing fields keep the documented defaults.
    pub fn from_json(v: &serde_json::Value) -> Self {
        let d = Self::default();
        let b =
            |key: &str, fallback: bool| v.get(key).and_then(|x| x.as_bool()).unwrap_or(fallback);
        let s = |key: &str, fallback: &str| {
            v.get(key)
                .and_then(|x| x.as_str())
                .unwrap_or(fallback)
                .to_string()
        };
        Self {
            minimize_to_tray: b("minimize_to_tray", d.minimize_to_tray),
            start_at_login: b("start_at_login", d.start_at_login),
            show_widget: b("show_widget", d.show_widget),
            autosave_captures: b("autosave_captures", d.autosave_captures),
            capture_delay_secs: v
                .get("capture_delay_secs")
                .and_then(|x| x.as_u64())
                .map_or(d.capture_delay_secs, |x| x.min(3_600) as u32),
            theme: Theme::from_json(v.get("theme")),
            default_format: match v.get("default_format").and_then(|x| x.as_str()) {
                Some(f) if ["png", "jpeg", "webp"].contains(&f) => f.to_string(),
                _ => d.default_format,
            },
            default_quality: v
                .get("default_quality")
                .and_then(|x| x.as_u64())
                .map_or(d.default_quality, |q| q.clamp(1, 100) as u8),
            save_dir: v
                .get("save_dir")
                .and_then(|x| x.as_str())
                .map(str::to_string),
            filename_pattern: s("filename_pattern", &d.filename_pattern),
            hotkey_full: s("hotkey_full", &d.hotkey_full),
            hotkey_region: s("hotkey_region", &d.hotkey_region),
            watermark_text: s("watermark_text", &d.watermark_text),
            watermark_opacity: v.get("watermark_opacity").and_then(|x| x.as_u64()).map_or(
                d.watermark_opacity,
                |o| {
                    o.clamp(
                        u64::from(WATERMARK_OPACITY_MIN),
                        u64::from(WATERMARK_OPACITY_MAX),
                    ) as u8
                },
            ),
            watermark_corner: match v.get("watermark_corner").and_then(|x| x.as_str()) {
                Some(c) if WATERMARK_CORNERS.contains(&c) => c.to_string(),
                _ => d.watermark_corner,
            },
            upload_url: s("upload_url", &d.upload_url),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "minimize_to_tray": self.minimize_to_tray,
            "start_at_login": self.start_at_login,
            "show_widget": self.show_widget,
            "autosave_captures": self.autosave_captures,
            "capture_delay_secs": self.capture_delay_secs,
            "theme": self.theme.name(),
            "default_format": self.default_format,
            "default_quality": self.default_quality,
            "save_dir": self.save_dir,
            "filename_pattern": self.filename_pattern,
            "hotkey_full": self.hotkey_full,
            "hotkey_region": self.hotkey_region,
            // Engine-owned (io::settings), exact key names.
            "watermark_text": self.watermark_text,
            "watermark_opacity": self.watermark_opacity,
            "watermark_corner": self.watermark_corner,
            "upload_url": self.upload_url,
        })
    }
}

/// Everything the tray/hotkey/single-instance layer hands to the shell. The
/// handles are only kept alive here — dropping them detaches the service.
///
/// Desktop only: the web build has no tray, no global hotkeys and no second
/// instance, so the struct does not exist there at all.
#[cfg(feature = "native")]
#[derive(Default)]
pub struct PlatformLink {
    tray_rx: Option<Receiver<TrayEvent>>,
    hotkey_tx: Option<Sender<HotkeyEvent>>,
    hotkey_rx: Option<Receiver<HotkeyEvent>>,
    instance_rx: Option<Receiver<Vec<String>>>,
    _tray: Option<snagcraft_platform::TrayHandle>,
    hotkeys: Option<snagcraft_platform::HotkeyHandle>,
}

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

/// Watermark ink opacity clamp (percent) — exports below 2% are invisible.
pub const WATERMARK_OPACITY_MIN: u8 = 2;
pub const WATERMARK_OPACITY_MAX: u8 = 100;
pub const WATERMARK_OPACITY_DEFAULT: u8 = 45;
/// Watermark size in image pixels (the engine default is 24).
pub const WATERMARK_SIZE: f32 = 24.0;
/// Corner ids `doc.export` accepts, in menu order.
pub const WATERMARK_CORNERS: [&str; 4] = ["bottom-right", "bottom-left", "top-right", "top-left"];
/// Timelapse frame period clamp (seconds) and its default.
pub const TIMELAPSE_PERIOD_MIN: u32 = 1;
pub const TIMELAPSE_PERIOD_MAX: u32 = 300;
pub const TIMELAPSE_PERIOD_DEFAULT: u32 = 30;
/// Hard cap on the optional frame count so a corrupt value cannot run forever.
pub const TIMELAPSE_COUNT_MAX: u32 = 100_000;
/// Upload page used by `upload.open` when the user has not set one.
pub const UPLOAD_URL_DEFAULT: &str = "https://snagcraft.example/upload";

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

/// What a region drag is being collected for (`record.start` / `scroll.start`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickPurpose {
    Record,
    Scroll,
}

impl PickPurpose {
    #[cfg(feature = "native")]
    fn verb(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Scroll => "scrolling capture",
        }
    }
}

/// Frozen-screen region picker: the portal screenshot at 1:1, dragged like the
/// crop frame. Screen pixels == image pixels (`interactive(false)` grabs the
/// whole screen), so the chosen rect is a `region` the engine can use directly.
pub struct RegionPick {
    pub purpose: PickPurpose,
    surface: Option<Surface>,
    tex: Option<egui::TextureHandle>,
    pub rect: Option<GRect>,
}

impl RegionPick {
    fn new(purpose: PickPurpose) -> Self {
        Self {
            purpose,
            surface: None,
            tex: None,
            rect: None,
        }
    }
}

/// Live recording (engine `record.*` mirror + the picker's options).
#[derive(Debug)]
pub struct RecordState {
    /// `record.start` dispatched, no terminal event yet.
    pub armed: bool,
    /// Engine says frames are rolling.
    pub active: bool,
    pub paused: bool,
    /// Set when the `started` event arrives — the HUD's elapsed clock.
    pub started_at: Option<Instant>,
    pub format: String,
    /// PulseAudio/PipeWire source id; `None` records silently.
    pub audio: Option<String>,
    /// (id, name) from `audio.sources`, loaded lazily.
    pub sources: Vec<(String, String)>,
    /// `audio.sources` was queried (an empty list is a valid answer).
    pub sources_loaded: bool,
    pub output: Option<String>,
}

impl Default for RecordState {
    fn default() -> Self {
        Self {
            armed: false,
            active: false,
            paused: false,
            started_at: None,
            format: "mp4".to_string(),
            audio: None,
            sources: Vec::new(),
            sources_loaded: false,
            output: None,
        }
    }
}

/// Scrolling capture (engine `scroll.*` mirror).
#[derive(Default, Debug)]
pub struct ScrollState {
    pub armed: bool,
    pub active: bool,
    pub index: u32,
    pub total_height: u32,
    pub output: Option<String>,
}

/// Timelapse (engine `timelapse.*` mirror + the start dialog's values).
#[derive(Debug)]
pub struct TimelapseState {
    pub armed: bool,
    pub running: bool,
    pub frames: u32,
    pub period_secs: u32,
    pub limit: Option<u32>,
    pub dialog_open: bool,
}

impl Default for TimelapseState {
    fn default() -> Self {
        Self {
            armed: false,
            running: false,
            frames: 0,
            period_secs: TIMELAPSE_PERIOD_DEFAULT,
            limit: None,
            dialog_open: false,
        }
    }
}

/// OCR result window.
#[derive(Default)]
pub struct OcrState {
    pub open: bool,
    pub text: String,
}

pub struct SnagCraftApp {
    /// Shared with the control server (W5a/W5b): one document, two doors.
    pub session: Arc<Mutex<Session>>,
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
    /// Settings mirror (engine is the owner; this is the last applied copy).
    pub settings: SettingsView,
    /// Unsaved-annotation flag for the close guard (cleared by save/export/
    /// open/new/capture, set by any successful `edit.*`).
    pub doc_dirty: bool,
    /// Skip every guard on the way out (tray Quit / modal Discard).
    pub force_quit: bool,
    /// "Unsaved changes" modal visible.
    pub confirm_close: bool,
    pub settings_open: bool,
    /// Floating widget: folded tab vs expanded pill.
    pub widget_expanded: bool,
    /// Tray icon actually registered (hide-to-tray is only safe then).
    pub tray_ok: bool,
    /// Main window hidden (to tray or to the widget): eframe then runs
    /// `App::logic` instead of `App::ui`, so the channels are pumped there.
    hidden: bool,
    applied_theme: Option<Theme>,
    /// Root context, captured each frame so settings changes can re-theme.
    main_ctx: Option<egui::Context>,
    widget_shown: bool,
    widget_applied: Option<bool>,
    #[cfg(feature = "native")]
    platform: PlatformLink,
    /// Set when a settings change must be persisted by the app layer.
    settings_dirty: bool,
    redraw: bool,
    tex: Option<egui::TextureHandle>,
    drag: Option<Drag>,
    drag_cur: Option<Point>,
    /// In-progress crop frame (image coordinates); survives pointer release.
    pub crop: Option<GRect>,
    brush_pts: Vec<Point>,
    editing: Option<TextEdit>,
    pub selected: Option<u64>,
    capture_rx: Option<mpsc::Receiver<Result<Surface, String>>>,
    /// W4d live layer.
    pub rec: RecordState,
    pub scroll: ScrollState,
    pub timelapse: TimelapseState,
    pub ocr: OcrState,
    /// `ocr.available` cached: it probes for the tesseract binary.
    pub ocr_available: Option<bool>,
    pick: Option<RegionPick>,
    pick_rx: Option<mpsc::Receiver<Result<Surface, String>>>,
    /// Web open path: in-flight browser file read (`w`, `h`) or its error.
    #[cfg(not(feature = "native"))]
    drop_rx: Option<mpsc::Receiver<Result<(u32, u32), String>>>,
}

impl SnagCraftApp {
    pub fn new() -> Self {
        Self::with_shared_session(Arc::new(Mutex::new(Session::new())))
    }

    /// Build the app around a session the caller already holds — the desktop
    /// shell spawns the control server on the same `Arc` before the UI starts,
    /// so both doors see one document. Poisoning is recovered, not propagated:
    /// the UI is the only writer and a half-applied command must not brick it.
    pub fn with_shared_session(session: Arc<Mutex<Session>>) -> Self {
        let settings = {
            let mut guard = session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.doc().is_none() {
                let _ = guard.dispatch("doc.new", serde_json::json!({"w": 1000, "h": 640}));
            }
            // The engine loaded the settings file in `Session::new` — mirror it.
            guard
                .dispatch("settings.get", serde_json::json!({}))
                .map(|v| SettingsView::from_json(&v))
                .unwrap_or_default()
        };
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
            settings,
            doc_dirty: false,
            force_quit: false,
            confirm_close: false,
            settings_open: false,
            widget_expanded: false,
            tray_ok: false,
            hidden: false,
            applied_theme: None,
            main_ctx: None,
            widget_shown: false,
            widget_applied: None,
            #[cfg(feature = "native")]
            platform: PlatformLink::default(),
            settings_dirty: false,
            redraw: true,
            tex: None,
            drag: None,
            drag_cur: None,
            crop: None,
            brush_pts: Vec::new(),
            editing: None,
            selected: None,
            capture_rx: None,
            rec: RecordState::default(),
            scroll: ScrollState::default(),
            timelapse: TimelapseState::default(),
            ocr: OcrState::default(),
            ocr_available: None,
            pick: None,
            pick_rx: None,
            #[cfg(not(feature = "native"))]
            drop_rx: None,
        }
    }

    /// Locked session. Poison is recovered (`into_inner`) rather than unwrapped:
    /// a panicking command must not take the whole UI down with it.
    pub fn sess(&self) -> MutexGuard<'_, Session> {
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The shared handle — `spawn_control` serves this exact session.
    pub fn session_arc(&self) -> Arc<Mutex<Session>> {
        Arc::clone(&self.session)
    }

    /// Dispatch through the shared lock and return an owned result, so the
    /// engine mutex is never held while the caller touches `self` again.
    fn exec(&self, cmd: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        self.sess().dispatch(cmd, params).map_err(|e| e.to_string())
    }

    /// Seeded demo scene (selftest + kittest snapshot), built through commands:
    /// one of every object kind the engine renders.
    pub fn seeded() -> Self {
        let mut app = Self::new();
        // Detailed base so blur/spotlight/magnify have visible work to do.
        app.sess().load_surface(synthetic_base(900, 600));
        let shadow = serde_json::json!({
            "enabled": true, "color": [0, 0, 0, 115], "blur": 6,
            "offset_x": 3.0, "offset_y": 4.0});
        let add = |app: &mut Self, obj: serde_json::Value| {
            let _ = app.exec("edit.add", serde_json::json!({"object": obj}));
        };
        // Spotlight first: it dims the base outside its hole, later objects stay bright.
        add(
            &mut app,
            serde_json::json!({"kind": "Spotlight", "id": 0,
                "rect": {"x": 40.0, "y": 40.0, "w": 380.0, "h": 250.0}, "dim": 0.45}),
        );
        let _ = app.exec(
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
        app.redraw = true;
        app.doc_dirty = true;
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
        self.start_capture_delayed(self.settings.capture_delay_secs);
    }

    fn start_capture_delayed(&mut self, delay_secs: u32) {
        #[cfg(not(feature = "native"))]
        {
            let _ = delay_secs;
            self.status =
                "Screen capture is desktop-only — drop an image file here to annotate it.".into();
        }
        #[cfg(feature = "native")]
        {
            if self.capture_rx.is_some() {
                return;
            }
            let (tx, rx) = mpsc::channel();
            self.capture_rx = Some(rx);
            self.status = if delay_secs > 0 {
                format!("Capturing in {delay_secs}s… answer the OS picker if one appears.")
            } else {
                "Capturing… answer the OS picker if one appears.".into()
            };
            // Hostile input: the delay is clamped by SettingsView (<= 3600) and
            // capped again here so a corrupt file cannot park the thread forever.
            let delay = Duration::from_secs(u64::from(delay_secs.min(3_600)));
            std::thread::spawn(move || {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let result = snagcraft_capture::capture_full_blocking().map_err(|e| e.to_string());
                let _ = tx.send(result);
            });
        }
    }

    /// Capture the whole screen (toolbar widget's Full button, tray, hotkey).
    pub fn capture_full(&mut self) {
        self.start_capture();
    }

    /// Capture and switch straight into the crop tool (Region button/hotkey).
    pub fn capture_region(&mut self) {
        self.select_tool(Tool::Crop);
        self.start_capture();
    }

    fn poll_capture(&mut self) {
        let Some(rx) = &self.capture_rx else { return };
        match rx.try_recv() {
            Ok(Ok(surface)) => {
                let (w, h) = (surface.width(), surface.height());
                self.capture_rx = None;
                // Autosave a copy first: the document below is a separate
                // in-memory surface, so this never touches the original file.
                let autosaved = self.autosave_capture(&surface);
                self.sess().load_surface(surface);
                self.redraw = true;
                // A fresh capture starts a clean document.
                self.doc_dirty = false;
                self.selected = None;
                self.status = match autosaved {
                    Some(path) => format!(
                        "Captured {w}×{h} — auto-saved to {} — annotate away.",
                        path.display()
                    ),
                    None => format!("Captured {w}×{h} — annotate away."),
                };
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
        match self.exec(cmd, params) {
            Ok(_) => {
                self.redraw = true;
                // Dirty tracking: edits make the document unsaved, while
                // save/export/open/new all leave it clean.
                if cmd.starts_with("edit.") {
                    self.doc_dirty = true;
                } else if matches!(cmd, "doc.save" | "doc.export" | "doc.open" | "doc.new") {
                    self.doc_dirty = false;
                }
            }
            Err(e) => self.status = format!("{cmd}: {e}"),
        }
    }

    /// Copy the flattened composite to the system clipboard (Ctrl+C / menu).
    #[cfg(feature = "native")]
    fn copy_to_clipboard(&mut self) {
        match self.exec("clipboard.copy", serde_json::json!({})) {
            Ok(v) => {
                self.status = format!(
                    "Copied {}×{} image to clipboard — paste anywhere.",
                    v["width"], v["height"]
                );
            }
            Err(e) => self.status = format!("Copy failed: {e}"),
        }
    }

    /// Web fallback: `clipboard.copy` is a native command (arboard), and the
    /// async Clipboard API needs a user-activation-safe `ClipboardItem` write
    /// that is out of scope for the first web build. Say so instead of failing
    /// silently — Export (PNG download) is the web way to move an image out.
    #[cfg(not(feature = "native"))]
    fn copy_to_clipboard(&mut self) {
        self.status = "Image clipboard is desktop-only — use Export to download a PNG.".into();
    }

    /// Paste an image from the system clipboard (Ctrl+V / menu).
    #[cfg(feature = "native")]
    fn paste_from_clipboard(&mut self) {
        match self.exec("clipboard.paste", serde_json::json!({})) {
            Ok(v) => {
                if v["pasted"].as_bool().unwrap_or(false) {
                    self.redraw = true;
                    self.doc_dirty = true;
                    self.selected = None;
                    self.status = format!("Pasted {}×{} from clipboard.", v["width"], v["height"]);
                } else {
                    self.status = "Clipboard has no image to paste.".into();
                }
            }
            Err(e) => self.status = format!("Paste failed: {e}"),
        }
    }

    /// Web fallback: no image clipboard — drop a file on the canvas instead.
    #[cfg(not(feature = "native"))]
    fn paste_from_clipboard(&mut self) {
        self.status = "Clipboard paste is desktop-only — drop an image file here instead.".into();
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
            .sess()
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

    #[cfg(feature = "native")]
    fn open_dialog(&mut self) -> bool {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        {
            self.dispatch(
                "doc.open",
                serde_json::json!({"path": path.to_string_lossy()}),
            );
            self.status = format!("Opened {}", path.display());
            return true;
        }
        false
    }

    /// Web fallback: the browser's file picker is not reachable from the Rust
    /// side without a DOM `<input type=file>`; eframe already delivers dropped
    /// files, so that is the web open path ([`Self::poll_dropped`]).
    #[cfg(not(feature = "native"))]
    fn open_dialog(&mut self) -> bool {
        self.status = "Open: drop an image file onto the canvas to load it.".into();
        false
    }

    /// Native "save annotations" dialog. Returns true when a file was written
    /// (the close guard needs to know whether closing is now safe).
    #[cfg(feature = "native")]
    pub fn save_dialog(&mut self) -> bool {
        let mut dialog = rfd::FileDialog::new()
            .set_file_name("capture.snagcraft")
            .add_filter("SnagCraft", &["snagcraft"]);
        if let Some(dir) = &self.settings.save_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.dispatch(
                "doc.save",
                serde_json::json!({"path": path.to_string_lossy()}),
            );
            self.status = format!("Saved {}", path.display());
            return true;
        }
        false
    }

    /// Web fallback: `doc.save` writes a real file through std::fs, which the
    /// browser has no access to — the editable format stays desktop-only.
    #[cfg(not(feature = "native"))]
    pub fn save_dialog(&mut self) -> bool {
        self.status = "Editable .snagcraft files are desktop-only — use Export for a PNG.".into();
        false
    }

    /// Export the flattened image using the settings' format/quality.
    #[cfg(feature = "native")]
    pub fn export_dialog(&mut self) -> bool {
        let format = self.settings.default_format.clone();
        let quality = self.settings.default_quality;
        let filter = match format.as_str() {
            "jpeg" => ("JPEG", vec!["jpeg", "jpg"]),
            "webp" => ("WebP", vec!["webp"]),
            _ => ("PNG", vec!["png"]),
        };
        let mut dialog = rfd::FileDialog::new()
            .set_file_name(format!("capture.{format}"))
            .add_filter(filter.0, &filter.1);
        if let Some(dir) = &self.settings.save_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            let mut params = serde_json::json!({"path": path.to_string_lossy(),
                "format": format, "quality": quality});
            if let Some(watermark) = self.watermark_value() {
                params["watermark"] = watermark;
            }
            self.dispatch("doc.export", params);
            self.status = format!("Exported {}", path.display());
            return true;
        }
        false
    }

    /// Web export: `doc.export` writes through `std::fs`, so the browser path
    /// renders the composite in memory, encodes it with `snagcraft-io` (the
    /// codecs are platform-free) and hands the bytes to the browser as a
    /// download via a Blob object URL.
    ///
    /// Known gap: the export watermark is skipped — `doc.export` applies it
    /// with the session's bundled faces, which are private to the engine.
    #[cfg(not(feature = "native"))]
    pub fn export_dialog(&mut self) -> bool {
        let (format, ext, mime) = match self.settings.default_format.as_str() {
            "jpeg" => (snagcraft_io::ImageFormat::Jpeg, "jpg", "image/jpeg"),
            "webp" => (snagcraft_io::ImageFormat::WebP, "webp", "image/webp"),
            _ => (snagcraft_io::ImageFormat::Png, "png", "image/png"),
        };
        let quality = self.settings.default_quality;
        let watermarked = self.watermark_value().is_some();
        let rendered = {
            let guard = self
                .session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.render().map_err(|e| e.to_string())
        };
        let bytes = rendered
            .and_then(|img| snagcraft_io::encode(&img, format, quality).map_err(|e| e.to_string()));
        match bytes {
            Ok(bytes) => {
                let name = format!("snagcraft.{ext}");
                match web::download(&name, mime, &bytes) {
                    Ok(()) => {
                        self.status = if watermarked {
                            format!(
                                "Exported {name} ({} bytes) — watermark is desktop-only.",
                                bytes.len()
                            )
                        } else {
                            format!("Exported {name} ({} bytes).", bytes.len())
                        };
                        true
                    }
                    Err(err) => {
                        self.status = format!("Export failed: {err}");
                        false
                    }
                }
            }
            Err(err) => {
                self.status = format!("Export failed: {err}");
                false
            }
        }
    }

    /// `doc.export`'s watermark block, or `None` when the text is blank.
    pub fn watermark_value(&self) -> Option<serde_json::Value> {
        let text = self.settings.watermark_text.trim();
        if text.is_empty() {
            return None;
        }
        let opacity = f64::from(
            self.settings
                .watermark_opacity
                .clamp(WATERMARK_OPACITY_MIN, WATERMARK_OPACITY_MAX),
        ) / 100.0;
        Some(serde_json::json!({
            "text": text,
            "opacity": opacity,
            "corner": self.settings.watermark_corner,
            "size": WATERMARK_SIZE,
        }))
    }
}

// ------------------------------------------------------- W4d: the live layer

/// Seconds since the Unix epoch — file-name stamps only, no calendar math.
fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// First free `<dir>/<stem>.<ext>`, then `-2`, `-3`, … Never returns an
/// existing path, so an autosave can never overwrite an older capture.
pub fn dedup_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..=9_999u32 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Absurdly crowded folder: fall back to a stamp that cannot collide with
    // the numbered series (still existence-checked by the caller's write).
    dir.join(format!("{stem}-{}.{ext}", epoch_secs()))
}

/// `mm:ss` (or `h:mm:ss` past an hour) for the recording HUD.
pub fn clock_label(secs: u64) -> String {
    let (h, m, s) = (secs / 3_600, (secs % 3_600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

impl SnagCraftApp {
    /// The capture/recording output folder: `settings.save_dir`, else
    /// `$HOME/Pictures/SnagCraft`.
    pub fn save_dir(&self) -> PathBuf {
        if let Some(dir) = self
            .settings
            .save_dir
            .as_ref()
            .filter(|d| !d.trim().is_empty())
        {
            return PathBuf::from(dir);
        }
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Pictures").join("SnagCraft"))
            .unwrap_or_else(|| PathBuf::from("SnagCraft"))
    }

    /// [`Self::save_dir`], created on demand (failure is reported, not fatal).
    fn ensure_save_dir(&mut self) -> PathBuf {
        let dir = self.save_dir();
        if let Err(err) = std::fs::create_dir_all(&dir) {
            self.status = format!("Cannot create {}: {err}", dir.display());
        }
        dir
    }

    // ---- recording ---------------------------------------------------------

    /// True while a take is starting or rolling (the widget's stop square).
    pub fn recording(&self) -> bool {
        self.rec.armed || self.rec.active
    }

    /// `record.start` — whole screen, or the region the picker collected.
    pub fn start_recording(&mut self, region: Option<GRect>) {
        if self.recording() {
            return;
        }
        let dir = self.ensure_save_dir();
        let ext = if self.rec.format == "gif" {
            "gif"
        } else {
            "mp4"
        };
        let path = dir.join(format!("snagcraft-rec-{}.{ext}", epoch_secs()));
        let mut params = serde_json::json!({
            "format": ext,
            "output": path.to_string_lossy(),
        });
        if let Some(source) = self.rec.audio.clone() {
            params["audio_source"] = serde_json::json!(source);
        }
        if let Some(rect) = region {
            params["region"] = rect_json(rect);
        }
        match self.exec("record.start", params) {
            Ok(_) => {
                self.rec.armed = true;
                self.rec.active = false;
                self.rec.paused = false;
                self.rec.started_at = None;
                self.rec.output = Some(path.display().to_string());
                self.status = format!("Recording to {}…", path.display());
                self.redraw = true;
            }
            Err(err) => self.status = format!("Record: {err}"),
        }
    }

    /// `record.stop` — the encoder finalises and reports `stopped`.
    pub fn stop_recording(&mut self) {
        match self.exec("record.stop", serde_json::json!({})) {
            Ok(_) => self.status = "Stopping the recording…".into(),
            Err(err) => self.status = format!("Record stop: {err}"),
        }
    }

    /// `audio.sources` for the picker (once per session — enumerating devices
    /// spawns pactl, so it is not a per-frame call).
    pub fn refresh_audio_sources(&mut self) {
        if let Ok(v) = self.exec("audio.sources", serde_json::json!({})) {
            self.rec.sources = v["sources"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|s| {
                            Some((
                                s["id"].as_str()?.to_string(),
                                s["name"].as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        self.rec.sources_loaded = true;
    }

    /// Pause/resume toggle (HUD + widget).
    pub fn toggle_record_pause(&mut self) {
        let cmd = if self.rec.paused {
            "record.resume"
        } else {
            "record.pause"
        };
        match self.exec(cmd, serde_json::json!({})) {
            Ok(_) => {}
            Err(err) => self.status = format!("Record pause: {err}"),
        }
    }

    /// Clear the "a take is in flight" flags, keeping the chosen options.
    fn end_recording(&mut self) {
        self.rec.armed = false;
        self.rec.active = false;
        self.rec.paused = false;
        self.rec.started_at = None;
    }

    /// Elapsed time of the running take (from the `started` event).
    pub fn record_elapsed(&self) -> Duration {
        self.rec
            .started_at
            .map_or(Duration::ZERO, |start| start.elapsed())
    }

    /// HUD clock: `00:12`.
    pub fn record_clock(&self) -> String {
        clock_label(self.record_elapsed().as_secs())
    }

    // ---- region picker (record / scrolling capture) -------------------------

    /// Freeze the screen and let the user drag a rect — same idiom as the crop
    /// frame, but the pixels are screen pixels, which is what the engine wants.
    pub fn start_region_pick(&mut self, purpose: PickPurpose) {
        if self.pick.is_some() || self.pick_rx.is_some() || self.capture_rx.is_some() {
            return;
        }
        self.pick = Some(RegionPick::new(purpose));
        #[cfg(feature = "native")]
        {
            let (tx, rx) = mpsc::channel();
            self.pick_rx = Some(rx);
            self.status = format!("Capturing the screen for the {} region…", purpose.verb());
            std::thread::spawn(move || {
                let _ =
                    tx.send(snagcraft_capture::capture_full_blocking().map_err(|e| e.to_string()));
            });
        }
        #[cfg(not(feature = "native"))]
        {
            // The screen-freeze half of the picker is a desktop service; keep
            // the picker up so the crop-style rect can still be dragged on the
            // loaded document instead of pretending a screen grab is coming.
            self.pick_rx = None;
            self.status =
                "Screen preview is desktop-only — drag the region on the current image.".into();
        }
    }

    /// Test/seed hook: start a picker with the screen image already in hand.
    pub fn seed_region_pick(&mut self, purpose: PickPurpose, surface: Surface) {
        let mut pick = RegionPick::new(purpose);
        pick.surface = Some(surface);
        self.pick = Some(pick);
        self.pick_rx = None;
    }

    /// The purpose of the picker that is currently up, if any.
    pub fn region_pick(&self) -> Option<PickPurpose> {
        self.pick.as_ref().map(|p| p.purpose)
    }

    fn poll_pick(&mut self) {
        let Some(rx) = &self.pick_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(surface)) => {
                self.pick_rx = None;
                if let Some(pick) = &mut self.pick {
                    pick.surface = Some(surface);
                }
                self.status = "Drag a rectangle over the frozen screen, then press Enter.".into();
            }
            Ok(Err(err)) => {
                self.pick_rx = None;
                self.pick = None;
                self.status = format!("Region pick failed: {err}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pick_rx = None;
                self.pick = None;
                self.status = "Region capture thread ended unexpectedly.".into();
            }
        }
    }

    /// Enter on the picker: hand the rect to the engine. A too-small frame
    /// keeps the picker up so the user can simply drag again.
    pub fn confirm_region_pick(&mut self) {
        let Some(rect) = self.pick.as_ref().and_then(|pick| pick.rect) else {
            self.status = "Drag a rectangle first, then press Enter.".into();
            return;
        };
        if rect.w < 8.0 || rect.h < 8.0 {
            self.status = "That region is too small — drag a bigger one.".into();
            return;
        }
        let Some(pick) = self.pick.take() else {
            return;
        };
        self.pick_rx = None;
        match pick.purpose {
            PickPurpose::Record => self.start_recording(Some(rect)),
            PickPurpose::Scroll => self.start_scrolling(rect),
        }
    }

    pub fn cancel_region_pick(&mut self) {
        if self.pick.take().is_some() {
            self.pick_rx = None;
            self.status = "Region pick cancelled.".into();
        }
    }

    // ---- scrolling capture --------------------------------------------------

    /// `scroll.start` for the picked rect; the HUD guides the user to scroll.
    pub fn start_scrolling(&mut self, region: GRect) {
        if self.scroll.armed {
            return;
        }
        let dir = self.ensure_save_dir();
        let path = dir.join(format!("snagcraft-scroll-{}.png", epoch_secs()));
        let params = serde_json::json!({
            "region": rect_json(region),
            "output": path.to_string_lossy(),
        });
        match self.exec("scroll.start", params) {
            Ok(_) => {
                self.scroll = ScrollState {
                    armed: true,
                    active: false,
                    index: 0,
                    total_height: 0,
                    output: Some(path.display().to_string()),
                };
                self.status = "Scrolling capture — scroll slowly, Enter or Stop ends it.".into();
                self.redraw = true;
            }
            Err(err) => self.status = format!("Scrolling capture: {err}"),
        }
    }

    /// Enter / Stop: stitch what was captured and open it in the editor.
    pub fn stop_scrolling(&mut self) {
        match self.exec("scroll.stop", serde_json::json!({})) {
            Ok(_) => self.status = "Finishing the scrolling capture…".into(),
            Err(err) => self.status = format!("Scroll stop: {err}"),
        }
    }

    // ---- timelapse ----------------------------------------------------------

    /// `timelapse.start` with the dialog's period/count into the save folder.
    pub fn start_timelapse(&mut self) {
        if self.timelapse.armed {
            return;
        }
        let dir = self.ensure_save_dir();
        let period = self
            .timelapse
            .period_secs
            .clamp(TIMELAPSE_PERIOD_MIN, TIMELAPSE_PERIOD_MAX);
        let mut params = serde_json::json!({
            "period_secs": period,
            "dir": dir.to_string_lossy(),
        });
        if let Some(count) = self
            .timelapse
            .limit
            .map(|c| c.clamp(1, TIMELAPSE_COUNT_MAX))
        {
            params["count"] = serde_json::json!(count);
        }
        match self.exec("timelapse.start", params) {
            Ok(_) => {
                self.timelapse.armed = true;
                self.timelapse.running = true;
                self.timelapse.frames = 0;
                self.timelapse.dialog_open = false;
                self.status = format!("Timelapse every {period}s into {}.", dir.display());
                self.redraw = true;
            }
            Err(err) => self.status = format!("Timelapse: {err}"),
        }
    }

    pub fn stop_timelapse(&mut self) {
        match self.exec("timelapse.stop", serde_json::json!({})) {
            Ok(_) => self.status = "Timelapse stopping…".into(),
            Err(err) => self.status = format!("Timelapse stop: {err}"),
        }
    }

    // ---- OCR / upload -------------------------------------------------------

    /// `ocr.run` on the current document, shown in the result window.
    pub fn run_ocr(&mut self) {
        match self.exec("ocr.run", serde_json::json!({"lang": "eng"})) {
            Ok(v) => {
                self.ocr.text = v["text"].as_str().unwrap_or_default().to_string();
                self.ocr.open = true;
                self.status = format!(
                    "OCR extracted {} characters.",
                    self.ocr.text.chars().count()
                );
                self.redraw = true;
            }
            Err(err) => self.status = format!("OCR: {err}"),
        }
    }

    /// Cached `ocr.available` (probing spawns tesseract, so once per session).
    pub fn ocr_ready(&mut self) -> bool {
        if let Some(available) = self.ocr_available {
            return available;
        }
        let available = self
            .exec("ocr.available", serde_json::json!({}))
            .ok()
            .and_then(|v| v["available"].as_bool())
            .unwrap_or(false);
        self.ocr_available = Some(available);
        available
    }

    /// The configured share page; empty disables the action.
    pub fn upload_target(&self) -> Option<String> {
        let url = self.settings.upload_url.trim();
        (!url.is_empty()).then(|| url.to_string())
    }

    /// `upload.open` for the configured share page.
    pub fn open_upload(&mut self) {
        let Some(url) = self.upload_target() else {
            self.status =
                format!("Set an upload URL in Settings — Export (e.g. {UPLOAD_URL_DEFAULT}).");
            return;
        };
        match self.exec("upload.open", serde_json::json!({"url": url})) {
            Ok(_) => self.status = format!("Opened {url}"),
            Err(err) => self.status = format!("Upload page: {err}"),
        }
    }

    // ---- autosave -----------------------------------------------------------

    /// W3 gap: a fresh capture is written to the save folder when the setting is
    /// on. The document keeps its own copy — the file is a *copy*, never a move.
    fn autosave_capture(&mut self, surface: &Surface) -> Option<PathBuf> {
        if !self.settings.autosave_captures {
            return None;
        }
        let dir = self.ensure_save_dir();
        let stem = format!("snagcraft-{}", epoch_secs());
        let path = dedup_path(&dir, &stem, "png");
        match image::save_buffer(
            &path,
            surface.as_raw(),
            surface.width(),
            surface.height(),
            image::ExtendedColorType::Rgba8,
        ) {
            Ok(()) => Some(path),
            Err(err) => {
                self.status = format!("Auto-save failed: {err}");
                None
            }
        }
    }

    // ---- heartbeat ----------------------------------------------------------

    /// Drain the recording/scroll/timelapse channels. Runs from `app_ui` *and*
    /// `logic` — eframe skips `ui` while the window is hidden, and a recording
    /// started from the widget must still report its `stopped` event.
    fn poll_media(&mut self) {
        self.poll_pick();
        if self.rec.armed {
            self.poll_record();
        }
        if self.scroll.armed {
            self.poll_scroll();
        }
        if self.timelapse.armed {
            self.poll_timelapse();
        }
    }

    fn poll_record(&mut self) {
        match self.exec("record.poll", serde_json::json!({})) {
            Ok(v) => {
                let events = v["events"].as_array().cloned().unwrap_or_default();
                for event in events {
                    match event["type"].as_str() {
                        Some("started") => {
                            self.rec.active = true;
                            self.rec.armed = true;
                            self.rec.started_at.get_or_insert_with(Instant::now);
                            self.status = "Recording — the HUD's Stop ends it.".into();
                        }
                        Some("stopped") => {
                            let path = event["path"].as_str().unwrap_or_default().to_string();
                            self.end_recording();
                            self.status = format!("Recording saved: {path}");
                        }
                        Some("failed") => {
                            let msg = event["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            self.end_recording();
                            self.status = format!("Recording failed: {msg}");
                        }
                        Some("paused") => self.rec.paused = true,
                        Some("resumed") => self.rec.paused = false,
                        _ => {}
                    }
                }
                if !v["running"].as_bool().unwrap_or(true) && self.rec.armed {
                    self.end_recording();
                }
            }
            Err(err) => {
                self.end_recording();
                self.status = format!("Record poll: {err}");
            }
        }
        if self.rec.armed
            && let Ok(v) = self.exec("record.state", serde_json::json!({}))
        {
            if let Some(active) = v["active"].as_bool() {
                self.rec.active = active;
            }
            if let Some(paused) = v["paused"].as_bool() {
                self.rec.paused = paused;
            }
        }
    }

    fn poll_scroll(&mut self) {
        match self.exec("scroll.poll", serde_json::json!({})) {
            Ok(v) => {
                let events = v["events"].as_array().cloned().unwrap_or_default();
                for event in events {
                    match event["type"].as_str() {
                        Some("started") => {
                            self.scroll.active = true;
                            self.status =
                                "Scrolling capture — scroll slowly, Enter or Stop ends it.".into();
                        }
                        Some("captured") => {
                            self.scroll.active = true;
                            self.scroll.index = event["index"].as_u64().unwrap_or(0) as u32;
                            self.scroll.total_height =
                                event["total_height"].as_u64().unwrap_or(0) as u32;
                        }
                        Some("done") => {
                            let path = event["path"].as_str().unwrap_or_default().to_string();
                            self.scroll.armed = false;
                            self.scroll.active = false;
                            if path.is_empty() {
                                self.status = "Scrolling capture finished with no image.".into();
                            } else {
                                self.dispatch("doc.open", serde_json::json!({"path": path}));
                                self.status = format!("Scrolling capture saved: {path}");
                                self.selected = None;
                            }
                        }
                        Some("failed") => {
                            let msg = event["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            self.scroll.armed = false;
                            self.scroll.active = false;
                            self.status = format!("Scrolling capture failed: {msg}");
                        }
                        Some("stopped") => {
                            self.scroll.armed = false;
                            self.scroll.active = false;
                            self.status = "Scrolling capture stopped.".into();
                        }
                        _ => {}
                    }
                }
                if !v["running"].as_bool().unwrap_or(true) && self.scroll.armed {
                    self.scroll.armed = false;
                    self.scroll.active = false;
                }
            }
            Err(err) => {
                self.scroll.armed = false;
                self.scroll.active = false;
                self.status = format!("Scroll poll: {err}");
            }
        }
    }

    fn poll_timelapse(&mut self) {
        match self.exec("timelapse.poll", serde_json::json!({})) {
            Ok(v) => {
                let events = v["events"].as_array().cloned().unwrap_or_default();
                for event in events {
                    match event["type"].as_str() {
                        Some("frame") => {
                            self.timelapse.running = true;
                            self.timelapse.frames = self.timelapse.frames.saturating_add(1);
                        }
                        Some("done") => {
                            let frames = event["frames"].as_u64().unwrap_or(0);
                            self.timelapse.armed = false;
                            self.timelapse.running = false;
                            self.timelapse.frames = frames as u32;
                            self.status = format!("Timelapse finished: {frames} frames.");
                        }
                        Some("failed") => {
                            let msg = event["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            self.timelapse.armed = false;
                            self.timelapse.running = false;
                            self.status = format!("Timelapse failed: {msg}");
                        }
                        _ => {}
                    }
                }
                if !v["running"].as_bool().unwrap_or(true) && self.timelapse.armed {
                    self.timelapse.armed = false;
                    self.timelapse.running = false;
                }
            }
            Err(err) => {
                self.timelapse.armed = false;
                self.timelapse.running = false;
                self.status = format!("Timelapse poll: {err}");
            }
        }
    }

    // ---- windows ------------------------------------------------------------

    /// OCR result: selectable text plus one-click copy (the document is never
    /// touched — this is a read of the composite).
    fn ocr_window(&mut self, ctx: &egui::Context) {
        if !self.ocr.open {
            return;
        }
        let mut open = true;
        let mut copy = false;
        let mut close = false;
        egui::Window::new("Extracted text")
            .open(&mut open)
            .resizable(true)
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "Read from the current image — the document is untouched.",
                        )
                        .size(11.0)
                        .weak(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} characters",
                                self.ocr.text.chars().count()
                            ))
                            .size(11.0)
                            .color(pal_muted(ui)),
                        );
                    });
                });
                ui.add_space(4.0);
                let mut buf = self.ocr.text.clone();
                ui.add(
                    egui::TextEdit::multiline(&mut buf)
                        .desired_rows(12)
                        .desired_width(f32::INFINITY)
                        .code_editor(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Copy text").clicked() {
                        copy = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if copy {
            ctx.copy_text(self.ocr.text.clone());
            self.status = "Extracted text copied to the clipboard.".into();
        }
        if close || !open {
            self.ocr.open = false;
        }
    }

    /// Timelapse start dialog: period, optional frame limit, target folder.
    fn timelapse_dialog(&mut self, ctx: &egui::Context) {
        if !self.timelapse.dialog_open {
            return;
        }
        let dir = self.save_dir();
        let mut open = true;
        let mut start = false;
        let mut cancel = false;
        egui::Window::new("Timelapse")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Grab a frame on a timer — good for slow changes.")
                        .size(11.5)
                        .weak(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Every");
                    ui.add(
                        egui::Slider::new(
                            &mut self.timelapse.period_secs,
                            TIMELAPSE_PERIOD_MIN..=TIMELAPSE_PERIOD_MAX,
                        )
                        .suffix(" s")
                        .integer()
                        .trailing_fill(true),
                    );
                });
                ui.horizontal(|ui| {
                    let mut limited = self.timelapse.limit.is_some();
                    if ui.checkbox(&mut limited, "Stop after").changed() {
                        self.timelapse.limit = limited.then_some(60);
                    }
                    if let Some(limit) = self.timelapse.limit.as_mut() {
                        ui.add(
                            egui::Slider::new(limit, 1..=TIMELAPSE_COUNT_MAX)
                                .integer()
                                .suffix(" frames"),
                        );
                    }
                });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!("Folder: {}", dir.display()))
                        .size(11.0)
                        .monospace()
                        .weak(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Start").clicked() {
                        start = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if start {
            self.start_timelapse();
        }
        if cancel || !open {
            self.timelapse.dialog_open = false;
        }
    }
}

impl Default for SnagCraftApp {
    fn default() -> Self {
        Self::new()
    }
}

// --------------------------------------------------------- W3b: shell plumbing

/// Widget geometry (`windows.rs:205-213`): the outer window, then the folded tab.
pub const WIDGET_EXPANDED_SIZE: (f32, f32) = (304.0, 60.0);
pub const WIDGET_FOLDED_SIZE: (f32, f32) = (58.0, 74.0);
/// Live red dot on the widget.
const WIDGET_RED: Color32 = Color32::from_rgb(214, 40, 40);
/// Stop control fill (live widget `#e5484d`).
const WIDGET_STOP: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
const WIDGET_BG: Color32 = Color32::from_rgba_premultiplied(26, 29, 35, 242);

/// The floating widget's own viewport (immediate viewport, one per frame).
pub fn widget_viewport_id() -> ViewportId {
    ViewportId::from_hash_of("snagcraft-widget")
}

/// What the settings panel asks for beyond "the value changed".
#[derive(Default)]
struct SettingsActions {
    changed: bool,
    pick_dir: bool,
    rebind: bool,
    open_upload: bool,
}

impl SnagCraftApp {
    // ---- platform wiring ----------------------------------------------------
    //
    // Every entry point here is a desktop service (StatusNotifier tray, the
    // XDG global-shortcuts portal, single-instance forwarding). The web build
    // has none of them, so the whole block is native-only.

    /// Hand the live tray over (the handle keeps the icon alive).
    #[cfg(feature = "native")]
    pub fn attach_tray(&mut self, handle: snagcraft_platform::TrayHandle, rx: Receiver<TrayEvent>) {
        self.tray_ok = handle.is_available();
        if !self.tray_ok {
            self.status = "No tray host — close-to-tray stays off.".into();
        }
        self.platform._tray = Some(handle);
        self.attach_tray_events(rx);
    }

    /// Drain-only variant of [`Self::attach_tray`] (no live handle needed —
    /// the channels are what the shell actually consumes).
    #[cfg(feature = "native")]
    pub fn attach_tray_events(&mut self, rx: Receiver<TrayEvent>) {
        self.platform.tray_rx = Some(rx);
    }

    /// Record why the tray never came up (headless / locked session).
    #[cfg(feature = "native")]
    pub fn tray_failed(&mut self, error: &snagcraft_platform::PlatformError) {
        self.tray_ok = false;
        self.status = format!("Tray unavailable ({error}) — close-to-tray stays off.");
    }

    /// Second-launch argv stream from `acquire_or_forward`.
    #[cfg(feature = "native")]
    pub fn attach_instance(&mut self, rx: Receiver<Vec<String>>) {
        self.platform.instance_rx = Some(rx);
    }

    /// Hotkey portal session: keep the handle, drain the events channel.
    #[cfg(feature = "native")]
    pub fn attach_hotkeys(
        &mut self,
        tx: Sender<HotkeyEvent>,
        rx: Receiver<HotkeyEvent>,
        handle: Result<snagcraft_platform::HotkeyHandle, snagcraft_platform::PlatformError>,
    ) {
        self.platform.hotkey_tx = Some(tx);
        self.platform.hotkey_rx = Some(rx);
        match handle {
            Ok(handle) => self.platform.hotkeys = Some(handle),
            Err(err) => self.status = format!("Global hotkeys unavailable: {err}"),
        }
    }

    /// True exactly once after a settings change: the app layer answers it
    /// with `io::settings::save` (this crate stays free of the io dependency).
    pub fn take_settings_dirty(&mut self) -> bool {
        std::mem::take(&mut self.settings_dirty)
    }

    /// Re-bind the global hotkeys after the trigger strings changed. The portal
    /// allows one `BindShortcuts` per session, so the old handle is dropped.
    #[cfg(feature = "native")]
    pub fn rebind_hotkeys(&mut self) {
        let Some(tx) = self.platform.hotkey_tx.clone() else {
            self.status = "No hotkey portal — bindings unchanged.".into();
            return;
        };
        let defs = vec![
            snagcraft_platform::HotkeyDef::new("capture-full", self.settings.hotkey_full.clone()),
            snagcraft_platform::HotkeyDef::new(
                "capture-region",
                self.settings.hotkey_region.clone(),
            ),
        ];
        self.platform.hotkeys = None;
        match snagcraft_platform::spawn_hotkeys(defs, tx) {
            Ok(handle) => {
                self.platform.hotkeys = Some(handle);
                self.status = "Rebinding global hotkeys — approve the dialog if asked.".into();
            }
            Err(err) => self.status = format!("Hotkeys: {err}"),
        }
    }

    /// Drain the tray/hotkey/second-instance channels (desktop only).
    #[cfg(feature = "native")]
    fn poll_platform(&mut self, ctx: &egui::Context) {
        let tray_events: Vec<TrayEvent> = self
            .platform
            .tray_rx
            .as_ref()
            .map(|rx| std::iter::from_fn(|| rx.try_recv().ok()).take(32).collect())
            .unwrap_or_default();
        for event in tray_events {
            match event {
                TrayEvent::Show => {
                    self.hidden = false;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                    ctx.request_repaint();
                }
                TrayEvent::Capture => self.capture_full(),
                TrayEvent::ToggleWidget => self.toggle_widget(ctx),
                TrayEvent::Quit => {
                    self.force_quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        let hotkey_events: Vec<HotkeyEvent> = self
            .platform
            .hotkey_rx
            .as_ref()
            .map(|rx| std::iter::from_fn(|| rx.try_recv().ok()).take(32).collect())
            .unwrap_or_default();
        for event in hotkey_events {
            match event {
                HotkeyEvent::Activated { id } => match id.as_str() {
                    "capture-region" => self.capture_region(),
                    "capture-full" => self.capture_full(),
                    other => self.status = format!("Unknown hotkey: {other}"),
                },
                HotkeyEvent::Bound { granted } => {
                    let names: Vec<String> = granted
                        .iter()
                        .map(|(id, trigger)| format!("{id} ({trigger})"))
                        .collect();
                    self.status = if names.is_empty() {
                        "No global hotkeys were granted — tray and toolbar still work.".into()
                    } else {
                        format!("Hotkeys bound: {}", names.join(", "))
                    };
                }
                HotkeyEvent::Failed(msg) => {
                    self.status = format!("Hotkeys unavailable: {msg}");
                }
            }
        }

        let launches: Vec<Vec<String>> = self
            .platform
            .instance_rx
            .as_ref()
            .map(|rx| std::iter::from_fn(|| rx.try_recv().ok()).take(8).collect())
            .unwrap_or_default();
        for argv in launches {
            self.handle_launch_args(&argv);
        }
    }

    /// Web: there is no tray, no global-hotkey portal and no second instance,
    /// so there is nothing to drain.
    #[cfg(not(feature = "native"))]
    fn poll_platform(&mut self, _ctx: &egui::Context) {}

    /// Web open path: eframe's browser backend turns a dropped file into an
    /// `egui::DroppedFile`, whose bytes arrive asynchronously. Read them in a
    /// `wasm-bindgen-futures` task, then decode through `snagcraft-io` (the
    /// codecs are platform-free) and load the surface into the shared session.
    #[cfg(not(feature = "native"))]
    fn poll_dropped(&mut self, ctx: &egui::Context) {
        // Drain any finished read first: the future cannot touch `self`.
        let finished: Vec<Result<(u32, u32), String>> = self
            .drop_rx
            .as_ref()
            .map(|rx| std::iter::from_fn(|| rx.try_recv().ok()).take(4).collect())
            .unwrap_or_default();
        if !finished.is_empty() {
            self.drop_rx = None;
            for result in finished {
                match result {
                    Ok((w, h)) => {
                        self.redraw = true;
                        self.doc_dirty = false;
                        self.selected = None;
                        self.status =
                            format!("Opened {w}×{h} from the dropped file — annotate away.");
                    }
                    Err(err) => self.status = format!("Open failed: {err}"),
                }
            }
        }
        if self.drop_rx.is_some() {
            return;
        }
        let files = ctx.input(|i| i.raw.dropped_files.clone());
        let Some(file) = files.into_iter().next() else {
            return;
        };
        let name = file.path().display().to_string();
        let label = name.clone();
        let (tx, rx) = mpsc::channel();
        self.drop_rx = Some(rx);
        let session = Arc::clone(&self.session);
        let repaint = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match file.bytes_async().await {
                Ok(bytes) => load_image_bytes(&session, &bytes),
                Err(err) => Err(err),
            };
            let _ = tx.send(result.map_err(|err| format!("{label}: {err}")));
            repaint.request_repaint();
        });
        self.status = format!("Reading {name}…");
    }

    /// CLI / forwarded argv: `--capture`, `--region`, and image paths.
    pub fn handle_launch_args(&mut self, args: &[String]) {
        for arg in args {
            match arg.as_str() {
                "--capture" => self.capture_full(),
                "--region" => self.capture_region(),
                other if other.starts_with('-') => {}
                other => {
                    let lower = other.to_ascii_lowercase();
                    let known = [
                        ".png",
                        ".jpg",
                        ".jpeg",
                        ".webp",
                        ".gif",
                        ".bmp",
                        ".snagcraft",
                    ];
                    if known.iter().any(|ext| lower.ends_with(ext)) {
                        self.dispatch("doc.open", serde_json::json!({"path": other}));
                        self.status = format!("Opened {other}");
                    }
                }
            }
        }
    }

    // ---- settings -----------------------------------------------------------

    /// Apply a settings change: engine first (it owns the values), then the
    /// persistence hook, then the live effects.
    pub fn push_settings(&mut self, next: SettingsView) {
        let payload = next.to_json();
        match self.exec("settings.set", serde_json::json!({"settings": payload})) {
            Ok(applied) => {
                // The engine owns every field, watermark/upload included, so
                // the answer is the truth — no local mirror to reconcile.
                self.settings = SettingsView::from_json(&applied);
                self.settings_dirty = true;
                self.apply_settings();
                self.redraw = true;
                self.status = "Settings saved.".into();
            }
            Err(err) => self.status = format!("settings.set: {err}"),
        }
    }

    /// Live effects of the current settings (theme now; visibility is read
    /// every frame by the widget/close-guard code).
    fn apply_settings(&mut self) {
        if self.applied_theme == Some(self.settings.theme) {
            return;
        }
        self.applied_theme = Some(self.settings.theme);
        if let Some(ctx) = self.main_ctx.clone() {
            apply_theme(&ctx, self.settings.theme);
        }
        self.redraw = true;
    }

    /// Tray "Toggle widget" entry (desktop only — the widget itself is a
    /// second viewport).
    #[cfg(feature = "native")]
    fn toggle_widget(&mut self, _ctx: &egui::Context) {
        let mut next = self.settings.clone();
        next.show_widget = !next.show_widget;
        self.push_settings(next);
    }

    // ---- close guard --------------------------------------------------------

    /// eframe 0.37: the X button arrives as `ViewportEvent::Close`, and the
    /// window only closes if we do *not* answer with `CancelClose`.
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        let close = ctx.input(|i| i.viewport().events.contains(&egui::ViewportEvent::Close));
        if !close {
            return;
        }
        // Tray Quit / modal Discard already decided — let eframe close.
        if self.force_quit {
            return;
        }
        if self.doc_dirty {
            self.confirm_close = true;
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            return;
        }
        let hide_to_widget = self.widget_shown && self.settings.show_widget;
        let hide_to_tray = self.settings.minimize_to_tray && self.tray_ok;
        if hide_to_widget || hide_to_tray {
            self.hide_main(ctx);
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
    }

    /// Hide the main window but keep the app alive. eframe stops calling
    /// `App::ui` for a hidden root, so [`Self::logic`] takes over the pumping.
    fn hide_main(&mut self, ctx: &egui::Context) {
        self.hidden = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        // One full pass while hidden, so the widget viewport (when it is the
        // thing keeping the app reachable) is still requested.
        ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    /// eframe runs this instead of [`app_ui`] while the window is hidden: no
    /// painting, but the tray/hotkey/instance channels must stay drained or the
    /// window could never be brought back.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if self.hidden {
            // Slow heartbeat: eframe otherwise only wakes for OS events, and a
            // hidden window has none. A quarter second keeps the tray snappy
            // without burning a frame loop.
            ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.main_ctx = Some(ctx.clone());
        self.poll_platform(ctx);
        // The live layer keeps running while hidden: a recording start/stop or
        // a scrolling capture must land even with no main window painting.
        self.poll_media();
        if self.recording() || self.scroll.armed || self.timelapse.armed {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.handle_close_request(ctx);
    }

    fn close_modal(&mut self, ctx: &egui::Context) {
        if !self.confirm_close {
            return;
        }
        let mut action = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    picture_icon(ui);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("You have unsaved annotations").strong());
                        ui.label(
                            egui::RichText::new("Closing now throws them away.")
                                .weak()
                                .size(11.5),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                action = Some(0u8);
                            }
                            if ui.button("Discard").clicked() {
                                action = Some(1);
                            }
                            if ui.button("Cancel").clicked() {
                                action = Some(2);
                            }
                        });
                    });
                });
            });
        match action {
            // Save: only close once the file really landed.
            Some(0) => {
                if self.save_dialog() {
                    self.doc_dirty = false;
                    self.force_quit = true;
                    self.confirm_close = false;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Some(1) => {
                self.force_quit = true;
                self.confirm_close = false;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Some(2) => self.confirm_close = false,
            _ => {}
        }
    }

    // ---- settings panel -----------------------------------------------------

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = true;
        let mut next = self.settings.clone();
        let mut acts = SettingsActions::default();
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| settings_ui(ui, &mut next, &mut acts));
        if !open {
            self.settings_open = false;
        }
        if acts.pick_dir {
            #[cfg(feature = "native")]
            {
                let mut dialog = rfd::FileDialog::new();
                if let Some(current) = &next.save_dir {
                    dialog = dialog.set_directory(current);
                }
                if let Some(dir) = dialog.pick_folder() {
                    next.save_dir = Some(dir.to_string_lossy().to_string());
                    acts.changed = true;
                }
            }
            #[cfg(not(feature = "native"))]
            {
                self.status = "Folder pickers are desktop-only — the web build downloads.".into();
                acts.changed = false;
            }
        }
        if acts.changed {
            // Login autostart is a real desktop entry, not just a flag: apply
            // it first and roll the checkbox back if the write failed.
            #[cfg(feature = "native")]
            {
                if next.start_at_login != self.settings.start_at_login {
                    match snagcraft_platform::set_autostart(next.start_at_login) {
                        Ok(()) => {
                            self.status = if next.start_at_login {
                                "SnagCraft will start at login.".into()
                            } else {
                                "SnagCraft will no longer start at login.".into()
                            };
                        }
                        Err(err) => {
                            self.status = format!("Autostart failed: {err}");
                            next.start_at_login = self.settings.start_at_login;
                        }
                    }
                }
            }
            self.push_settings(next);
        }
        if acts.rebind {
            #[cfg(feature = "native")]
            {
                self.rebind_hotkeys();
            }
            #[cfg(not(feature = "native"))]
            {
                self.status = "Global hotkeys are desktop-only.".into();
            }
        }
        if acts.open_upload {
            #[cfg(feature = "native")]
            {
                self.open_upload();
            }
            #[cfg(not(feature = "native"))]
            {
                self.status = "Uploading is desktop-only in this build.".into();
            }
        }
    }

    // ---- floating widget ----------------------------------------------------

    #[cfg(feature = "native")]
    fn floating_widget(&mut self, ctx: &egui::Context) {
        if !self.settings.show_widget {
            if self.widget_shown {
                ctx.send_viewport_cmd_to(widget_viewport_id(), ViewportCommand::Close);
                self.widget_shown = false;
            }
            return;
        }
        let (w, h) = if self.widget_expanded {
            WIDGET_EXPANDED_SIZE
        } else {
            WIDGET_FOLDED_SIZE
        };
        let builder = ViewportBuilder::default()
            .with_title("SnagCraft widget")
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_inner_size([w, h]);
        ctx.show_viewport_immediate(widget_viewport_id(), builder, |ui, class| {
            widget_ui(ui, self, class);
        });
        self.widget_shown = true;
    }

    /// Web: a browser tab cannot host a second transparent always-on-top
    /// viewport, so the floating widget stays a desktop feature.
    #[cfg(not(feature = "native"))]
    fn floating_widget(&mut self, _ctx: &egui::Context) {}
}

/// A small picture glyph for the close-guard modal (painted, not a font glyph).
fn picture_icon(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
    let frame = Rect::from_center_size(rect.center(), vec2(42.0, 34.0));
    let accent = pal_accent(ui);
    let painter = ui.painter();
    painter.rect_filled(frame, CornerRadius::same(4), accent.linear_multiply(0.12));
    painter.rect_stroke(
        frame,
        CornerRadius::same(4),
        Stroke::new(1.5, accent),
        StrokeKind::Inside,
    );
    // Sun + two hills: reads as "picture" at 42×34.
    painter.circle_filled(pos2(frame.left() + 11.0, frame.top() + 10.0), 3.2, accent);
    painter.add(Shape::convex_polygon(
        vec![
            pos2(frame.left() + 5.0, frame.bottom() - 4.0),
            pos2(frame.left() + 18.0, frame.top() + 15.0),
            pos2(frame.left() + 30.0, frame.bottom() - 4.0),
        ],
        accent.linear_multiply(0.55),
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            pos2(frame.left() + 20.0, frame.bottom() - 4.0),
            pos2(frame.left() + 30.0, frame.top() + 18.0),
            pos2(frame.right() - 4.0, frame.bottom() - 4.0),
        ],
        accent,
        Stroke::NONE,
    ));
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).strong().size(12.5));
}

/// The settings body. Pure UI: it mutates `next` and reports what happened.
fn settings_ui(ui: &mut egui::Ui, next: &mut SettingsView, acts: &mut SettingsActions) {
    section(ui, "General");
    // Tray / autostart / floating widget are desktop services; the web build
    // keeps the same settings file shape but has nothing to apply them to.
    #[cfg(feature = "native")]
    {
        acts.changed |= ui
            .checkbox(
                &mut next.minimize_to_tray,
                "Minimize to tray instead of closing",
            )
            .on_hover_text("Close hides the window; the tray icon brings it back")
            .changed();
        acts.changed |= ui
            .checkbox(&mut next.start_at_login, "Start at login")
            .changed();
        acts.changed |= ui
            .checkbox(&mut next.show_widget, "Show the floating widget")
            .changed();
    }
    #[cfg(not(feature = "native"))]
    {
        // No-op prefixes keep the shape of the native block (and the value
        // checks) without pretending the services exist in a browser tab.
        let _ = (next.minimize_to_tray, next.start_at_login, next.show_widget);
        ui.label(
            egui::RichText::new("Tray, login autostart and the floating widget are desktop-only.")
                .weak()
                .size(11.0),
        );
    }
    acts.changed |= ui
        .checkbox(&mut next.autosave_captures, "Auto-save captures")
        .changed();
    ui.horizontal(|ui| {
        ui.label("Capture delay");
        acts.changed |= ui
            .add(
                egui::Slider::new(&mut next.capture_delay_secs, 0..=10)
                    .suffix(" s")
                    .integer()
                    .trailing_fill(true),
            )
            .on_hover_text("Wait this long before grabbing the screen")
            .changed();
    });

    ui.separator();
    section(ui, "Export");
    ui.horizontal(|ui| {
        ui.label("Default format");
        egui::ComboBox::from_id_salt("settings-format")
            .width(96.0)
            .selected_text(next.default_format.clone())
            .show_ui(ui, |ui| {
                for format in ["png", "jpeg", "webp"] {
                    acts.changed |= ui
                        .selectable_value(&mut next.default_format, format.to_string(), format)
                        .changed();
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Quality");
        acts.changed |= ui
            .add(
                egui::Slider::new(&mut next.default_quality, 1..=100)
                    .suffix(" %")
                    .trailing_fill(true),
            )
            .on_hover_text("JPEG/WebP only — PNG is lossless")
            .changed();
    });
    section(ui, "Watermark");
    ui.horizontal(|ui| {
        ui.label("Text");
        acts.changed |= ui
            .add(
                egui::TextEdit::singleline(&mut next.watermark_text)
                    .desired_width(200.0)
                    .hint_text("Empty = no watermark"),
            )
            .on_hover_text("Stamped onto exported images only, never the document")
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Opacity");
        acts.changed |= ui
            .add(
                egui::Slider::new(
                    &mut next.watermark_opacity,
                    WATERMARK_OPACITY_MIN..=WATERMARK_OPACITY_MAX,
                )
                .suffix(" %")
                .trailing_fill(true),
            )
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Corner");
        egui::ComboBox::from_id_salt("settings-watermark-corner")
            .width(140.0)
            .selected_text(corner_label(&next.watermark_corner))
            .show_ui(ui, |ui| {
                for corner in WATERMARK_CORNERS {
                    acts.changed |= ui
                        .selectable_value(
                            &mut next.watermark_corner,
                            corner.to_string(),
                            corner_label(corner),
                        )
                        .changed();
                }
            });
    });
    section(ui, "Share");
    ui.horizontal(|ui| {
        ui.label("Upload URL");
        acts.changed |= ui
            .add(
                egui::TextEdit::singleline(&mut next.upload_url)
                    .desired_width(200.0)
                    .hint_text(UPLOAD_URL_DEFAULT),
            )
            .on_hover_text("Opened by Export — Open upload page")
            .changed();
    });
    #[cfg(feature = "native")]
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!next.upload_url.trim().is_empty(), |ui| {
            if ui
                .button("Open upload page")
                .on_hover_text("Open the share page in the browser")
                .clicked()
            {
                acts.open_upload = true;
            }
        });
        ui.label(
            egui::RichText::new("Opened with the system browser.")
                .weak()
                .size(11.0),
        );
    });
    #[cfg(not(feature = "native"))]
    ui.label(
        egui::RichText::new("Sharing through the upload page is desktop-only.")
            .weak()
            .size(11.0),
    );

    ui.separator();
    section(ui, "Save location");
    ui.horizontal(|ui| {
        ui.label("Folder");
        let shown = next
            .save_dir
            .clone()
            .unwrap_or_else(|| "Pictures/SnagCraft".into());
        ui.add(
            egui::Label::new(egui::RichText::new(shown).size(11.5).monospace())
                .truncate()
                .sense(Sense::hover()),
        );
        #[cfg(feature = "native")]
        if ui.button("Choose…").clicked() {
            acts.pick_dir = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("File name");
        acts.changed |= ui
            .add(
                egui::TextEdit::singleline(&mut next.filename_pattern)
                    .desired_width(220.0)
                    .hint_text("SnagCraft-{date}-{time}"),
            )
            .on_hover_text("{date} and {time} expand at save time")
            .changed();
    });

    ui.separator();
    section(ui, "Global hotkeys");
    ui.horizontal(|ui| {
        ui.label("Capture full screen");
        acts.changed |= ui
            .add(
                egui::TextEdit::singleline(&mut next.hotkey_full)
                    .desired_width(150.0)
                    .hint_text("Print"),
            )
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Capture region");
        acts.changed |= ui
            .add(
                egui::TextEdit::singleline(&mut next.hotkey_region)
                    .desired_width(150.0)
                    .hint_text("SHIFT+Print"),
            )
            .changed();
    });
    ui.horizontal(|ui| {
        if ui
            .button("Rebind")
            .on_hover_text("Ask the compositor to bind these two shortcuts")
            .clicked()
        {
            acts.rebind = true;
        }
        ui.label(
            egui::RichText::new("The compositor may ask for approval.")
                .weak()
                .size(11.0),
        );
    });

    ui.separator();
    section(ui, "Theme");
    ui.horizontal(|ui| {
        acts.changed |= ui
            .radio_value(&mut next.theme, Theme::Light, "Light")
            .changed();
        acts.changed |= ui
            .radio_value(&mut next.theme, Theme::Dark, "Dark")
            .changed();
    });
}

/// One compact pill button (the widget paints its own chrome).
fn widget_button(ui: &mut egui::Ui, rect: Rect, label: &str) -> Response {
    ui.put(
        rect,
        egui::Button::new(egui::RichText::new(label).size(12.0).color(Color32::WHITE))
            .fill(Color32::from_rgb(46, 51, 61))
            .corner_radius(CornerRadius::same(8)),
    )
}

/// STOP control for the widget's recording layout: a filled red square with a
/// white glyph (the live 1.5.0 widget look — no text button).
fn stop_square_button(ui: &mut egui::Ui, rect: Rect) -> Response {
    let resp = ui.interact(rect, ui.scope_id().with("widget-stop"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Stop"));
    let fill = if resp.hovered() {
        WIDGET_STOP.linear_multiply(1.18)
    } else {
        WIDGET_STOP
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(7), fill);
    let glyph = (rect.width().min(rect.height()) * 0.40).clamp(9.0, 14.0);
    painter.rect_filled(
        Rect::from_center_size(rect.center(), vec2(glyph, glyph)),
        CornerRadius::same(1),
        Color32::WHITE,
    );
    resp.on_hover_text("Stop recording")
}

/// The floating widget's contents. Called from an immediate viewport, so the
/// whole window is transparent and just the pill is painted.
///
/// `class` matters: only a real `Immediate` viewport may resize/move itself —
/// when egui embeds the viewport instead (kittest, backends without viewport
/// support) those commands would resize the *parent* window.
pub fn widget_ui(ui: &mut egui::Ui, app: &mut SnagCraftApp, class: egui::ViewportClass) {
    let ctx = ui.ctx().clone();
    let full = ui.max_rect();
    let pill = full.shrink(4.0);
    let radius = CornerRadius::same(if app.widget_expanded { 24 } else { 14 });
    let painter = ui.painter().clone();
    painter.rect_filled(pill, radius, WIDGET_BG);
    painter.rect_stroke(
        pill,
        radius,
        Stroke::new(1.0, Color32::from_white_alpha(46)),
        StrokeKind::Inside,
    );

    // Drag from anywhere on the pill (Wayland-native StartDrag, W0-proved).
    let drag = ui.interact(
        pill,
        ui.scope_id().with("widget-drag"),
        Sense::click_and_drag(),
    );
    if drag.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    if app.widget_expanded {
        let h = pill.height();
        let btn_h = (h - 16.0).clamp(18.0, 32.0);
        let y = pill.center().y - btn_h * 0.5;
        let mut x = pill.left() + 10.0;
        if app.recording() {
            // Live layout: [stop square] [one Pause/Resume] [live dot] [×].
            // No fold control mid-take — the take owns the pill.
            let square = btn_h.clamp(20.0, 26.0);
            let stop_rect = Rect::from_min_size(pos2(x, y), vec2(square, square));
            if stop_square_button(ui, stop_rect).clicked() {
                app.stop_recording();
            }
            let pause_rect = Rect::from_min_size(pos2(x + square + 6.0, y), vec2(58.0, btn_h));
            let toggle = if app.rec.paused { "Resume" } else { "Pause" };
            if widget_button(ui, pause_rect, toggle).clicked() {
                app.toggle_record_pause();
            }
            painter.circle_filled(pos2(pill.right() - 44.0, pill.center().y), 4.5, WIDGET_RED);
        } else {
            let mut place = |ui: &mut egui::Ui, width: f32, label: &str| -> Response {
                let rect = Rect::from_min_size(pos2(x, y), vec2(width, btn_h));
                x += width + 6.0;
                widget_button(ui, rect, label)
            };
            if place(ui, 50.0, "Full").clicked() {
                app.capture_full();
            }
            if place(ui, 62.0, "Region").clicked() {
                app.capture_region();
            }
            if place(ui, 52.0, "Rec").clicked() {
                app.start_recording(None);
            }
        }

        if !app.recording() {
            let fold_rect = Rect::from_min_size(pos2(pill.right() - 62.0, y), vec2(26.0, btn_h));
            if widget_button(ui, fold_rect, "»").clicked() {
                app.widget_expanded = false;
            }
        }
        let close_rect = Rect::from_min_size(pos2(pill.right() - 30.0, y), vec2(22.0, btn_h));
        let close = ui
            .put(
                close_rect,
                egui::Button::new(egui::RichText::new("×").size(13.0).color(Color32::WHITE))
                    .fill(Color32::TRANSPARENT)
                    .corner_radius(CornerRadius::same(8)),
            )
            .on_hover_text("Hide the widget");
        if close.clicked() {
            let mut next = app.settings.clone();
            next.show_widget = false;
            app.push_settings(next);
            // Never strand the app: if the main window is hidden, closing the
            // widget restores it (the widget was the way back).
            if app.hidden {
                app.hidden = false;
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
        }
    } else {
        // Folded tab: wordmark, live dot, expand chevron — laid out against the
        // rect we actually got, so a host that hands us a shorter/wider area
        // (embedded viewports do) still stacks cleanly instead of colliding.
        let chevron_h = (pill.height() * 0.26).clamp(14.0, 20.0);
        painter.text(
            pos2(pill.center().x, pill.top() + chevron_h * 0.75),
            Align2::CENTER_CENTER,
            "S",
            FontId::proportional((pill.width() * 0.30).clamp(12.0, 16.0)),
            Color32::from_rgb(235, 238, 245),
        );
        painter.circle_filled(
            pos2(pill.center().x, pill.center().y + 2.0),
            (pill.width() * 0.09).clamp(3.5, 5.0),
            WIDGET_RED,
        );
        let chevron = Rect::from_center_size(
            pos2(pill.center().x, pill.bottom() - chevron_h * 0.5 - 3.0),
            vec2((pill.width() * 0.62).clamp(20.0, 30.0), chevron_h),
        );
        let expand = ui
            .put(
                chevron,
                egui::Button::new(egui::RichText::new("»").size(13.0).color(Color32::WHITE))
                    .fill(Color32::from_rgb(46, 51, 61))
                    .corner_radius(CornerRadius::same(8)),
            )
            .on_hover_text("Open the capture widget");
        if expand.clicked() || (drag.clicked() && !drag.dragged()) {
            app.widget_expanded = true;
        }
        // Enter / Space expand the tab (a11y).
        if ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space)) {
            app.widget_expanded = true;
        }
    }

    // Widget-scoped V: start/stop the take. The main window keeps bare V for
    // the Select tool (13-tool parity), so this only fires in the widget's own
    // viewport (a real one — embedded/kittest viewports share the root input).
    if class == egui::ViewportClass::Immediate
        && ui.input(|i| i.key_pressed(Key::V) && !i.modifiers.any())
    {
        if app.recording() {
            app.stop_recording();
        } else {
            app.start_recording(None);
        }
    }

    // Park top-right and resize only when the state actually changed, and only
    // for a viewport we actually own.
    if class == egui::ViewportClass::Immediate && app.widget_applied != Some(app.widget_expanded) {
        let (w, h) = if app.widget_expanded {
            WIDGET_EXPANDED_SIZE
        } else {
            WIDGET_FOLDED_SIZE
        };
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(w, h)));
        if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(
                (monitor.x - w - 16.0).max(0.0),
                16.0,
            )));
        }
        app.widget_applied = Some(app.widget_expanded);
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

/// Install the shell's visual style. Light is the long-standing palette; Dark
/// is `Visuals::dark()` with the #3d7ec2 accent and near-black panel fills.
/// Slider trailing fill and the 4px corner radius stay identical in both.
pub fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let (mut v, accent, border, fg, muted, weak) = match theme {
        Theme::Light => (
            egui::Visuals::light(),
            ACCENT,
            BORDER,
            FG,
            FG_MUTED,
            Color32::from_rgb(249, 250, 251),
        ),
        Theme::Dark => (
            egui::Visuals::dark(),
            ACCENT_DARK,
            DARK_BORDER,
            DARK_FG,
            Color32::from_rgb(154, 161, 173),
            Color32::from_rgb(38, 42, 48),
        ),
    };
    match theme {
        Theme::Light => {
            v.panel_fill = PANEL_FILL;
            v.window_fill = Color32::WHITE;
            v.extreme_bg_color = Color32::WHITE;
            v.widgets.inactive.bg_fill = Color32::WHITE;
            v.widgets.hovered.bg_fill = Color32::from_rgb(229, 231, 235);
            v.widgets.active.bg_fill = Color32::from_rgb(224, 226, 230);
        }
        Theme::Dark => {
            v.panel_fill = DARK_PANEL;
            v.window_fill = DARK_SURFACE;
            v.extreme_bg_color = Color32::from_rgb(22, 24, 28);
            v.widgets.noninteractive.bg_fill = DARK_PANEL;
            v.widgets.inactive.bg_fill = Color32::from_rgb(43, 47, 54);
            v.widgets.hovered.bg_fill = Color32::from_rgb(51, 56, 66);
            v.widgets.active.bg_fill = Color32::from_rgb(58, 64, 72);
        }
    }
    v.faint_bg_color = weak;
    v.selection.bg_fill = accent.linear_multiply(0.35);
    v.slider_trailing_fill = true;
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    v.weak_text_color = Some(muted);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(4);
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, fg);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, fg);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, fg);
    v.widgets.open.fg_stroke = Stroke::new(1.0, fg);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.bg_stroke = Stroke::new(1.5, accent);
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

/// A painted gear (the bundled fonts carry no U+2699, so the glyph is drawn)
/// with an explicit accessibility label the tests can query.
fn settings_button(ui: &mut egui::Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let ink = if resp.hovered() {
        pal_accent(ui)
    } else {
        pal_fg(ui)
    };
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), pal_hover(ui));
    }
    let c = rect.center();
    let r = rect.width() * 0.26;
    let stroke = Stroke::new(1.7, ink);
    let painter = ui.painter();
    painter.circle_stroke(c, r, stroke);
    painter.circle_stroke(c, r * 0.36, stroke);
    for i in 0..8 {
        let angle = std::f32::consts::TAU * (i as f32) / 8.0;
        let (sin, cos) = angle.sin_cos();
        painter.line_segment(
            [
                pos2(c.x + cos * r, c.y + sin * r),
                pos2(c.x + cos * r * 1.45, c.y + sin * r * 1.45),
            ],
            stroke,
        );
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Settings (Ctrl+,)"));
    resp.on_hover_text("Settings (Ctrl+,)")
}

fn tool_button(ui: &mut egui::Ui, tool: Tool, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let accent = pal_accent(ui);
    let fill = if active {
        accent.linear_multiply(0.16)
    } else if resp.hovered() {
        pal_hover(ui)
    } else {
        pal_surface(ui)
    };
    let border = if active { accent } else { pal_border(ui) };
    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(if active { 1.5 } else { 1.0 }, border),
        StrokeKind::Outside,
    );
    let ink = if active { accent } else { pal_fg(ui) };
    icon(ui.painter(), rect, tool, ink);
    resp.on_hover_text(tool.label())
}

// ------------------------------------------------------------------- the rail

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(pal_muted(ui)).size(11.5));
}

fn palette(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    let accent = pal_accent(ui);
    let border = pal_border(ui);
    for (name, rgb) in PALETTE {
        let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        let selected = app.color.to_array()[..3] == rgb;
        let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        ui.painter().rect_filled(rect, CornerRadius::same(5), color);
        let ring = if selected { accent } else { border };
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
    ui.label(egui::RichText::new(label).color(pal_muted(ui)).size(11.0))
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
    let accent = pal_accent(ui);
    let border = pal_border(ui);
    let ink = pal_fg(ui);
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
            accent.linear_multiply(0.16)
        } else if resp.hovered() {
            pal_hover(ui)
        } else {
            pal_surface(ui)
        };
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(
                if active { 1.5 } else { 1.0 },
                if active { accent } else { border },
            ),
            StrokeKind::Outside,
        );
        shape_icon(
            ui.painter(),
            rect.shrink(7.0),
            kind,
            if active { accent } else { ink },
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
        ui.label(
            egui::RichText::new("Shadow")
                .color(pal_muted(ui))
                .size(11.0),
        );
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
    ui.label(egui::RichText::new(label).color(pal_muted(ui)).size(11.0));
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
                ui.label(
                    egui::RichText::new("Radius")
                        .color(pal_muted(ui))
                        .size(11.0),
                );
                ui.add_sized(
                    vec2(132.0, 18.0),
                    egui::Slider::new(&mut app.blur_radius, BLUR_MIN..=BLUR_MAX)
                        .trailing_fill(true),
                )
                .on_hover_text("Blur radius (2-60, raised to 10 under the hood for legibility)");
            }
            Tool::Bucket => {
                palette(ui, app);
                ui.label(
                    egui::RichText::new("Tolerance")
                        .color(pal_muted(ui))
                        .size(11.0),
                );
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
                ui.label(egui::RichText::new("Dim").color(pal_muted(ui)).size(11.0));
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
                ui.label(egui::RichText::new("Zoom").color(pal_muted(ui)).size(11.0));
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

    // A region picker owns Enter/Esc while it is up (crop-frame idiom).
    if app.pick.is_some() {
        let (confirm, cancel) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if confirm {
            app.confirm_region_pick();
        }
        if cancel {
            app.cancel_region_pick();
        }
        return;
    }

    ctx.input_mut(|i| {
        // Ctrl+Alt+V: start/stop a recording. Bare V stays the Select tool
        // (the 13-tool map owns every unmodified letter).
        if i.consume_key(Modifiers::CTRL | Modifiers::ALT, Key::V) {
            if app.recording() {
                app.stop_recording();
            } else {
                app.start_recording(None);
            }
        }
        // Enter finishes a scrolling capture (Enter is otherwise unclaimed).
        if app.scroll.armed && i.consume_key(Modifiers::NONE, Key::Enter) {
            app.stop_scrolling();
        }
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
        if i.consume_key(Modifiers::CTRL, Key::Comma) {
            app.settings_open = true;
        }
        if i.consume_key(Modifiers::CTRL, Key::D)
            && let Some(id) = app.selected
        {
            app.dispatch("edit.duplicate", serde_json::json!({"id": id}));
        }
        // Only claim Enter while a crop frame is pending — otherwise the
        // floating widget's Enter-to-expand never sees the key.
        if app.crop.is_some()
            && i.consume_key(Modifiers::NONE, Key::Enter)
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

/// "bottom-right" → "Bottom right" (menu/combo label).
fn corner_label(corner: &str) -> &'static str {
    match corner {
        "bottom-left" => "Bottom left",
        "top-right" => "Top right",
        "top-left" => "Top left",
        _ => "Bottom right",
    }
}

/// One-line HUD text: strong and a touch larger than the status bar.
fn hud_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(12.0).strong()
}

/// The top banner: whatever live job is running speaks here (recording,
/// scrolling capture, timelapse), with its own controls.
fn media_hud(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    let scrolling = app.scroll.armed || app.scroll.active;
    let timelapsing = app.timelapse.armed || app.timelapse.running;
    if !(app.recording() || scrolling || timelapsing) {
        return;
    }
    let dark = ui.visuals().dark_mode;
    egui::Panel::top("media-hud")
        .frame(
            egui::Frame::new()
                .fill(if dark {
                    DARK_SURFACE
                } else {
                    Color32::from_rgb(254, 235, 235)
                })
                .inner_margin(egui::Margin::symmetric(8, 5))
                .stroke(Stroke::new(1.0, WIDGET_RED.linear_multiply(0.55))),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if app.recording() {
                    // Painted dot: the bundled faces have no ● guarantee.
                    let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.5, WIDGET_RED);
                    let clock = app.record_clock();
                    let label = if !app.rec.active {
                        "Starting recorder…".to_string()
                    } else if app.rec.paused {
                        format!("Recording paused {clock}")
                    } else {
                        format!("Recording {clock}")
                    };
                    ui.label(hud_text(label).color(pal_fg(ui)));
                    if app.rec.active {
                        let toggle = if app.rec.paused { "Resume" } else { "Pause" };
                        if ui
                            .button(toggle)
                            .on_hover_text("Pause/resume the encoder")
                            .clicked()
                        {
                            app.toggle_record_pause();
                        }
                    }
                    if ui
                        .button("Stop recording")
                        .on_hover_text("Finalise the take and save it")
                        .clicked()
                    {
                        app.stop_recording();
                    }
                    ui.separator();
                    let audio = match app.rec.audio.as_deref() {
                        Some(id) => app
                            .rec
                            .sources
                            .iter()
                            .find(|(sid, _)| sid == id)
                            .map(|(_, name)| name.clone())
                            .unwrap_or_else(|| id.to_string()),
                        None => "no audio".to_string(),
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{} — {audio}",
                            app.rec.format.to_ascii_uppercase()
                        ))
                        .size(11.0)
                        .color(pal_muted(ui)),
                    );
                }
                if scrolling {
                    ui.label(
                        hud_text(format!(
                            "Scroll slowly — captured {} px, {} frames. Enter or Stop to finish.",
                            app.scroll.total_height, app.scroll.index
                        ))
                        .color(pal_fg(ui)),
                    );
                    if ui.button("Stop scroll").clicked() {
                        app.stop_scrolling();
                    }
                }
                if timelapsing {
                    ui.label(
                        hud_text(format!("Timelapse: {} frames", app.timelapse.frames))
                            .color(pal_fg(ui)),
                    );
                    if ui.button("Stop timelapse").clicked() {
                        app.stop_timelapse();
                    }
                }
            });
        });
}

/// The Capture dropdown: one place for every grab (screen, region, record,
/// scroll, OCR, timelapse) so the toolbar keeps its single row.
///
/// Desktop only: every entry dispatches a native engine command.
#[cfg(feature = "native")]
fn capture_menu(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    let capturing = app.capture_rx.is_some();
    let accent = pal_accent(ui);
    ui.scope(|ui| {
        // Paint it as the primary button: accent fill, white ink.
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = accent;
        widgets.hovered.weak_bg_fill = accent.linear_multiply(1.15);
        widgets.active.weak_bg_fill = accent.linear_multiply(0.85);
        widgets.inactive.fg_stroke.color = Color32::WHITE;
        widgets.hovered.fg_stroke.color = Color32::WHITE;
        widgets.active.fg_stroke.color = Color32::WHITE;
        ui.menu_button(egui::RichText::new("Capture").strong(), |ui| {
            ui.set_min_width(250.0);
            if ui
                .add_enabled(!capturing, egui::Button::new("Full screen"))
                .clicked()
            {
                app.capture_full();
                ui.close();
            }
            if ui
                .add_enabled(!capturing, egui::Button::new("Region…"))
                .clicked()
            {
                app.capture_region();
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(!app.recording(), egui::Button::new("Record screen"))
                .on_hover_text("Ctrl+Alt+V")
                .clicked()
            {
                app.start_recording(None);
                ui.close();
            }
            if ui
                .add_enabled(!app.recording(), egui::Button::new("Record region…"))
                .clicked()
            {
                app.start_region_pick(PickPurpose::Record);
                ui.close();
            }
            ui.menu_button("Recording options", |ui| {
                ui.set_min_width(220.0);
                ui.label(egui::RichText::new("Format").size(11.5).weak());
                for format in ["mp4", "gif"] {
                    let label = format.to_ascii_uppercase();
                    if ui
                        .radio_value(&mut app.rec.format, format.to_string(), label)
                        .changed()
                    {
                        // Recording options are per-take, not persisted.
                    }
                }
                ui.separator();
                ui.label(egui::RichText::new("Audio").size(11.5).weak());
                if !app.rec.sources_loaded {
                    app.refresh_audio_sources();
                }
                let mut choice = app.rec.audio.clone();
                let none = ui.radio_value(&mut choice, None, "None").changed();
                let mut picked = none;
                let sources = app.rec.sources.clone();
                for (id, name) in sources {
                    if ui
                        .radio_value(&mut choice, Some(id.clone()), name)
                        .changed()
                    {
                        picked = true;
                    }
                }
                if picked {
                    app.rec.audio = choice;
                }
            });
            ui.separator();
            if ui.button("Scrolling capture…").clicked() {
                app.start_region_pick(PickPurpose::Scroll);
                ui.close();
            }
            let ocr_ready = app.ocr_ready();
            let ocr = ui
                .add_enabled(ocr_ready, egui::Button::new("Grab text (OCR)"))
                .on_hover_text("Read the text in the current image");
            let ocr = if ocr_ready {
                ocr
            } else {
                ocr.on_disabled_hover_text("Install tesseract")
            };
            if ocr.clicked() {
                app.run_ocr();
                ui.close();
            }
            if ui.button("Timelapse…").clicked() {
                app.timelapse.dialog_open = true;
                ui.close();
            }
            ui.separator();
            let upload = app.upload_target();
            if ui
                .add_enabled(upload.is_some(), egui::Button::new("Open upload page"))
                .on_hover_text("Set the URL in Settings — Export")
                .clicked()
            {
                app.open_upload();
                ui.close();
            }
        });
    });
}

/// The frozen-screen region picker (record/scroll): crop-frame idiom, but the
/// pixels are screen pixels so the rect goes straight to the engine.
/// Returns true when it handled the whole central panel.
fn region_picker(ui: &mut egui::Ui, app: &mut SnagCraftApp) -> bool {
    let Some(pick) = app.pick.as_mut() else {
        return false;
    };
    let avail = ui.available_rect_before_wrap();
    if !avail.is_positive() {
        return true;
    }
    ui.painter()
        .rect_filled(avail, CornerRadius::ZERO, Color32::from_rgb(16, 18, 22));
    let Some(surface) = pick.surface.as_ref() else {
        ui.painter().text(
            avail.center(),
            Align2::CENTER_CENTER,
            "Capturing the screen…",
            FontId::proportional(14.0),
            Color32::WHITE,
        );
        return true;
    };
    let (iw, ih) = (surface.width() as f32, surface.height() as f32);
    if iw < 1.0 || ih < 1.0 {
        return true;
    }
    let scale = (avail.width() / iw)
        .min(avail.height() / ih)
        .clamp(0.01, 8.0);
    let size = vec2(iw * scale, ih * scale);
    let origin = avail.center() - size * 0.5;
    let img_rect = Rect::from_min_size(origin, size);
    if pick.tex.is_none() {
        let color_img = egui::ColorImage::from_rgba_unmultiplied(
            [surface.width() as usize, surface.height() as usize],
            surface.as_raw(),
        );
        pick.tex = Some(ui.ctx().load_texture(
            "snagcraft-pick",
            color_img,
            egui::TextureOptions::LINEAR,
        ));
    }
    if let Some(tex) = &pick.tex {
        ui.painter().image(
            tex.id(),
            img_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let to_image = |p: Pos2| Point::new((p.x - origin.x) / scale, (p.y - origin.y) / scale);
    let resp = ui.interact(
        img_rect,
        ui.scope_id().with("region-pick"),
        Sense::click_and_drag(),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let pointer_img = resp.interact_pointer_pos().map(to_image);
    let press_img = ui
        .ctx()
        .input(|i| i.pointer.press_origin())
        .map(to_image)
        .or(pointer_img);
    if resp.drag_started()
        && let Some(p) = press_img
    {
        pick.rect = Some(GRect::new(p.x, p.y, 0.0, 0.0));
    }
    if resp.dragged()
        && let (Some(p), Some(start)) = (
            pointer_img,
            pick.rect.as_ref().map(|r| Point::new(r.x, r.y)),
        )
    {
        let end = Point::new(p.x.clamp(0.0, iw), p.y.clamp(0.0, ih));
        pick.rect = Some(GRect::from_corners(start, end));
    }

    // Dim everything outside the frame, then outline it like the crop tool.
    let frame = pick.rect.map(|frame| {
        Rect::from_min_size(
            pos2(origin.x + frame.x * scale, origin.y + frame.y * scale),
            vec2(frame.w * scale, frame.h * scale),
        )
    });
    match frame {
        Some(r) => {
            dim_outside_preview(ui.painter(), img_rect, r, 0.55);
            ui.painter().rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(1.5, Color32::WHITE),
                StrokeKind::Inside,
            );
            for corner in [
                r.left_top(),
                r.right_top(),
                r.left_bottom(),
                r.right_bottom(),
            ] {
                ui.painter().rect_filled(
                    Rect::from_center_size(corner, vec2(6.0, 6.0)),
                    CornerRadius::ZERO,
                    Color32::WHITE,
                );
            }
            if let Some(rect) = pick.rect {
                ui.painter().text(
                    r.left_top() + vec2(4.0, 18.0),
                    Align2::LEFT_TOP,
                    format!("{}×{}", rect.w.round(), rect.h.round()),
                    FontId::monospace(12.0),
                    Color32::WHITE,
                );
            }
        }
        None => {
            ui.painter()
                .rect_filled(img_rect, CornerRadius::ZERO, Color32::from_black_alpha(60));
        }
    }
    let hint = format!(
        "{} — drag a rectangle, Enter to start, Esc to cancel",
        match pick.purpose {
            PickPurpose::Record => "Recording region",
            PickPurpose::Scroll => "Scrolling capture region",
        }
    );
    let bar = Rect::from_min_size(
        pos2(avail.left() + 12.0, avail.top() + 10.0),
        vec2(avail.width() - 24.0, 26.0),
    );
    ui.painter()
        .rect_filled(bar, CornerRadius::same(6), Color32::from_black_alpha(190));
    ui.painter().text(
        bar.left_center() + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        hint,
        FontId::proportional(12.5),
        Color32::WHITE,
    );
    true
}

// ------------------------------------------------------------------- the shell

/// The whole shell in one fn — shared by eframe and headless kittest.
pub fn app_ui(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    let ctx = ui.ctx().clone();
    app.main_ctx = Some(ctx.clone());
    app.apply_settings();
    app.poll_platform(&ctx);
    #[cfg(not(feature = "native"))]
    app.poll_dropped(&ctx);
    app.poll_capture();
    app.poll_media();
    if app.recording()
        || app.scroll.armed
        || app.scroll.active
        || app.timelapse.armed
        || app.timelapse.running
    {
        // Elapsed clock + frame counts keep ticking while nothing else does.
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    app.handle_close_request(&ctx);

    media_hud(ui, app);

    egui::Panel::top("toolbar")
        .frame(
            egui::Frame::new()
                .fill(pal_panel(ui))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .stroke(Stroke::new(1.0, pal_border(ui))),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    #[cfg(feature = "native")]
                    let capturing = app.capture_rx.is_some();
                    if settings_button(ui).clicked() {
                        app.settings_open = true;
                    }
                    let (can_redo, can_undo) = {
                        let sess = app.sess();
                        (sess.can_redo(), sess.can_undo())
                    };
                    ui.add_enabled_ui(can_redo, |ui| {
                        if ui.button("Redo").on_hover_text("Ctrl+Shift+Z").clicked() {
                            app.dispatch("edit.redo", serde_json::json!({}));
                        }
                    });
                    ui.add_enabled_ui(can_undo, |ui| {
                        if ui.button("Undo").on_hover_text("Ctrl+Z").clicked() {
                            app.dispatch("edit.undo", serde_json::json!({}));
                        }
                    });
                    if ui
                        .button("Export")
                        .on_hover_text("Export the flattened image")
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
                    #[cfg(feature = "native")]
                    capture_menu(ui, app);
                    #[cfg(feature = "native")]
                    ui.add_enabled_ui(!capturing, |ui| {
                        if ui
                            .button("OCR")
                            .on_hover_text("Grab text (OCR) from the current image")
                            .clicked()
                        {
                            app.run_ocr();
                        }
                    });
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
                .fill(pal_surface(ui))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .stroke(Stroke::new(1.0, pal_border(ui))),
        )
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("props-scroll")
                .show(ui, |ui| rail(ui, app));
        });

    egui::Panel::bottom("status")
        .frame(
            egui::Frame::new()
                .fill(pal_panel(ui))
                .inner_margin(egui::Margin::symmetric(8, 3))
                .stroke(Stroke::new(1.0, pal_border(ui))),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some((w, h)) = app.sess().doc().map(|d| (d.size.w, d.size.h)) {
                    ui.label(
                        egui::RichText::new(format!("{w}×{h}"))
                            .color(pal_muted(ui))
                            .size(11.0),
                    );
                    ui.separator();
                }
                ui.label(
                    egui::RichText::new(app.tool.label().split(" (").next().unwrap_or("Tool"))
                        .color(pal_fg(ui))
                        .size(11.0)
                        .strong(),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(&app.status)
                        .color(pal_muted(ui))
                        .size(11.0),
                );
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
                .fill(pal_canvas(ui))
                .inner_margin(egui::Margin::same(0)),
        )
        .show(ui, |ui| {
            // Recording / scrolling-capture region picker owns the canvas.
            if region_picker(ui, app) {
                return;
            }
            let avail = ui.available_rect_before_wrap();
            let Some((iw, ih)) = app.sess().doc().map(|d| (d.size.w, d.size.h)) else {
                ui.label("No document — Capture or Open to begin.");
                return;
            };
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
            let rendered = if app.redraw || app.tex.is_none() {
                app.sess().render().ok()
            } else {
                None
            };
            if let Some(img) = rendered {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                app.tex = Some(ctx.load_texture(
                    "snagcraft-doc",
                    color_img,
                    egui::TextureOptions::LINEAR,
                ));
                app.redraw = false;
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
                && let Some(obj) = app.sess().doc().and_then(|d| d.get(id))
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
                        app.selected = {
                            let sess = app.sess();
                            sess.doc().and_then(|d| d.hit_test(p))
                        };
                    }
                    if resp.drag_started()
                        && let (Some(p), Some(id)) = (press_img, app.selected)
                    {
                        let inside = app
                            .sess()
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
                        && let Some(obj) = app.sess().doc().and_then(|d| d.get(*id))
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
                app.selected = {
                    let sess = app.sess();
                    sess.doc().and_then(|d| d.hit_test(p))
                };
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
                let (can_undo, can_redo) = {
                    let sess = app.sess();
                    (sess.can_undo(), sess.can_redo())
                };
                ui.add_enabled_ui(can_undo, |ui| {
                    if menu_row(ui, "Undo", "Ctrl+Z") {
                        app.dispatch("edit.undo", serde_json::json!({}));
                    }
                });
                ui.add_enabled_ui(can_redo, |ui| {
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

    app.settings_window(&ctx);
    app.close_modal(&ctx);
    app.ocr_window(&ctx);
    app.timelapse_dialog(&ctx);
    app.floating_widget(&ctx);

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
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        SnagCraftApp::logic(self, ctx);
    }
    /// Fully transparent root: the floating widget viewport needs it, and the
    /// main window stays opaque because every panel paints a full-size fill.
    #[cfg(feature = "native")]
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Web: the canvas is the whole page, so the root fill is the page colour
    /// (index.html paints the same dark behind it while the wasm boots).
    #[cfg(not(feature = "native"))]
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.07, 0.08, 0.10, 1.0]
    }
}

// ------------------------------------------------------------- web fallbacks

/// Decode image bytes and load them into the shared session — the web "open"
/// path (the browser hands over bytes, never a filesystem path).
#[cfg(not(feature = "native"))]
fn load_image_bytes(session: &Arc<Mutex<Session>>, bytes: &[u8]) -> Result<(u32, u32), String> {
    let surface = snagcraft_io::decode(bytes).map_err(|e| e.to_string())?;
    let (w, h) = (surface.width(), surface.height());
    let mut guard = session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.load_surface(surface);
    Ok((w, h))
}

/// Browser file plumbing for the web build: the PNG/JPEG/WebP export download.
#[cfg(all(not(feature = "native"), target_arch = "wasm32"))]
mod web {
    use wasm_bindgen::JsCast as _;

    fn js_err(value: wasm_bindgen::JsValue) -> String {
        format!("{value:?}")
    }

    /// Save `bytes` through the browser: a typed `Blob`, an object URL, a
    /// synthetic `<a download>` click, then the URL is revoked. The anchor is
    /// appended to the body because Chromium ignores a click on a detached one.
    pub fn download(name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let document = window.document().ok_or_else(|| "no document".to_string())?;

        let parts = js_sys::Array::new();
        parts.push(&js_sys::Uint8Array::from(bytes));
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type(mime);
        let blob =
            web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag).map_err(js_err)?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_err)?;

        let anchor: web_sys::HtmlAnchorElement = document
            .create_element("a")
            .map_err(js_err)?
            .dyn_into()
            .map_err(|_| "createElement('a') did not yield an anchor".to_string())?;
        anchor.set_href(&url);
        anchor.set_download(name);
        if let Some(body) = document.body() {
            body.append_child(anchor.as_ref()).map_err(js_err)?;
        }
        anchor.click();
        anchor.remove();
        web_sys::Url::revoke_object_url(&url).ok();
        Ok(())
    }
}

/// Host build without the `native` feature: there is no browser to hand a
/// download to, so the export path reports it instead of pretending.
#[cfg(all(not(feature = "native"), not(target_arch = "wasm32")))]
mod web {
    pub fn download(_name: &str, _mime: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("no browser in this build".to_string())
    }
}
