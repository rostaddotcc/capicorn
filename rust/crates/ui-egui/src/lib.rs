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
const FG_MUTED: Color32 = Color32::from_rgb(90, 97, 109);
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

/// W24: destructive-action ink, theme-aware (the live `--danger`).
fn pal_danger(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        DANGER_DARK
    } else {
        DANGER_LIGHT
    }
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

/// Interval-capture cadence choices (live `INTERVAL_CHOICES`, settings.ts:88).
pub const INTERVAL_CHOICES: [u32; 5] = [5, 10, 30, 60, 300];

/// `every 30 s` / `every 5 min` — the interval dropdown label.
fn interval_label(secs: u32) -> String {
    if secs >= 300 {
        format!("every {} min", secs / 60)
    } else {
        format!("every {secs} s")
    }
}

/// The fixed stamp the filename preview uses when the pattern field is not
/// focused. A live clock would make every snapshot differ run to run; while the
/// user types, the preview follows the real clock instead.
fn preview_stamp() -> chrono::DateTime<chrono::Local> {
    use chrono::TimeZone as _;
    chrono::Local
        .with_ymd_and_hms(2026, 11, 14, 19, 45, 12)
        .single()
        .unwrap_or_else(chrono::Local::now)
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
    /// Tesseract language code for `ocr.run` (`eng`, `swe`, …). W7.
    pub ocr_lang: String,
    /// Interval-capture cadence in seconds (1..=3600). W11.
    pub interval_secs: u32,
    /// Recording microphone source name (`None` = no microphone). W11.
    pub mic_source: Option<String>,
    /// Mix the desktop's own output into recordings. W11.
    pub system_audio: bool,
    /// Master switch for global-hotkey registration (off = PrintScreen only). W11.
    pub hotkeys_enabled: bool,
    /// `mp4` | `gif` — the Record button's container. W11.
    pub recording_format: String,
    /// Recordings get their own folder. W11.
    pub split_folders: bool,
    /// Recording folder override (`None` = follow `save_dir`). W11.
    pub recording_dir: Option<String>,
    /// W23: New-canvas dialog's remembered size/background (live prefs).
    pub new_canvas_w: u32,
    pub new_canvas_h: u32,
    pub new_canvas_bg: String,
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
            ocr_lang: "eng".to_string(),
            interval_secs: 30,
            mic_source: None,
            system_audio: false,
            hotkeys_enabled: false,
            recording_format: "mp4".to_string(),
            split_folders: false,
            recording_dir: None,
            new_canvas_w: 1280,
            new_canvas_h: 800,
            new_canvas_bg: "white".to_string(),
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
            // A hostile/invalid code falls back to `eng` here, so a bad file
            // can never make every `settings.set` from the panel fail.
            ocr_lang: match v.get("ocr_lang").and_then(|x| x.as_str()) {
                Some(lang) if snagcraft_io::settings::valid_ocr_lang(lang) => lang.to_string(),
                _ => d.ocr_lang,
            },
            interval_secs: v
                .get("interval_secs")
                .and_then(|x| x.as_u64())
                .map_or(d.interval_secs, |x| x.clamp(1, 3_600) as u32),
            // Blank/hostile device names fall back to `None` so the panel can
            // never push an invalid value that `settings.set` rejects.
            mic_source: v
                .get("mic_source")
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .filter(|src| {
                    !src.trim().is_empty()
                        && src.len() <= snagcraft_io::settings::MAX_MIC_SOURCE_LEN
                        && !src.chars().any(char::is_control)
                }),
            system_audio: b("system_audio", d.system_audio),
            hotkeys_enabled: b("hotkeys_enabled", d.hotkeys_enabled),
            recording_format: match v.get("recording_format").and_then(|x| x.as_str()) {
                Some(f) if snagcraft_io::settings::is_supported_recording_format(f) => {
                    f.to_string()
                }
                _ => d.recording_format,
            },
            split_folders: b("split_folders", d.split_folders),
            recording_dir: v
                .get("recording_dir")
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .filter(|dir| !dir.trim().is_empty()),
            new_canvas_w: v
                .get("new_canvas_w")
                .and_then(|x| x.as_u64())
                .filter(|n| (16..=8_192).contains(n))
                .map(|n| n as u32)
                .unwrap_or(d.new_canvas_w),
            new_canvas_h: v
                .get("new_canvas_h")
                .and_then(|x| x.as_u64())
                .filter(|n| (16..=8_192).contains(n))
                .map(|n| n as u32)
                .unwrap_or(d.new_canvas_h),
            new_canvas_bg: match v.get("new_canvas_bg").and_then(|x| x.as_str()) {
                Some(bg @ ("white" | "transparent")) => bg.to_string(),
                _ => d.new_canvas_bg,
            },
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
            "ocr_lang": self.ocr_lang,
            "interval_secs": self.interval_secs,
            "mic_source": self.mic_source,
            "system_audio": self.system_audio,
            "hotkeys_enabled": self.hotkeys_enabled,
            "recording_format": self.recording_format,
            "split_folders": self.split_folders,
            "recording_dir": self.recording_dir,
            "new_canvas_w": self.new_canvas_w,
            "new_canvas_h": self.new_canvas_h,
            "new_canvas_bg": self.new_canvas_bg,
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

/// W14: the rect of rect-based objects — these get select-mode resize handles.
fn obj_rect(o: &AnnotObject) -> Option<GRect> {
    match o {
        AnnotObject::Shape { rect, .. }
        | AnnotObject::Blur { rect, .. }
        | AnnotObject::Highlighter { rect, .. }
        | AnnotObject::Spotlight { rect, .. }
        | AnnotObject::Callout { rect, .. }
        | AnnotObject::Magnify { rect, .. } => Some(*rect),
        _ => None,
    }
}

/// W14: corners in tl, tr, bl, br order — corner `i` anchors at `3 - i`.
fn rect_corners(r: GRect) -> [Point; 4] {
    [
        Point::new(r.x, r.y),
        Point::new(r.x + r.w, r.y),
        Point::new(r.x, r.y + r.h),
        Point::new(r.x + r.w, r.y + r.h),
    ]
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
    Create {
        start: Point,
    },
    Move {
        id: u64,
        start: Point,
    },
    /// W14 select-mode resize: `anchor` is the corner opposite the dragged
    /// handle; the live rect spans anchor → pointer.
    Resize {
        id: u64,
        anchor: Point,
    },
}

struct TextEdit {
    pos: Point,
    buf: String,
    focus_requested: bool,
    /// W14: `Some(id)` re-edits an existing Text/Callout in place (set_text on
    /// commit, remove when emptied); `None` places a new caption.
    target: Option<u64>,
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

/// Compact 3-2-1 countdown shown before a delayed grab starts (B4#5).
///
/// Presentational only: the capture thread still sleeps `capture_delay_secs`,
/// this just paints what is about to happen. `deadline` is `None` for tests and
/// snapshots, which freezes the number so an image cannot tick mid-render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Countdown {
    /// Whole seconds still to show (3, 2, 1).
    pub remaining: u32,
    deadline: Option<Instant>,
}

impl Countdown {
    /// Count down from `seconds` in real time.
    pub fn running(seconds: u32) -> Self {
        Self {
            remaining: seconds.max(1),
            deadline: Some(Instant::now() + Duration::from_secs(1)),
        }
    }

    /// A frozen countdown (snapshot/test seam).
    pub fn frozen(seconds: u32) -> Self {
        Self {
            remaining: seconds,
            deadline: None,
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
    /// W18: when the current pause began (`None` = not paused). The clock
    /// freezes here while paused (live `paintRecTimer`, app.ts:2289-2292).
    pub paused_at: Option<Instant>,
    /// W18: total time spent paused, subtracted from the elapsed clock so a
    /// resumed take skips the paused span instead of jumping by it.
    pub paused_total: Duration,
    /// W18: the region this take records (`None` = whole screen/window) —
    /// the HUD's "Region" scope tag (live `recScopeEl`, app.ts:2308-2310).
    pub region: Option<GRect>,
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
            paused_at: None,
            paused_total: Duration::ZERO,
            region: None,
            format: "mp4".to_string(),
            audio: None,
            sources: Vec::new(),
            sources_loaded: false,
            output: None,
        }
    }
}

/// W20: classify a scrolling-capture failure exactly like the live
/// `scrollErrorText` (app.ts:2586-2599): locked screen, the 40 000 px height
/// cap, and screen-sharing refusals each get their own recovery hint.
pub fn scroll_error_status(err: &str) -> String {
    let raw = err.trim();
    let lower = raw.to_ascii_lowercase();
    if lower.contains("lock") {
        return "The screen is locked — unlock it, then start the scrolling capture again.".into();
    }
    if [
        "height",
        "too tall",
        "too high",
        "maximum",
        "max_height",
        "cap",
    ]
    .iter()
    .any(|key| lower.contains(key))
    {
        return "That would pass the 40 000 px height limit. Select a shorter region, or capture the page in two passes."
            .into();
    }
    if [
        "portal",
        "permission",
        "denied",
        "refus",
        "share",
        "screencast",
        "pipewire",
    ]
    .iter()
    .any(|key| lower.contains(key))
    {
        return "Screen sharing wasn't allowed. Approve the system prompt (or pick this window) and try again."
            .into();
    }
    format!("Scrolling capture failed: {raw}")
}

/// W18: the live recording filename — `SnagCraft-{date}-{time}.{format}`
/// (`recordingFilename`, app.ts:2368-2369), tokens expanded at start time.
pub fn recording_filename(dir: &Path, ext: &str) -> PathBuf {
    let stem = snagcraft_io::files::expand("SnagCraft-{date}-{time}", chrono::Local::now());
    dir.join(format!("{stem}.{ext}"))
}

/// W18: classify a failed `record.start` exactly like the live app
/// (app.ts:2481-2494): a user dismissal stays neutral, a permission/portal
/// refusal says how to recover, anything else points at the portal log
/// (the rebuild writes the same `~/.cache/snagcraft-portal.log`).
///
/// Order note: the rebuild's portal errors are wrapped ("screen cast portal:
/// … failed: … cancelled"), so the cancel check runs FIRST — the live app
/// received bare cancel strings and could test portal keywords first.
pub fn record_start_error_status(err: &str) -> String {
    let lower = err.to_ascii_lowercase();
    if lower.contains("cancel") {
        return "Recording cancelled — no screen was picked.".into();
    }
    if ["portal", "permission", "denied", "deny", "refus"]
        .iter()
        .any(|key| lower.contains(key))
    {
        return "Recording cancelled — screen sharing wasn't allowed. Approve the system prompt and try again."
            .into();
    }
    format!("Could not start recording: {err} — see ~/.cache/snagcraft-portal.log for details")
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
    /// Frame folder of the current/last run — where the stop-time MP4 lands
    /// (the live `dir`, `app.ts:3031`). `None` before the first start.
    pub dir: Option<PathBuf>,
    /// W15: the stop-time video encode is running on its own thread.
    pub assembling: bool,
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
            dir: None,
            assembling: false,
        }
    }
}

/// W15: the finished stop-time timelapse video (what `assemble_timelapse`
/// reports, owned so the worker channel needs no recording-crate type on web).
#[derive(Debug)]
struct TimelapseVideo {
    video: PathBuf,
    frames: u32,
    fps: u32,
}

/// OCR result window.
#[derive(Default)]
pub struct OcrState {
    pub open: bool,
    pub text: String,
    /// `ocr.run` is in flight on a worker thread: the window shows progress and
    /// Extract/Copy stay disabled until the result lands.
    pub busy: bool,
    /// `ocr.languages` cached after the first probe (`None` = not asked yet).
    pub languages: Option<Vec<String>>,
}

/// One tile in the Recent strip (W9): the file plus its lazily decoded thumb.
pub struct RecentItem {
    pub path: PathBuf,
    pub name: String,
    /// W22: when the capture landed (in-session push = now, folder scan =
    /// mtime) — the live tile's time caption (`clockLabel(item.ts)`).
    pub ts: SystemTime,
    /// Decoded on first paint; `None` = not tried yet or unreadable (the tile
    /// falls back to the film placeholder).
    pub thumb: Option<egui::TextureHandle>,
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
    /// Kittest/headless seam: when false a capture request is recorded in
    /// [`Self::last_capture`] instead of opening the portal picker, so a key
    /// dispatch can be asserted without a screen grab.
    pub capture_enabled: bool,
    /// What the last capture request asked for (key handler, tray, toolbar).
    pub last_capture: Option<CaptureKind>,
    /// Unsaved-annotation flag for the close guard (cleared by save/export/
    /// open/new/capture, set by any successful `edit.*`).
    pub doc_dirty: bool,
    /// Skip every guard on the way out (tray Quit / modal Discard).
    pub force_quit: bool,
    /// "Unsaved changes" modal visible.
    pub confirm_close: bool,
    pub settings_open: bool,
    /// New-canvas dialog (Ctrl+N / File → New…): eight presets + custom size.
    pub new_canvas_open: bool,
    pub new_canvas_w: f32,
    pub new_canvas_h: f32,
    /// W23: the dialog's background choice — "white" | "transparent".
    pub new_canvas_bg: String,
    /// Resize dialog (toolbar Resize…): prefilled with the current canvas.
    pub resize_open: bool,
    pub resize_w: f32,
    pub resize_h: f32,
    pub resize_error: Option<String>,
    /// Keyboard-shortcuts sheet (`?`, Help → Keyboard shortcuts): the one
    /// place where every real binding is listed.
    pub shortcuts_open: bool,
    /// About modal (File → About SnagCraft…).
    pub about_open: bool,
    /// Canvas view zoom on top of the fit-to-window scale (Ctrl+scroll).
    /// 1.0 is the untouched fit, so a fresh session renders byte-identically.
    pub view_zoom: f32,
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
    /// W25: in-app monitor picker (Windows/macOS, where no OS picker exists).
    /// `monitors` is the `capture.monitors` list; the dialog opens only when it
    /// is non-empty. Native-only — the web build has no screen capture.
    #[cfg(feature = "native")]
    pub monitor_picker_open: bool,
    #[cfg(feature = "native")]
    pub monitors: Vec<snagcraft_capture::MonitorInfo>,
    /// W4d live layer.
    pub rec: RecordState,
    pub scroll: ScrollState,
    pub timelapse: TimelapseState,
    pub ocr: OcrState,
    /// `ocr.available` cached: it probes for the tesseract binary.
    pub ocr_available: Option<bool>,
    /// Recent strip (W9): in-session captures/saves plus one folder scan.
    pub recent: Vec<RecentItem>,
    /// Strip visible — the toolbar chip (and the strip's Hide) toggle it.
    pub film_open: bool,
    /// W22: the recent tile whose file is the open document — the live
    /// `loadedRecentId` accent highlight (`film-item.is-active`).
    pub recent_active: Option<PathBuf>,
    /// The startup scan of the save folder has already run.
    film_scanned: bool,
    /// Compact 3-2-1 countdown before a delayed capture starts (B4#5).
    pub countdown: Option<Countdown>,
    /// Off-thread `ocr.run`: the worker sends the extracted text (or its error).
    ocr_rx: Option<mpsc::Receiver<Result<String, String>>>,
    /// W21: off-thread upload — the worker sends the returned link (or error).
    /// `Some` = an upload is in flight (one at a time).
    upload_rx: Option<mpsc::Receiver<Result<String, String>>>,
    /// W15: stop-time timelapse encode worker result (native only; the web
    /// build has no ffmpeg and never arms a timelapse).
    timelapse_assemble_rx: Option<mpsc::Receiver<Result<TimelapseVideo, String>>>,
    pick: Option<RegionPick>,
    pick_rx: Option<mpsc::Receiver<Result<Surface, String>>>,
    /// Web open path: in-flight browser file read (`w`, `h`) or its error.
    #[cfg(not(feature = "native"))]
    drop_rx: Option<mpsc::Receiver<Result<(u32, u32), String>>>,
    /// W16: in-flight browser clipboard op (ClipboardItem write/read).
    #[cfg(not(feature = "native"))]
    web_clipboard_rx: Option<mpsc::Receiver<Result<WebClipboard, String>>>,
    /// W28: off-thread video-thumbnail extraction for the Recent rail (ffmpeg
    /// first frame, live `recording_thumb`). The worker decodes to a Surface;
    /// the UI thread loads the texture. `pending` guards one request per file.
    #[cfg(feature = "native")]
    video_thumb_tx: Option<mpsc::Sender<(PathBuf, String)>>,
    #[cfg(feature = "native")]
    video_thumb_rx: Option<mpsc::Receiver<(PathBuf, Option<Surface>)>>,
    #[cfg(feature = "native")]
    video_thumb_pending: std::collections::HashSet<PathBuf>,
}

/// W16: the outcome of an async browser clipboard operation.
#[cfg(not(feature = "native"))]
#[derive(Debug)]
enum WebClipboard {
    /// PNG written to the system clipboard (`w`×`h` of the composite).
    Copied { w: u32, h: u32 },
    /// Image read from the clipboard and loaded as the new base.
    Pasted { w: u32, h: u32 },
    /// The clipboard holds no image (text or empty) — not an error.
    Empty,
}

/// Canvas zoom range for Ctrl+scroll: 25 %–400 % of the fit-to-window view.
const VIEW_ZOOM_MIN: f32 = 0.25;
const VIEW_ZOOM_MAX: f32 = 4.0;

/// Project home, shown in About (links are inert text — nothing is fetched).
const REPO_URL: &str = "https://github.com/rostaddotcc/snagcraft";

/// Release string: the crate version the binary was built from.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every binding the shortcuts sheet lists. Kept honest by `handle_keys`: a row
/// here that no handler consumes would be a lie, and the sheet is the only
/// documentation a user reads.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl + scroll", "Zoom the canvas"),
    ("Ctrl + N", "New canvas"),
    ("Ctrl + O", "Open an image"),
    ("Ctrl + S", "Save to the save folder"),
    ("Ctrl + Shift + S", "Save As…"),
    ("Ctrl + Z / Ctrl + Shift + Z", "Undo / redo"),
    ("Ctrl + C / Ctrl + V", "Copy / paste image"),
    ("Ctrl + D", "Duplicate selection"),
    ("Delete / Backspace", "Delete selection"),
    ("Ctrl + ,", "Settings"),
    ("Ctrl + .", "Stop a running timelapse"),
    ("Ctrl + Alt + V", "Start / stop a recording"),
    ("PrintScreen", "Capture the full screen"),
    ("Shift + PrintScreen", "Capture a region"),
    ("V C A T R B G E L H K S M", "Tool letters"),
    ("?", "This sheet"),
    ("Esc", "Close a dialog / cancel"),
];

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
            capture_enabled: true,
            last_capture: None,
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
            shortcuts_open: false,
            about_open: false,
            view_zoom: 1.0,
            settings_open: false,
            new_canvas_open: false,
            new_canvas_w: 1280.0,
            new_canvas_h: 800.0,
            new_canvas_bg: "white".to_string(),
            resize_open: false,
            resize_w: 0.0,
            resize_h: 0.0,
            resize_error: None,
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
            #[cfg(feature = "native")]
            monitor_picker_open: false,
            #[cfg(feature = "native")]
            monitors: Vec::new(),
            rec: RecordState::default(),
            scroll: ScrollState::default(),
            timelapse: TimelapseState::default(),
            ocr: OcrState::default(),
            ocr_available: None,
            recent: Vec::new(),
            film_open: false,
            recent_active: None,
            film_scanned: false,
            countdown: None,
            ocr_rx: None,
            upload_rx: None,
            timelapse_assemble_rx: None,
            pick: None,
            pick_rx: None,
            #[cfg(not(feature = "native"))]
            drop_rx: None,
            #[cfg(not(feature = "native"))]
            web_clipboard_rx: None,
            #[cfg(feature = "native")]
            video_thumb_tx: None,
            #[cfg(feature = "native")]
            video_thumb_rx: None,
            #[cfg(feature = "native")]
            video_thumb_pending: std::collections::HashSet::new(),
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
            if self.capture_rx.is_some() || !self.capture_enabled {
                return;
            }
            let (tx, rx) = mpsc::channel();
            self.capture_rx = Some(rx);
            // The compact HUD mirrors the thread's sleep; the thread stays the
            // authority on when the grab really happens.
            self.countdown = (delay_secs > 0).then(|| Countdown::running(delay_secs));
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
        self.last_capture = Some(CaptureKind::Full);
        self.start_capture();
    }

    /// W25: is a screen grab in flight? (test seam for the monitor picker)
    pub fn capture_in_flight(&self) -> bool {
        self.capture_rx.is_some()
    }

    /// Capture and switch straight into the crop tool (Region button/hotkey).
    pub fn capture_region(&mut self) {
        self.last_capture = Some(CaptureKind::Region);
        self.select_tool(Tool::Crop);
        self.start_capture();
    }

    /// W25: Capture → "Choose screen…". Query the connected outputs; where the
    /// OS provides no picker (Windows/macOS) open the in-app monitor picker.
    /// On Linux the list is empty and the system portal picker is the selector.
    #[cfg(feature = "native")]
    pub fn open_monitor_picker(&mut self) {
        match self.exec("capture.monitors", serde_json::json!({})) {
            Ok(v) => {
                let monitors: Vec<snagcraft_capture::MonitorInfo> =
                    serde_json::from_value(v["monitors"].clone()).unwrap_or_default();
                if monitors.is_empty() {
                    self.status =
                        "Monitor selection uses the system picker here — start a recording or scrolling capture to choose a screen."
                            .into();
                } else {
                    self.monitors = monitors;
                    self.monitor_picker_open = true;
                }
                self.redraw = true;
            }
            Err(err) => self.status = format!("Cannot list monitors: {err}"),
        }
    }

    /// W25: capture the monitor the user picked, through the same off-thread
    /// pipeline as a full capture (autosave + Recent + load come for free).
    #[cfg(feature = "native")]
    pub fn capture_monitor(&mut self, index: usize) {
        self.monitor_picker_open = false;
        if self.capture_rx.is_some() || !self.capture_enabled {
            return;
        }
        self.last_capture = Some(CaptureKind::Full);
        let (tx, rx) = mpsc::channel();
        self.capture_rx = Some(rx);
        self.status = "Capturing the selected monitor…".into();
        self.redraw = true;
        std::thread::spawn(move || {
            let result = snagcraft_capture::capture_monitor_index(index).map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn poll_capture(&mut self) {
        let Some(rx) = &self.capture_rx else { return };
        match rx.try_recv() {
            Ok(Ok(surface)) => {
                let (w, h) = (surface.width(), surface.height());
                self.capture_rx = None;
                self.countdown = None;
                // Autosave a copy first: the document below is a separate
                // in-memory surface, so this never touches the original file.
                let autosaved = self.autosave_capture(&surface);
                if let Ok(Some(path)) = &autosaved {
                    self.push_recent(path.clone());
                }
                // W22: the fresh capture is the loaded recent (live
                // loadedRecentId); an unsaved capture highlights nothing.
                self.recent_active = autosaved.as_ref().ok().and_then(|p| p.clone());
                self.sess().load_surface(surface);
                self.redraw = true;
                // A fresh capture starts a clean document.
                self.doc_dirty = false;
                self.selected = None;
                // W19: all three auto-save outcomes stay visible (live
                // autoSave wording semantics, app.ts:1837-1855).
                self.status = match autosaved {
                    Ok(Some(path)) => format!(
                        "Captured {w}×{h} — auto-saved to {} — annotate away.",
                        path.display()
                    ),
                    Ok(None) => {
                        format!("Captured {w}×{h} — auto-save is off, not saved to disk.")
                    }
                    Err(err) => {
                        format!("Captured {w}×{h} — kept in the editor — auto-save failed: {err}")
                    }
                };
            }
            Ok(Err(e)) => {
                self.capture_rx = None;
                self.countdown = None;
                self.status = format!("Capture failed: {e}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.capture_rx = None;
                self.countdown = None;
                self.status = "Capture thread ended unexpectedly.".into();
            }
        }
    }

    fn dispatch(&mut self, cmd: &str, params: serde_json::Value) {
        match self.exec(cmd, params) {
            Ok(_) => {
                self.redraw = true;
                // Dirty tracking is the engine's: history depth vs the last
                // save point, so undo back to the saved state is clean again
                // (a hand-set flag could never do that).
                let dirty = self.sess().dirty();
                self.doc_dirty = dirty;
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

    /// Settings → Troubleshooting: copy the recording diagnostics to the
    /// clipboard (`diagnostics.copy`).
    #[cfg(feature = "native")]
    pub fn copy_diagnostics(&mut self) {
        match self.exec("diagnostics.copy", serde_json::json!({})) {
            Ok(v) => {
                let bytes = v["bytes"].as_u64().unwrap_or(0);
                self.status =
                    format!("Diagnostics copied ({bytes} bytes) — paste them into a bug report.");
            }
            Err(err) => self.status = format!("Diagnostics: {err}"),
        }
    }

    /// Web fallback: diagnostics read host logs and put them on the system
    /// clipboard, neither of which exists in a browser tab.
    #[cfg(not(feature = "native"))]
    pub fn copy_diagnostics(&mut self) {
        self.status = "Recording diagnostics are desktop-only.".into();
    }

    /// W16 web copy: render the same composite Export uses, encode it as PNG
    /// (the clipboard interchange format) and write it through the async
    /// Clipboard API (`ClipboardItem`). The write needs user activation — a
    /// button click or Ctrl+C qualifies — and clipboard-write permission;
    /// failures surface with the Export fallback hint.
    #[cfg(all(not(feature = "native"), target_arch = "wasm32"))]
    fn copy_to_clipboard(&mut self) {
        if self.web_clipboard_rx.is_some() {
            return; // one clipboard op at a time
        }
        let watermark = self.watermark_value();
        let rendered = self
            .sess()
            .render_watermarked(watermark.as_ref())
            .map_err(|e| e.to_string());
        let surface = match rendered {
            Ok(surface) => surface,
            Err(err) => {
                self.status = format!("Copy failed: {err}");
                return;
            }
        };
        let (w, h) = (surface.width(), surface.height());
        let bytes = match snagcraft_io::encode(
            &surface,
            snagcraft_io::ImageFormat::Png,
            self.settings.default_quality,
        ) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.status = format!("Copy failed: {err}");
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        self.web_clipboard_rx = Some(rx);
        self.status = "Copying to clipboard…".into();
        let repaint = self.main_ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = web::clipboard_write_png(&bytes)
                .await
                .map(|()| WebClipboard::Copied { w, h });
            let _ = tx.send(result);
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    /// Host build without a browser: no Clipboard API to call.
    #[cfg(all(not(feature = "native"), not(target_arch = "wasm32")))]
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

    /// W16 web paste: read the clipboard through the async Clipboard API
    /// (`clipboard.read()` — the browser shows its permission prompt), load
    /// the first image/png as the new base, exactly like the dropped-file
    /// path. Text-only clipboards report "no image", never an error.
    #[cfg(all(not(feature = "native"), target_arch = "wasm32"))]
    fn paste_from_clipboard(&mut self) {
        if self.web_clipboard_rx.is_some() {
            return; // one clipboard op at a time
        }
        let (tx, rx) = mpsc::channel();
        self.web_clipboard_rx = Some(rx);
        self.status = "Reading clipboard…".into();
        let session = Arc::clone(&self.session);
        let repaint = self.main_ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match web::clipboard_read_png().await {
                Ok(Some(bytes)) => {
                    load_image_bytes(&session, &bytes).map(|(w, h)| WebClipboard::Pasted { w, h })
                }
                Ok(None) => Ok(WebClipboard::Empty),
                Err(err) => Err(err),
            };
            let _ = tx.send(result);
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    /// Host build without a browser: no Clipboard API to call.
    #[cfg(all(not(feature = "native"), not(target_arch = "wasm32")))]
    fn paste_from_clipboard(&mut self) {
        self.status = "Clipboard paste is desktop-only — drop an image file here instead.".into();
    }

    /// W16: collect the async clipboard result; every terminal path clears
    /// the channel so the next Copy/Paste can run.
    #[cfg(not(feature = "native"))]
    fn poll_web_clipboard(&mut self) {
        let Some(rx) = self.web_clipboard_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(WebClipboard::Copied { w, h })) => {
                self.web_clipboard_rx = None;
                self.status = format!("Copied {w}×{h} image to clipboard — paste anywhere.");
            }
            Ok(Ok(WebClipboard::Pasted { w, h })) => {
                self.web_clipboard_rx = None;
                self.redraw = true;
                self.doc_dirty = true;
                self.selected = None;
                self.status = format!("Pasted {w}×{h} from clipboard.");
            }
            Ok(Ok(WebClipboard::Empty)) => {
                self.web_clipboard_rx = None;
                self.status = "Clipboard has no image to paste.".into();
            }
            Ok(Err(err)) => {
                self.web_clipboard_rx = None;
                self.status =
                    format!("Clipboard: {err} — use Export or drop an image file instead.");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.web_clipboard_rx = None;
                self.status = "Clipboard operation stopped unexpectedly.".into();
            }
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
        if let Some(id) = edit.target {
            // W14 re-edit: emptying the box removes the object (live settle
            // parity — an empty caption is not content).
            if text.is_empty() {
                self.dispatch("edit.remove", serde_json::json!({"id": id}));
            } else {
                self.dispatch("edit.set_text", serde_json::json!({"id": id, "text": text}));
            }
            return;
        }
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
            self.push_recent(path);
            return true;
        }
        false
    }

    /// Web export: `doc.export` writes through `std::fs`, so the browser path
    /// renders the composite in memory, encodes it with `snagcraft-io` (the
    /// codecs are platform-free) and hands the bytes to the browser as a
    /// download via a Blob object URL. The watermark is applied through the
    /// engine's `render_watermarked` — identical to the desktop `doc.export`.
    #[cfg(not(feature = "native"))]
    pub fn export_dialog(&mut self) -> bool {
        let (format, ext, mime) = match self.settings.default_format.as_str() {
            "jpeg" => (snagcraft_io::ImageFormat::Jpeg, "jpg", "image/jpeg"),
            "webp" => (snagcraft_io::ImageFormat::WebP, "webp", "image/webp"),
            _ => (snagcraft_io::ImageFormat::Png, "png", "image/png"),
        };
        let quality = self.settings.default_quality;
        let watermark = self.watermark_value();
        let watermarked = watermark.is_some();
        let rendered = {
            let guard = self
                .session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .render_watermarked(watermark.as_ref())
                .map_err(|e| e.to_string())
        };
        let bytes = rendered
            .and_then(|img| snagcraft_io::encode(&img, format, quality).map_err(|e| e.to_string()));
        match bytes {
            Ok(bytes) => {
                let name = format!("snagcraft.{ext}");
                match web::download(&name, mime, &bytes) {
                    Ok(()) => {
                        self.status = if watermarked {
                            format!("Exported {name} ({} bytes, watermarked).", bytes.len())
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

    // ---- W9: recent strip ---------------------------------------------------

    /// Record `path` as the newest recent capture. Deduped by path, capped at
    /// [`snagcraft_io::recent::RECENT_LIMIT`], newest first (`app.ts:3907`).
    pub fn push_recent(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        self.recent.retain(|item| item.path != path);
        self.recent.insert(
            0,
            RecentItem {
                path,
                name,
                ts: SystemTime::now(),
                thumb: None,
            },
        );
        self.recent.truncate(snagcraft_io::recent::RECENT_LIMIT);
    }

    /// Number of recents the chip's count badge shows.
    pub fn recent_count(&self) -> usize {
        self.recent.len()
    }

    /// W28: request an off-thread first-frame thumbnail for a video recent item
    /// (live `recording_thumb`). Lazily spawns one persistent worker; `pending`
    /// guards a single in-flight request per file. Images stay on the fast
    /// synchronous `load_thumb` path — only video containers come here.
    #[cfg(feature = "native")]
    pub fn request_video_thumb(&mut self, path: PathBuf, name: String) {
        if self.video_thumb_pending.contains(&path) {
            return;
        }
        if self.video_thumb_tx.is_none() {
            let (tx, rx) = mpsc::channel::<(PathBuf, String)>();
            let (out_tx, out_rx) = mpsc::channel::<(PathBuf, Option<Surface>)>();
            self.video_thumb_tx = Some(tx);
            self.video_thumb_rx = Some(out_rx);
            std::thread::spawn(move || {
                while let Ok((p, _name)) = rx.recv() {
                    let surf = snagcraft_io::recent::video_thumbnail(&p, 64);
                    if out_tx.send((p, surf)).is_err() {
                        break;
                    }
                }
            });
        }
        self.video_thumb_pending.insert(path.clone());
        if let Some(tx) = &self.video_thumb_tx {
            let _ = tx.send((path, name));
        }
    }

    /// W28: drain finished video thumbnails, load their textures and set the
    /// recent tiles. A `None` surface (ffmpeg absent/unreadable) leaves the film
    /// placeholder. Called from the frame loop alongside the other workers.
    #[cfg(feature = "native")]
    pub fn poll_video_thumbs(&mut self, ctx: &egui::Context) {
        let mut done = Vec::new();
        if let Some(rx) = &self.video_thumb_rx {
            while let Ok(item) = rx.try_recv() {
                done.push(item);
            }
        }
        for (path, surf) in done {
            self.video_thumb_pending.remove(&path);
            if let Some(surf) = surf {
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [surf.width() as usize, surf.height() as usize],
                    surf.as_raw(),
                );
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let tex = ctx.load_texture(
                    format!("recent-{name}"),
                    color,
                    egui::TextureOptions::LINEAR,
                );
                if let Some(item) = self.recent.iter_mut().find(|r| r.path == path) {
                    item.thumb = Some(tex);
                }
            }
            self.redraw = true;
        }
    }

    /// Merge the save folder's images into the strip, once per session. The
    /// desktop scans `std::fs`; the web build has no folder to scan, so it only
    /// ever shows this session's captures.
    fn scan_recent_folder(&mut self) {
        if self.film_scanned {
            return;
        }
        self.film_scanned = true;
        #[cfg(feature = "native")]
        {
            let dir = self.save_dir();
            for entry in snagcraft_io::recent::scan_recent(&dir, snagcraft_io::recent::RECENT_LIMIT)
            {
                if self.recent.len() >= snagcraft_io::recent::RECENT_LIMIT {
                    break;
                }
                if self.recent.iter().any(|item| item.path == entry.path) {
                    continue;
                }
                self.recent.push(RecentItem {
                    path: entry.path,
                    name: entry.name,
                    ts: entry.mtime,
                    thumb: None,
                });
            }
        }
    }

    /// W19: the live Save (`saveEdited`, app.ts:1862-1885) — guards first
    /// ("Nothing to save yet" / "No changes to save"), then a *new* flattened
    /// copy with the `-edited` suffix (never overwrites; `save_in_dir`
    /// dedups), status "Saved copy {filename}", save point moves, Recent
    /// pickup. Returns whether a file landed (the close guard waits on it).
    pub fn save_now(&mut self) -> bool {
        #[cfg(not(feature = "native"))]
        {
            // No filesystem in a browser tab: Export downloads the same image.
            self.export_dialog()
        }
        #[cfg(feature = "native")]
        {
            if self.sess().doc().is_none() {
                self.status = "Nothing to save yet — capture a screen first.".into();
                return false;
            }
            if !self.doc_dirty {
                self.status = "No changes to save".into();
                return false;
            }
            let mut params = serde_json::json!({"suffix": "-edited"});
            if let Some(watermark) = self.watermark_value() {
                params["watermark"] = watermark;
            }
            match self.exec("doc.save", params) {
                Ok(value) => {
                    let saved = value["saved"].as_str().unwrap_or_default().to_string();
                    let filename = std::path::Path::new(&saved)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| saved.clone());
                    self.status = format!("Saved copy {filename}");
                    self.doc_dirty = false;
                    self.redraw = true;
                    if !saved.is_empty() {
                        // W22: the saved copy is now the open document.
                        self.recent_active = Some(PathBuf::from(&saved));
                        self.push_recent(PathBuf::from(saved));
                    }
                    true
                }
                Err(err) => {
                    self.status = format!("Save failed: {err}");
                    false
                }
            }
        }
    }

    /// Save As… (Ctrl+Shift+S / File → Save As…): flatten to a chosen path.
    #[cfg(feature = "native")]
    pub fn save_as_dialog(&mut self) -> bool {
        let format = self.settings.default_format.clone();
        let (image_format, filter, exts): (snagcraft_io::ImageFormat, &str, Vec<&str>) =
            match format.as_str() {
                "jpeg" => (snagcraft_io::ImageFormat::Jpeg, "JPEG", vec!["jpeg", "jpg"]),
                "webp" => (snagcraft_io::ImageFormat::WebP, "WebP", vec!["webp"]),
                _ => (snagcraft_io::ImageFormat::Png, "PNG", vec!["png"]),
            };
        let suggested =
            snagcraft_io::files::build_filename_now(&self.settings.filename_pattern, image_format);
        let dir = self.ensure_save_dir();
        let Some(path) = rfd::FileDialog::new()
            .set_directory(&dir)
            .set_file_name(&suggested)
            .add_filter(filter, &exts)
            .save_file()
        else {
            // Live app.ts:1908-1910: a dismissed dialog is reported, not silent.
            self.status = "Save As cancelled".into();
            return false;
        };
        self.save_as_to(path)
    }

    /// Headless seam for Save As…: the dialog (or a test) hands over the path,
    /// this runs `doc.save_as` and reports `Saved <path>`.
    pub fn save_as_to(&mut self, path: PathBuf) -> bool {
        let format = self.settings.default_format.clone();
        let quality = self.settings.default_quality;
        let mut params = serde_json::json!({
            "path": path.to_string_lossy(),
            "format": format,
            "quality": quality,
        });
        if let Some(watermark) = self.watermark_value() {
            params["watermark"] = watermark;
        }
        match self.exec("doc.save_as", params) {
            Ok(_) => {
                self.status = format!("Saved {}", path.display());
                self.doc_dirty = false;
                self.redraw = true;
                self.recent_active = Some(path.clone());
                self.push_recent(path);
                true
            }
            Err(err) => {
                self.status = format!("Save As failed: {err}");
                false
            }
        }
    }

    /// Web fallback: the browser cannot pick a filesystem path — Export
    /// downloads the same flattened image instead.
    #[cfg(not(feature = "native"))]
    pub fn save_as_dialog(&mut self) -> bool {
        self.status = "Save As is desktop-only — use Export to download a PNG.".into();
        false
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
        // Live app.ts:2402-2405: a capture in flight blocks a new take.
        #[cfg(feature = "native")]
        if self.capture_rx.is_some() || self.scroll.armed {
            self.status = "Wait for the capture to finish before recording.".into();
            return;
        }
        // W11: the separate recording folder and the persisted container are
        // what the Record button uses; `rec.format`/`rec.audio` stay the
        // in-session overrides when a picker set them.
        let dir = match (&self.settings.recording_dir, self.settings.split_folders) {
            (Some(dir), true) if !dir.trim().is_empty() => std::path::PathBuf::from(dir),
            _ => self.ensure_save_dir(),
        };
        let format = if self.rec.format.is_empty() {
            self.settings.recording_format.clone()
        } else {
            self.rec.format.clone()
        };
        let ext = if format == "gif" { "gif" } else { "mp4" };
        // W18: the live recording filename (app.ts:2368-2369), not an epoch.
        let path = recording_filename(&dir, ext);
        let mut params = serde_json::json!({
            "format": ext,
            "output": path.to_string_lossy(),
        });
        if let Some(source) = self
            .rec
            .audio
            .clone()
            .or_else(|| self.settings.mic_source.clone())
        {
            params["audio_source"] = serde_json::json!(source);
        }
        // W13: the settings toggle joins the mic in one mixed AAC track.
        params["system_audio"] = serde_json::json!(self.settings.system_audio);
        if let Some(rect) = region {
            params["region"] = rect_json(rect);
        }
        match self.exec("record.start", params) {
            Ok(_) => {
                self.rec.armed = true;
                self.rec.active = false;
                self.rec.paused = false;
                self.rec.started_at = None;
                self.rec.paused_at = None;
                self.rec.paused_total = Duration::ZERO;
                self.rec.region = region;
                self.rec.output = Some(path.display().to_string());
                // Live app.ts:2439-2444: the waiting status names the format
                // and the region size when one was picked.
                let fmt = if ext == "gif" { "GIF" } else { "MP4" };
                self.status = match region {
                    Some(r) => format!(
                        "Starting {fmt} region recording — {} × {}…",
                        r.w.round() as i64,
                        r.h.round() as i64
                    ),
                    None => format!("Starting {fmt} recording — choose a screen…"),
                };
                self.redraw = true;
            }
            Err(err) => self.status = record_start_error_status(&err),
        }
    }

    /// `record.stop` — the encoder finalises and reports `stopped`.
    pub fn stop_recording(&mut self) {
        match self.exec("record.stop", serde_json::json!({})) {
            // Live app.ts:2501 wording.
            Ok(_) => self.status = "Finishing the recording…".into(),
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
        self.rec.paused_at = None;
        self.rec.paused_total = Duration::ZERO;
        self.rec.region = None;
    }

    /// Elapsed time of the running take (from the `started` event).
    /// W18: paused takes freeze the clock at the pause moment; resumed takes
    /// skip the paused spans (live `paintRecTimer`, app.ts:2289-2292).
    pub fn record_elapsed(&self) -> Duration {
        let Some(start) = self.rec.started_at else {
            return Duration::ZERO;
        };
        let end = self.rec.paused_at.unwrap_or_else(Instant::now);
        end.duration_since(start)
            .saturating_sub(self.rec.paused_total)
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
        // W20: the live scrolling result is saved under the normal capture
        // filename pattern (writeImage, app.ts:2776), not an epoch name.
        let name = snagcraft_io::files::build_filename_now(
            &self.settings.filename_pattern,
            snagcraft_io::ImageFormat::Png,
        );
        let stem = name.strip_suffix(".png").unwrap_or(&name);
        let path = dedup_path(&dir, stem, "png");
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
            Err(err) => self.status = scroll_error_status(&err),
        }
    }

    /// Enter / Stop: stitch what was captured and open it in the editor.
    pub fn stop_scrolling(&mut self) {
        match self.exec("scroll.stop", serde_json::json!({})) {
            Ok(_) => self.status = "Finishing the scrolling capture…".into(),
            Err(err) => self.status = format!("Scroll stop: {err}"),
        }
    }

    /// W20: Cancel — discard the partial scroll, write nothing (the live
    /// card's Cancel button, `scrolling_cancel`, app.ts:2757-2763).
    pub fn cancel_scrolling(&mut self) {
        match self.exec("scroll.cancel", serde_json::json!({})) {
            Ok(_) => {
                self.scroll.armed = false;
                self.scroll.active = false;
                self.status = "Scrolling capture cancelled".into();
                self.redraw = true;
            }
            Err(err) => self.status = format!("Scroll cancel: {err}"),
        }
    }

    /// W20: a finished scroll — open it, pick it up in Recent, and report the
    /// live summary ("Scrolling capture: W × H, N frames · saved {filename}",
    /// app.ts:2780-2786). The engine already wrote the file before `done`.
    pub fn scroll_done(&mut self, path: &str, width: u32, height: u32) {
        self.scroll.armed = false;
        self.scroll.active = false;
        if path.is_empty() {
            self.status = "Scrolling capture finished with no image.".into();
            return;
        }
        let frames = self.scroll.index.max(1);
        self.dispatch("doc.open", serde_json::json!({"path": path}));
        self.selected = None;
        let filename = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        self.status =
            format!("Scrolling capture: {width} × {height}, {frames} frames · saved {filename}");
        self.recent_active = Some(PathBuf::from(path));
        self.push_recent(PathBuf::from(path));
        self.redraw = true;
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
                self.timelapse.dir = Some(dir.clone());
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

    /// W15 — a finished run becomes a video, exactly like the live app
    /// (`app.ts:3019-3047`): zero shots report that nothing was saved; any
    /// shots trigger `make_timelapse` (10 fps) into the run's own folder.
    /// A failed render never costs the shots — they stay on disk for a retry
    /// via `timelapse.assemble` (`src-tauri/src/timelapse.rs:1-6`).
    pub fn finish_timelapse(&mut self, frames: u64) {
        if frames == 0 {
            self.status =
                "Interval capture stopped before the first shot — nothing was saved.".into();
            return;
        }
        let Some(dir) = self.timelapse.dir.clone() else {
            // No run folder known (never started here): report, don't guess.
            self.status = format!("Timelapse finished: {frames} frames.");
            return;
        };
        self.status = format!("Building a timelapse from {frames} shots…");
        self.redraw = true;
        self.start_timelapse_assemble(dir, frames);
    }

    /// Encode the run's frames off-thread. The pure `assemble_timelapse` call
    /// needs no session lock, so the editor keeps painting during the encode
    /// (unlike `ocr.run`, which holds the session for its subprocess).
    fn start_timelapse_assemble(&mut self, dir: PathBuf, frames: u64) {
        #[cfg(not(feature = "native"))]
        {
            let _ = (dir, frames);
            self.status =
                format!("Timelapse finished: {frames} frames — video assembly is desktop-only.");
        }
        #[cfg(feature = "native")]
        {
            if self.timelapse.assembling {
                return; // one encode at a time
            }
            // Live app.ts:3031-3036: SnagCraft-timelapse-{date}-{time}.mp4,
            // fps 10, glob = exactly the frames this engine writes.
            let out = dir.join(snagcraft_io::files::expand(
                "SnagCraft-timelapse-{date}-{time}.mp4",
                chrono::Local::now(),
            ));
            let pattern = snagcraft_capture::timelapse::FRAME_GLOB;
            let dir_label = dir.display().to_string();
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("snagcraft-timelapse-assemble".to_string())
                .spawn(move || {
                    let outcome = snagcraft_recording::assemble_timelapse(&dir, pattern, 10, &out)
                        .map(|report| TimelapseVideo {
                            video: report.video,
                            frames: report.frames,
                            fps: report.fps,
                        })
                        .map_err(|err| err.to_string());
                    let _ = tx.send(outcome);
                });
            match spawned {
                Ok(_) => {
                    self.timelapse.assembling = true;
                    self.timelapse_assemble_rx = Some(rx);
                }
                Err(err) => {
                    self.status = format!(
                        "Timelapse failed: cannot start the encoder ({err}) — the {frames} shots are still in {dir_label}"
                    );
                }
            }
        }
    }

    /// Collect the encode worker's result; every terminal path clears the
    /// busy flag so a later run can assemble again.
    fn poll_timelapse_assemble(&mut self) {
        let Some(rx) = self.timelapse_assemble_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(video)) => {
                self.timelapse.assembling = false;
                self.timelapse_assemble_rx = None;
                self.status = format!(
                    "Timelapse saved: {} ({} shots, {} fps)",
                    video.video.display(),
                    video.frames,
                    video.fps
                );
                self.push_recent(video.video);
                self.redraw = true;
            }
            Ok(Err(err)) => {
                self.timelapse.assembling = false;
                self.timelapse_assemble_rx = None;
                let dir = self
                    .timelapse
                    .dir
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_else(|| "the save folder".to_string());
                self.status = format!(
                    "Timelapse failed: {err} — the {} shots are still in {dir}",
                    self.timelapse.frames
                );
                self.redraw = true;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.timelapse.assembling = false;
                self.timelapse_assemble_rx = None;
                self.status =
                    "Timelapse encoder stopped unexpectedly — the shots are still in the save folder."
                        .into();
                self.redraw = true;
            }
        }
    }

    // ---- OCR / upload -------------------------------------------------------

    /// The OCR language the picker and `ocr.run` use (invalid → `eng`).
    pub fn selected_ocr_lang(&self) -> String {
        let lang = self.settings.ocr_lang.trim();
        if snagcraft_io::settings::valid_ocr_lang(lang) {
            lang.to_string()
        } else {
            "eng".to_string()
        }
    }

    /// `ocr.languages`, probed once and cached; `["eng"]` when tesseract (or
    /// the web core, which has no OCR command) cannot answer. The persisted
    /// language is always in the list so the picker can show it.
    pub fn ocr_languages(&mut self) -> Vec<String> {
        if let Some(languages) = &self.ocr.languages {
            return languages.clone();
        }
        let mut languages = self
            .exec("ocr.languages", serde_json::json!({}))
            .ok()
            .and_then(|v| v["languages"].as_array().cloned())
            .map(|list| {
                list.iter()
                    .filter_map(|code| code.as_str())
                    .filter(|code| snagcraft_io::settings::valid_ocr_lang(code))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if languages.is_empty() {
            languages.push("eng".to_string());
        }
        let selected = self.selected_ocr_lang();
        if !languages.contains(&selected) {
            languages.insert(0, selected);
        }
        self.ocr.languages = Some(languages.clone());
        languages
    }

    /// Persist the OCR language through `settings.set` (the engine owns it).
    pub fn set_ocr_lang(&mut self, lang: String) {
        if !snagcraft_io::settings::valid_ocr_lang(&lang) {
            self.status = format!("OCR language '{lang}' is not a valid code.");
            return;
        }
        let mut next = self.settings.clone();
        next.ocr_lang = lang;
        self.push_settings(next);
    }

    /// `ocr.run` on the current document, shown in the result window.
    ///
    /// Rendering the composite and tesseract both take long enough to stall a
    /// frame, so the call runs on a worker thread and the window shows progress
    /// until [`Self::poll_ocr`] collects the text (or the error).
    pub fn run_ocr(&mut self) {
        if self.ocr.busy {
            return; // one extraction at a time
        }
        // W21: the live guard (app.ts:4151-4154).
        if self.sess().doc().is_none() {
            self.status = "Capture something first.".into();
            return;
        }
        let lang = self.selected_ocr_lang();
        self.ocr.open = true;
        self.ocr.busy = true;
        self.ocr.text.clear(); // live clears the previous result on a new run
        self.status = format!("Reading text ({lang})…");
        self.redraw = true;
        let session = Arc::clone(&self.session);
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("snagcraft-ocr".to_string())
            .spawn(move || {
                // W21: render under a BRIEF lock, then run tesseract without
                // it — the toolbar locks the session every frame, so holding
                // the lock across the subprocess would freeze the editor.
                let rendered = {
                    let guard = session
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.render().map_err(|e| e.to_string())
                };
                let outcome = rendered.and_then(|img| {
                    snagcraft_io::ocr::run_ocr(&img, &lang).map_err(|e| e.to_string())
                });
                let _ = tx.send(outcome);
            });
        if let Err(err) = spawned {
            self.ocr.busy = false;
            self.ocr_rx = None;
            self.status = format!("OCR: cannot start the worker ({err})");
            return;
        }
        self.ocr_rx = Some(rx);
    }

    /// Collect the worker's result. Clears the busy flag on success, error and
    /// a dead worker alike, so the window can never stay stuck.
    fn poll_ocr(&mut self) {
        let Some(rx) = self.ocr_rx.as_ref() else {
            return;
        };
        let outcome = rx.try_recv();
        let pending = matches!(outcome, Err(std::sync::mpsc::TryRecvError::Empty));
        match outcome {
            Ok(outcome) => self.ocr_finished(outcome),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.ocr_finished(Err("the worker stopped without a result".into()));
            }
        }
        if !pending {
            self.redraw = true;
        }
    }

    /// W21: an OCR result — the live wording (app.ts:4163-4175): an empty
    /// extraction suggests another language, a non-empty one points at Copy.
    pub fn ocr_finished(&mut self, outcome: Result<String, String>) {
        self.ocr.busy = false;
        self.ocr_rx = None;
        match outcome {
            Ok(text) => {
                self.ocr.text = text;
                self.ocr.open = true;
                if self.ocr.text.is_empty() {
                    self.status = "No text found in this capture — try another language.".into();
                } else {
                    self.status = format!(
                        "Found {} characters — Copy puts them on the clipboard.",
                        self.ocr.text.chars().count()
                    );
                }
            }
            Err(err) => self.status = format!("OCR failed: {err}"),
        }
        self.redraw = true;
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

    /// W21: POST the current image to the configured endpoint off-thread and
    /// copy the link it replies with (File → Upload…). The render happens under
    /// a brief session lock; curl runs WITHOUT the lock so the editor stays
    /// responsive through the up-to-30 s upload window (live awaits async).
    ///
    /// The endpoint is the user's own `settings.upload_url`; nothing is posted
    /// anywhere until they press the item. Native only: the web build has no
    /// curl and no file system to stage from.
    #[cfg(feature = "native")]
    pub fn upload_image_now(&mut self) {
        if self.upload_rx.is_some() {
            return; // one upload at a time
        }
        if self.sess().doc().is_none() {
            self.status = "Nothing to upload — capture or open an image first.".into();
            return;
        }
        let Some(endpoint) = self.upload_target() else {
            self.status =
                format!("Set an upload URL in Settings — Export (e.g. {UPLOAD_URL_DEFAULT}).");
            // W21: the live app opens the settings panel here (app.ts:4507).
            self.settings_open = true;
            return;
        };
        self.status = "Uploading…".into();
        self.redraw = true;
        let session = Arc::clone(&self.session);
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("snagcraft-upload".to_string())
            .spawn(move || {
                // Render + encode under a BRIEF lock, then run curl without it
                // — the toolbar locks the session every frame, so a locked
                // subprocess would freeze the whole editor.
                let png = {
                    let guard = session
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard
                        .render_watermarked(None)
                        .map_err(|e| e.to_string())
                        .and_then(|img| {
                            snagcraft_io::encode(&img, snagcraft_io::ImageFormat::Png, 100)
                                .map_err(|e| e.to_string())
                        })
                };
                let outcome = png.and_then(|bytes| {
                    snagcraft_io::upload::upload_image(&endpoint, &bytes).map_err(|e| e.to_string())
                });
                let _ = tx.send(outcome);
            });
        match spawned {
            Ok(_) => self.upload_rx = Some(rx),
            Err(err) => {
                self.status = format!("Upload failed: cannot start the worker ({err})");
            }
        }
    }

    /// W21: collect the upload worker's result (poll_media, runs hidden too).
    fn poll_upload(&mut self) {
        let Some(rx) = self.upload_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => self.upload_finished(outcome),
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.upload_finished(Err("the worker stopped without a result".into()));
            }
        }
    }

    /// W21: an upload result — the live wording (app.ts:4512-4521): the link
    /// is copied and the status says so; an empty link is a failure.
    pub fn upload_finished(&mut self, outcome: Result<String, String>) {
        self.upload_rx = None;
        match outcome {
            Ok(link) => {
                let link = link.trim().to_string();
                if link.is_empty() {
                    self.status = "Upload failed: the endpoint returned no link.".into();
                } else {
                    if let Some(ctx) = &self.main_ctx {
                        ctx.copy_text(link.clone());
                    }
                    self.status = format!("Uploaded — link copied: {link}");
                }
            }
            Err(err) => self.status = format!("Upload failed: {err}"),
        }
        self.redraw = true;
    }

    /// Show the compact countdown at `seconds` without starting a grab
    /// (snapshot/test seam; the real path runs through `capture_delay_secs`).
    pub fn show_countdown(&mut self, seconds: u32) {
        self.countdown = Some(Countdown::frozen(seconds));
    }

    // ---- autosave -----------------------------------------------------------

    /// W3 gap: a fresh capture is written to the save folder when the setting is
    /// on. The document keeps its own copy — the file is a *copy*, never a move.
    /// W19: returns the copy, `Ok(None)` when the setting is off, or the failure
    /// reason — the caller composes the status so a failed auto-save is never
    /// swallowed by the "Captured…" message (live autoSave, app.ts:1837-1855).
    pub fn autosave_capture(&mut self, surface: &Surface) -> Result<Option<PathBuf>, String> {
        if !self.settings.autosave_captures {
            return Ok(None);
        }
        let dir = self.ensure_save_dir();
        let name = snagcraft_io::files::build_filename_now(
            &self.settings.filename_pattern,
            snagcraft_io::ImageFormat::Png,
        );
        let stem = name.strip_suffix(".png").unwrap_or(&name);
        let path = dedup_path(&dir, stem, "png");
        image::save_buffer(
            &path,
            surface.as_raw(),
            surface.width(),
            surface.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map(|()| Some(path))
        .map_err(|err| err.to_string())
    }

    // ---- heartbeat ----------------------------------------------------------

    /// Drain the recording/scroll/timelapse channels. Runs from `app_ui` *and*
    /// `logic` — eframe skips `ui` while the window is hidden, and a recording
    /// started from the widget must still report its `stopped` event.
    fn poll_media(&mut self) {
        self.poll_ocr();
        self.poll_pick();
        self.poll_upload();
        self.poll_timelapse_assemble();
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
                            // Live app.ts:2475 / :5312 wording.
                            self.status = "Recording — click Stop when you're done.".into();
                        }
                        Some("stopped") => {
                            let path = event["path"].as_str().unwrap_or_default().to_string();
                            self.record_stopped(&path);
                        }
                        Some("failed") => {
                            let msg = event["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            self.end_recording();
                            self.status = format!("Recording failed: {msg}");
                        }
                        Some("paused") => {
                            self.rec.paused = true;
                            // W18: freeze the clock at the pause moment.
                            self.rec.paused_at.get_or_insert_with(Instant::now);
                        }
                        Some("resumed") => {
                            self.rec.paused = false;
                            // W18: bank the paused span so the clock skips it.
                            if let Some(at) = self.rec.paused_at.take() {
                                self.rec.paused_total += at.elapsed();
                            }
                        }
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

    /// W18: a finished take — live `stopRecording` (app.ts:2505-2511): the
    /// saved file lands in the Recent rail immediately; an empty path is
    /// reported, never shown as a blank "saved".
    pub fn record_stopped(&mut self, path: &str) {
        self.end_recording();
        if path.is_empty() {
            self.status = "Recording stopped, but no file was written.".into();
        } else {
            self.status = format!("Recording saved: {path}");
            self.push_recent(PathBuf::from(path));
            self.redraw = true;
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
                            let width = event["width"].as_u64().unwrap_or(0) as u32;
                            let height = event["height"].as_u64().unwrap_or(0) as u32;
                            self.scroll_done(&path, width, height);
                        }
                        Some("failed") => {
                            let msg = event["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            self.scroll.armed = false;
                            self.scroll.active = false;
                            self.status = scroll_error_status(&msg);
                        }
                        Some("stopped") => {
                            self.scroll.armed = false;
                            self.scroll.active = false;
                            self.status = "Scrolling capture stopped.".into();
                        }
                        Some("cancelled") => {
                            self.scroll.armed = false;
                            self.scroll.active = false;
                            self.status = "Scrolling capture cancelled".into();
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
                            // W15: stop converts the run to MP4 (live parity).
                            self.finish_timelapse(frames);
                        }
                        Some("failed") => {
                            let msg = event["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string();
                            self.timelapse.armed = false;
                            self.timelapse.running = false;
                            // A failed run never costs the shots already on disk.
                            if self.timelapse.frames > 0
                                && let Some(dir) = &self.timelapse.dir
                            {
                                self.status = format!(
                                    "Timelapse failed: {msg} — the {} shots are still in {}",
                                    self.timelapse.frames,
                                    dir.display()
                                );
                            } else {
                                self.status = format!("Timelapse failed: {msg}");
                            }
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

    /// OCR window: language picker, an Extract button and the text. While
    /// `ocr.run` runs on its worker thread the body is a progress row instead of
    /// the text, and Extract/Copy are disabled — the document is never touched.
    fn ocr_window(&mut self, ctx: &egui::Context) {
        if !self.ocr.open {
            return;
        }
        let busy = self.ocr.busy;
        let ready = self.ocr_ready();
        let languages = self.ocr_languages();
        let mut lang = self.selected_ocr_lang();
        let mut picked: Option<String> = None;
        let mut open = true;
        let mut copy = false;
        let mut close = false;
        let mut extract = false;
        egui::Window::new("Extracted text")
            .open(&mut open)
            .resizable(true)
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Language");
                    egui::ComboBox::from_id_salt("ocr-language")
                        .width(170.0)
                        .selected_text(ocr_language_label(&lang))
                        .show_ui(ui, |ui| {
                            for code in &languages {
                                if ui
                                    .selectable_value(
                                        &mut lang,
                                        code.clone(),
                                        ocr_language_label(code),
                                    )
                                    .changed()
                                {
                                    picked = Some(code.clone());
                                }
                            }
                        });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let button = ui
                            .add_enabled(!busy && ready, egui::Button::new("Extract"))
                            .on_hover_text("Read the text in the current image");
                        if button.clicked() {
                            extract = true;
                        }
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
                if busy {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Reading text…");
                    });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("The image is only read, never changed.")
                            .size(11.0)
                            .weak(),
                    );
                } else {
                    let mut buf = self.ocr.text.clone();
                    ui.add(
                        egui::TextEdit::multiline(&mut buf)
                            .desired_rows(12)
                            .desired_width(f32::INFINITY)
                            .code_editor(),
                    );
                }
                ui.horizontal(|ui| {
                    // W21: live disables Copy until there is text (ocrCopyBtn).
                    // W24: Copy is the primary action (live btn-primary).
                    if ui
                        .add_enabled(
                            !busy && !self.ocr.text.is_empty(),
                            egui::Button::new(
                                egui::RichText::new("Copy text").color(Color32::WHITE),
                            )
                            .fill(pal_accent(ui)),
                        )
                        .clicked()
                    {
                        copy = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if let Some(code) = picked {
            self.set_ocr_lang(code);
        }
        if extract {
            self.run_ocr();
        }
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
                    egui::RichText::new(
                        "Grab a frame on a timer — stopping builds a 10 fps video.",
                    )
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

/// The live New-canvas sizes (`app.ts:4931` NEW_PRESETS) — the eight buttons.
const NEW_CANVAS_PRESETS: [(&str, u32, u32); 8] = [
    ("Default", 1280, 800),
    ("HD", 1280, 720),
    ("Full HD", 1920, 1080),
    ("QHD", 2560, 1440),
    ("4K UHD", 3840, 2160),
    ("Square", 1080, 1080),
    ("Story", 1080, 1920),
    ("Small", 800, 600),
];
/// Live `clampDim`: a new canvas is 16..=8192 on either side.
const NEW_CANVAS_MIN: f32 = 16.0;
const NEW_CANVAS_MAX: f32 = 8192.0;
/// Engine cap for `doc.resize` (`snagcraft_raster::MAX_DIM`).
const RESIZE_MAX: f32 = 16384.0;

fn clamp_canvas_dim(n: f32) -> f32 {
    if n.is_finite() {
        n.round().clamp(NEW_CANVAS_MIN, NEW_CANVAS_MAX)
    } else {
        NEW_CANVAS_MIN
    }
}

impl SnagCraftApp {
    /// Ctrl+N / File → New…: open the dialog seeded with the live defaults.
    pub fn open_new_canvas(&mut self) {
        if self.new_canvas_open {
            return;
        }
        // W23: the live dialog reopens with the last-used size/background
        // (readNewPrefs); the rebuild keeps them in the persisted settings.
        self.new_canvas_w = self.settings.new_canvas_w as f32;
        self.new_canvas_h = self.settings.new_canvas_h as f32;
        self.new_canvas_bg = self.settings.new_canvas_bg.clone();
        self.new_canvas_open = true;
    }

    /// Replace the document with a blank canvas. Mirrors the live
    /// `newCanvas()`: a dirty document is saved into the save folder first
    /// (no prompt, no work lost), then `doc.new` swaps in the blank canvas.
    /// W23: `bg` is "white" or "transparent"; the choice is remembered.
    pub fn create_new_canvas(&mut self, w: f32, h: f32, bg: &str) {
        let w = clamp_canvas_dim(w);
        let h = clamp_canvas_dim(h);
        let bg = if bg == "transparent" {
            "transparent"
        } else {
            "white"
        };
        #[cfg(feature = "native")]
        if self.doc_dirty {
            self.save_now();
        }
        // W23: remember the size/background like the live localStorage prefs.
        let mut next = self.settings.clone();
        next.new_canvas_w = w as u32;
        next.new_canvas_h = h as u32;
        next.new_canvas_bg = bg.to_string();
        self.push_settings(next);
        match self.exec("doc.new", serde_json::json!({"w": w, "h": h, "bg": bg})) {
            Ok(_) => {
                self.doc_dirty = false;
                self.selected = None;
                self.crop = None;
                self.tex = None;
                self.recent_active = None;
                self.redraw = true;
                self.new_canvas_open = false;
                self.status =
                    format!("New canvas {w}×{h} — draw away, or press PrintScreen to capture.");
            }
            Err(e) => self.status = format!("Could not create canvas: {e}"),
        }
    }

    /// Toolbar Resize…: open the dialog with the current dimensions filled in.
    pub fn open_resize(&mut self) {
        let dims = self.sess().doc().map(|d| (d.size.w, d.size.h));
        match dims {
            Some((w, h)) => {
                self.resize_w = w as f32;
                self.resize_h = h as f32;
                self.resize_error = None;
                self.resize_open = true;
            }
            None => self.status = "Nothing to resize — capture a screen first.".into(),
        }
    }

    /// Validate the dialog's two fields, then hand the size to `doc.resize`.
    pub fn apply_resize(&mut self) {
        let (w, h) = (self.resize_w.round(), self.resize_h.round());
        if !w.is_finite() || !h.is_finite() || w < 1.0 || h < 1.0 {
            self.resize_error = Some("Width and height must each be at least 1 pixel.".into());
            return;
        }
        if w > RESIZE_MAX || h > RESIZE_MAX {
            self.resize_error = Some(format!(
                "{} px is the maximum on either side.",
                RESIZE_MAX as u32
            ));
            return;
        }
        match self.exec("doc.resize", serde_json::json!({"w": w, "h": h})) {
            Ok(v) => {
                self.doc_dirty = true;
                self.tex = None;
                self.redraw = true;
                self.selected = None;
                self.resize_open = false;
                self.resize_error = None;
                self.status = format!("Resized to {} × {} px", v["width"], v["height"]);
            }
            Err(e) => self.resize_error = Some(format!("Resize failed: {e}")),
        }
    }

    /// Ctrl+scroll: multiply the fit scale, clamped so a wild spin cannot lose
    /// the canvas. Non-finite deltas (hostile input) fall back to the fit.
    pub fn zoom_view(&mut self, factor: f32) {
        let next = self.view_zoom * factor;
        self.view_zoom = if next.is_finite() {
            next.clamp(VIEW_ZOOM_MIN, VIEW_ZOOM_MAX)
        } else {
            1.0
        };
    }

    /// Reset the canvas view to the plain fit scale.
    pub fn zoom_reset(&mut self) {
        self.view_zoom = 1.0;
    }

    /// `?` / Help → Keyboard shortcuts: the sheet listing the real bindings.
    fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
        if !self.shortcuts_open {
            return;
        }
        let mut open = true;
        let mut close = false;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(430.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Every binding below works in this build.")
                        .size(11.5)
                        .weak(),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(430.0)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                        egui::Grid::new("shortcuts-grid")
                            .num_columns(2)
                            .spacing([18.0, 6.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (keys, what) in SHORTCUTS {
                                    ui.label(egui::RichText::new(*keys).monospace().strong());
                                    ui.label(egui::RichText::new(*what).size(12.0));
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    ui.weak("Esc also closes this sheet.");
                });
            });
        if close || !open {
            self.shortcuts_open = false;
        }
    }

    /// File → About SnagCraft…: mark, name, version, one line, repo.
    fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut open = true;
        let mut close = false;
        egui::Window::new("About SnagCraft")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(11), pal_accent(ui));
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "S",
                        egui::FontId::proportional(28.0),
                        Color32::WHITE,
                    );
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("SnagCraft").size(20.0).strong());
                        ui.label(
                            egui::RichText::new(format!("version {APP_VERSION}"))
                                .size(11.5)
                                .weak(),
                        );
                    });
                });
                ui.add_space(8.0);
                ui.label("Screenshot, annotate and share — one window, no cloud.");
                ui.add_space(6.0);
                ui.hyperlink_to(REPO_URL, REPO_URL);
                // W19: the live About's license line (index.html:1184) — the
                // W12 tick overclaimed it. Relicensing stays rostad's gate.
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("GPL-3.0-or-later · commercial license available")
                        .size(11.0)
                        .weak(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        close = true;
                    }
                    ui.weak("Esc also closes.");
                });
            });
        if close || !open {
            self.about_open = false;
        }
    }

    /// W25: the in-app monitor picker (Windows/macOS). Lists the connected
    /// outputs as cards — name, resolution, a Primary badge — and capturing the
    /// clicked one. This is the issue #1 fix where no OS picker exists.
    #[cfg(feature = "native")]
    fn monitor_picker_dialog(&mut self, ctx: &egui::Context) {
        if !self.monitor_picker_open {
            return;
        }
        let mut open = true;
        let mut cancel = false;
        let mut pick: Option<usize> = None;
        egui::Window::new("Choose a screen")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Pick the monitor to capture.")
                        .size(11.5)
                        .weak(),
                );
                ui.add_space(6.0);
                let monitors = self.monitors.clone();
                for m in &monitors {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(332.0, 46.0), egui::Sense::click());
                    let hovered = resp.hovered();
                    let fill = if hovered {
                        pal_hover(ui)
                    } else {
                        ui.visuals().faint_bg_color
                    };
                    let border = if hovered {
                        pal_accent(ui)
                    } else {
                        pal_border(ui)
                    };
                    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
                    ui.painter().rect_stroke(
                        rect,
                        CornerRadius::same(6),
                        Stroke::new(1.0, border),
                        StrokeKind::Inside,
                    );
                    ui.painter().text(
                        pos2(rect.left() + 12.0, rect.top() + 8.0),
                        egui::Align2::LEFT_TOP,
                        &m.name,
                        egui::FontId::proportional(12.5),
                        pal_fg(ui),
                    );
                    ui.painter().text(
                        pos2(rect.left() + 12.0, rect.top() + 26.0),
                        egui::Align2::LEFT_TOP,
                        format!("{} × {}", m.width, m.height),
                        egui::FontId::proportional(11.0),
                        pal_muted(ui),
                    );
                    if m.is_primary {
                        let badge = "Primary";
                        let galley = ui.painter().layout_no_wrap(
                            badge.to_string(),
                            egui::FontId::proportional(9.0),
                            pal_accent(ui),
                        );
                        let badge_rect = Rect::from_min_size(
                            pos2(rect.right() - galley.size().x - 20.0, rect.top() + 8.0),
                            galley.size() + vec2(8.0, 2.0),
                        );
                        ui.painter().rect_filled(
                            badge_rect,
                            CornerRadius::same(4),
                            pal_surface(ui),
                        );
                        ui.painter().galley(
                            pos2(badge_rect.left() + 4.0, badge_rect.top() + 1.0),
                            galley,
                            pal_accent(ui),
                        );
                    }
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            true,
                            format!("{} {} × {}", m.name, m.width, m.height),
                        )
                    });
                    let resp = resp.on_hover_text("Capture this monitor");
                    if resp.clicked() {
                        pick = Some(m.index);
                    }
                    ui.add_space(6.0);
                }
                ui.add_space(2.0);
                // Fixed-height footer row: a bare `with_layout(right_to_left)`
                // expands to the window's full available height and stretched
                // the dialog to ~2× its content. allocate_ui_with_layout pins it.
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    },
                );
            });
        if let Some(index) = pick {
            self.capture_monitor(index);
        } else if cancel || !open {
            self.monitor_picker_open = false;
        }
    }

    /// The New-canvas dialog: eight one-click presets plus custom fields.
    fn new_canvas_dialog(&mut self, ctx: &egui::Context) {
        if !self.new_canvas_open {
            return;
        }
        let mut open = true;
        let mut create: Option<(f32, f32)> = None;
        let mut cancel = false;
        egui::Window::new("New canvas")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("A blank canvas — pick a size, or type your own.")
                        .size(11.5)
                        .weak(),
                );
                ui.add_space(6.0);
                // W23: the live preset grid (style.css:4661) — two equal
                // columns of vertical cards (name over dims), radio semantics:
                // clicking fills the size fields, the matching card lights up.
                egui::Grid::new("new-canvas-presets")
                    .num_columns(2)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        for (i, (name, w, h)) in NEW_CANVAS_PRESETS.iter().enumerate() {
                            let active =
                                self.new_canvas_w == *w as f32 && self.new_canvas_h == *h as f32;
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(186.0, 44.0), egui::Sense::click());
                            let accent = pal_accent(ui);
                            let fill = if active {
                                accent.linear_multiply(0.14)
                            } else if resp.hovered() {
                                pal_hover(ui)
                            } else {
                                ui.visuals().faint_bg_color
                            };
                            let border = if active || resp.hovered() {
                                accent
                            } else {
                                pal_border(ui)
                            };
                            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
                            ui.painter().rect_stroke(
                                rect,
                                CornerRadius::same(6),
                                Stroke::new(1.0, border),
                                StrokeKind::Inside,
                            );
                            ui.painter().text(
                                pos2(rect.left() + 10.0, rect.top() + 7.0),
                                egui::Align2::LEFT_TOP,
                                *name,
                                egui::FontId::proportional(12.5),
                                if active { accent } else { pal_fg(ui) },
                            );
                            ui.painter().text(
                                pos2(rect.left() + 10.0, rect.top() + 24.0),
                                egui::Align2::LEFT_TOP,
                                format!("{w} × {h}"),
                                egui::FontId::proportional(11.0),
                                pal_muted(ui),
                            );
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::RadioButton,
                                    active,
                                    format!("{name} {w} × {h}"),
                                )
                            });
                            let resp = resp.on_hover_text("Fill in this size");
                            if resp.clicked() {
                                self.new_canvas_w = *w as f32;
                                self.new_canvas_h = *h as f32;
                            }
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Width");
                    ui.add(
                        egui::DragValue::new(&mut self.new_canvas_w)
                            .range(NEW_CANVAS_MIN..=NEW_CANVAS_MAX)
                            .speed(4.0)
                            .max_decimals(0)
                            .suffix(" px"),
                    );
                    ui.add_space(8.0);
                    ui.label("Height");
                    ui.add(
                        egui::DragValue::new(&mut self.new_canvas_h)
                            .range(NEW_CANVAS_MIN..=NEW_CANVAS_MAX)
                            .speed(4.0)
                            .max_decimals(0)
                            .suffix(" px"),
                    );
                });
                ui.add_space(6.0);
                // W23: the live white/transparent background choice.
                ui.horizontal(|ui| {
                    ui.label("Background");
                    ui.selectable_value(&mut self.new_canvas_bg, "white".to_string(), "White");
                    ui.selectable_value(
                        &mut self.new_canvas_bg,
                        "transparent".to_string(),
                        "Transparent",
                    );
                });
                ui.add_space(8.0);
                // Fixed-height footer (same stretch fix as the monitor picker):
                // a bare with_layout(right_to_left) expands to the window height.
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 28.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        // W23: Create is the primary action (live btn-primary).
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Create").color(Color32::WHITE),
                                )
                                .fill(pal_accent(ui)),
                            )
                            .clicked()
                        {
                            create = Some((self.new_canvas_w, self.new_canvas_h));
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    },
                );
            });
        if let Some((w, h)) = create {
            let bg = self.new_canvas_bg.clone();
            self.create_new_canvas(w, h, &bg);
        } else if cancel || !open {
            self.new_canvas_open = false;
        }
    }

    /// The Resize dialog: current dims prefilled, Esc / Cancel close it.
    fn resize_dialog(&mut self, ctx: &egui::Context) {
        if !self.resize_open {
            return;
        }
        let Some((cw, ch)) = self.sess().doc().map(|d| (d.size.w, d.size.h)) else {
            self.resize_open = false;
            return;
        };
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new("Resize image")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(330.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("Current: {cw} × {ch} px"))
                        .size(11.5)
                        .weak(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Width");
                    ui.add(
                        egui::DragValue::new(&mut self.resize_w)
                            .range(1.0..=RESIZE_MAX)
                            .speed(1.0)
                            .max_decimals(0)
                            .suffix(" px"),
                    );
                    ui.label("Height");
                    ui.add(
                        egui::DragValue::new(&mut self.resize_h)
                            .range(1.0..=RESIZE_MAX)
                            .speed(1.0)
                            .max_decimals(0)
                            .suffix(" px"),
                    );
                });
                if let Some(err) = &self.resize_error {
                    ui.label(
                        egui::RichText::new(err)
                            .size(11.0)
                            .color(Color32::from_rgb(0xd0, 0x3a, 0x3a)),
                    );
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        apply = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if apply {
            self.apply_resize();
        } else if cancel || !open {
            self.resize_open = false;
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
/// W24: the live `--danger` (style.css:140/185) — destructive-action ink.
const DANGER_LIGHT: Color32 = Color32::from_rgb(201, 64, 92);
const DANGER_DARK: Color32 = Color32::from_rgb(255, 111, 134);
/// Stop control fill (live widget `#e5484d`).
const WIDGET_STOP: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
const WIDGET_BG: Color32 = Color32::from_rgba_premultiplied(26, 29, 35, 242);

/// The floating widget's own viewport (immediate viewport, one per frame).
pub fn widget_viewport_id() -> ViewportId {
    ViewportId::from_hash_of("snagcraft-widget")
}

/// What a capture request asked for (W11 PrintScreen fallback assertions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    Full,
    Region,
}

/// What the settings panel asks for beyond "the value changed".
#[derive(Default)]
struct SettingsActions {
    changed: bool,
    pick_dir: bool,
    /// Put the save folder back to the OS default (`None`).
    reset_dir: bool,
    /// Pick the separate recording folder (W11).
    pick_rec_dir: bool,
    /// Recordings follow the screenshot folder again (W11).
    reset_rec_dir: bool,
    /// Settings → Troubleshooting: copy diagnostics to the clipboard (W11).
    copy_diagnostics: bool,
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
        // W11: the "Enable global hotkeys" switch owns registration. Off drops
        // the portal session; PrintScreen keeps working in-app.
        if !self.settings.hotkeys_enabled {
            self.platform.hotkeys = None;
            self.status =
                "Global hotkeys are off — PrintScreen still captures in-app. Turn them on to register."
                    .into();
            return;
        }
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
        // W28: land finished off-thread video thumbnails onto the Recent tiles.
        #[cfg(feature = "native")]
        self.poll_video_thumbs(ctx);
        if self.recording()
            || self.scroll.armed
            || self.timelapse.armed
            || self.timelapse.assembling
        {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.handle_close_request(ctx);
    }

    fn close_modal(&mut self, ctx: &egui::Context) {
        if !self.confirm_close {
            return;
        }
        let mut action = None;
        // W24: the live guard (index.html close-guard, app.ts:1148-1166) —
        // question title, Cancel on the left, Discard (ghost-danger) and
        // "Save & close" (primary) grouped on the right.
        egui::Window::new("Save changes before closing?")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    picture_icon(ui);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("You have unsaved annotations.").strong());
                        ui.label(
                            egui::RichText::new("Closing now throws them away.")
                                .weak()
                                .size(11.5),
                        );
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = Some(2u8);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Primary: accent-filled, white label (live btn-primary).
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Save & close").color(Color32::WHITE),
                                )
                                .fill(pal_accent(ui)),
                            )
                            .clicked()
                        {
                            action = Some(0);
                        }
                        // Destructive: danger ink + border, transparent fill
                        // (live btn-ghost-danger), set apart from Save.
                        let danger = pal_danger(ui);
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("Discard").color(danger))
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::new(1.0, danger)),
                            )
                            .clicked()
                        {
                            action = Some(1);
                        }
                    });
                });
            });
        match action {
            // Save: only close once the file really landed (live saveEdited).
            Some(0) => {
                if self.save_now() {
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
        let was_hotkeys = self.settings.hotkeys_enabled;
        // The mic dropdown reads the (once-per-session) `audio.sources` list.
        let sources = self.rec.sources.clone();
        let status = self.status.clone();
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            // The panel outgrew a short screen in W11: the window caps itself to
            // the viewport and scrolls instead of running off the bottom.
            .vscroll(true)
            // W17: show the scrollbar even before the first scroll, so the
            // below-fold rows read as reachable, not clipped (W11 lesson).
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .default_width(380.0)
            // W17: first open lands over the canvas, not over the toolbar/props
            // rail it configures (the old egui default covered the swatches).
            .default_pos(egui::pos2(60.0, 96.0))
            .show(ctx, |ui| {
                settings_ui(ui, &mut next, &mut acts, &sources, &status);
            });
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
        if acts.reset_dir {
            next.save_dir = None;
            acts.changed = true;
        }
        if acts.pick_rec_dir {
            #[cfg(feature = "native")]
            {
                let mut dialog = rfd::FileDialog::new();
                if let Some(current) = &next.recording_dir {
                    dialog = dialog.set_directory(current);
                }
                if let Some(dir) = dialog.pick_folder() {
                    next.recording_dir = Some(dir.to_string_lossy().to_string());
                    acts.changed = true;
                }
            }
            #[cfg(not(feature = "native"))]
            {
                self.status = "Folder pickers are desktop-only — the web build downloads.".into();
            }
        }
        if acts.reset_rec_dir {
            next.recording_dir = None;
            acts.changed = true;
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
            // W11: flipping "Enable global hotkeys" registers (or drops) the
            // portal bindings straight away.
            if self.settings.hotkeys_enabled != was_hotkeys {
                #[cfg(feature = "native")]
                self.rebind_hotkeys();
            }
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
        if acts.copy_diagnostics {
            self.copy_diagnostics();
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
    // W17: clearer hierarchy — more air above each group, a touch larger
    // than the 12.5 the body rows use, so scanning the panel is faster.
    ui.add_space(10.0);
    ui.label(egui::RichText::new(title).strong().size(13.0));
    ui.add_space(2.0);
}

/// Human label for a tesseract language code (`swe` → "Swedish (swe)").
/// Unknown codes are shown verbatim — the picker never hides a language the
/// engine reported.
fn ocr_language_label(code: &str) -> String {
    let name = match code {
        "eng" => "English",
        "swe" => "Swedish",
        "dan" => "Danish",
        "nor" => "Norwegian",
        "fin" => "Finnish",
        "deu" | "ger" => "German",
        "fra" | "fre" => "French",
        "spa" => "Spanish",
        "ita" => "Italian",
        "por" => "Portuguese",
        "nld" | "dut" => "Dutch",
        "pol" => "Polish",
        "rus" => "Russian",
        "ukr" => "Ukrainian",
        "ara" => "Arabic",
        "hin" => "Hindi",
        "jpn" => "Japanese",
        "kor" => "Korean",
        "chi_sim" => "Chinese (simplified)",
        "chi_tra" => "Chinese (traditional)",
        _ => return code.to_string(),
    };
    format!("{name} ({code})")
}

/// The settings body. Pure UI: it mutates `next` and reports what happened.
fn settings_ui(
    ui: &mut egui::Ui,
    next: &mut SettingsView,
    acts: &mut SettingsActions,
    sources: &[(String, String)],
    status: &str,
) {
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

    // W27: the live app splits General (the switches above) from Capture (delay
    // + interval), and offers the delay as a TIMER_CHOICES pill radio group
    // [Off, 3 s, 5 s, 10 s] — not a continuous slider. Match both.
    ui.separator();
    section(ui, "Capture");
    ui.label(
        egui::RichText::new("Wait before grabbing the screen, so you can set the shot up first.")
            .weak()
            .size(11.0),
    );
    ui.horizontal(|ui| {
        ui.label("Capture delay");
        // Uniform segmented pills (live `pills` radiogroup): every choice gets a
        // neutral container, the selected one the accent fill — selectable_value
        // alone leaves the unselected options as bare text.
        for secs in [0u32, 3, 5, 10] {
            let selected = next.capture_delay_secs == secs;
            let label = if secs == 0 {
                "Off".to_string()
            } else {
                format!("{secs} s")
            };
            let text_color = if selected { Color32::WHITE } else { pal_fg(ui) };
            let fill = if selected {
                pal_accent(ui)
            } else {
                ui.visuals().faint_bg_color
            };
            let border = if selected {
                pal_accent(ui)
            } else {
                pal_border(ui)
            };
            let resp = ui
                .add(
                    egui::Button::new(egui::RichText::new(label).size(11.5).color(text_color))
                        .fill(fill)
                        .stroke(Stroke::new(1.0, border))
                        .corner_radius(CornerRadius::same(6))
                        .min_size(vec2(36.0, 22.0)),
                )
                .on_hover_text("Wait this long before grabbing the screen");
            if resp.clicked() {
                next.capture_delay_secs = secs;
                acts.changed = true;
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("Interval capture");
        egui::ComboBox::from_id_salt("settings-interval")
            .width(110.0)
            .selected_text(interval_label(next.interval_secs))
            .show_ui(ui, |ui| {
                for secs in INTERVAL_CHOICES {
                    acts.changed |= ui
                        .selectable_value(&mut next.interval_secs, secs, interval_label(secs))
                        .changed();
                }
            });
    });
    ui.label(
        egui::RichText::new("Interval capture shoots this often; Stop ends the loop.")
            .weak()
            .size(11.0),
    );

    ui.separator();
    section(ui, "Recording audio");
    ui.horizontal(|ui| {
        ui.label("Microphone");
        let mic = next
            .mic_source
            .as_ref()
            .and_then(|id| {
                sources
                    .iter()
                    .find(|(sid, _)| sid == id)
                    .map(|(_, name)| name.clone())
            })
            .unwrap_or_else(|| "None".to_string());
        egui::ComboBox::from_id_salt("settings-mic")
            .width(190.0)
            .selected_text(mic)
            .show_ui(ui, |ui| {
                acts.changed |= ui
                    .selectable_value(&mut next.mic_source, None, "None")
                    .changed();
                for (id, name) in sources {
                    acts.changed |= ui
                        .selectable_value(&mut next.mic_source, Some(id.clone()), name)
                        .changed();
                }
            });
    });
    if sources.is_empty() {
        ui.label(
            egui::RichText::new("No microphone sources yet — they appear after a take.")
                .weak()
                .size(10.5),
        );
    }
    acts.changed |= ui
        .checkbox(&mut next.system_audio, "System audio (desktop output)")
        .on_hover_text("Mix the desktop's own sound into the take")
        .changed();

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
    ui.horizontal(|ui| {
        ui.label("Recording format");
        egui::ComboBox::from_id_salt("settings-rec-format")
            .width(96.0)
            .selected_text(next.recording_format.clone())
            .show_ui(ui, |ui| {
                for format in snagcraft_io::settings::RECORDING_FORMATS {
                    acts.changed |= ui
                        .selectable_value(&mut next.recording_format, format.to_string(), format)
                        .changed();
                }
            });
    });
    ui.label(
        egui::RichText::new("Used by the Record button in the top bar.")
            .weak()
            .size(10.5),
    );
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
        let shown = next.save_dir.clone().unwrap_or_else(|| {
            snagcraft_io::files::default_save_dir()
                .display()
                .to_string()
        });
        ui.add(
            egui::Label::new(egui::RichText::new(shown).size(11.5).monospace())
                .truncate()
                .sense(Sense::hover()),
        );
        #[cfg(feature = "native")]
        if ui.button("Choose…").clicked() {
            acts.pick_dir = true;
        }
        if next.save_dir.is_some() && ui.button("Reset to default").clicked() {
            acts.reset_dir = true;
        }
    });
    ui.label(
        egui::RichText::new(format!(
            "Default: {}",
            snagcraft_io::files::default_save_dir().display()
        ))
        .weak()
        .size(10.5),
    );
    let pattern_resp = ui
        .horizontal(|ui| {
            ui.label("File name");
            ui.add(
                egui::TextEdit::singleline(&mut next.filename_pattern)
                    .desired_width(220.0)
                    .hint_text("SnagCraft-{date}-{time}"),
            )
            .on_hover_text("{date} and {time} expand at save time")
        })
        .inner;
    acts.changed |= pattern_resp.changed();
    ui.label(
        egui::RichText::new("{date} 2026-11-14 · {time} 19-45-12 · {datetime} both")
            .weak()
            .size(10.5),
    );
    // Live preview through the real filename builder (W9). While the field is
    // focused it follows the clock; otherwise it shows a fixed example stamp so
    // the panel (and its snapshot) stays stable between runs.
    let stamp = if pattern_resp.has_focus() {
        chrono::Local::now()
    } else {
        preview_stamp()
    };
    let preview = snagcraft_io::files::build_filename(
        &next.filename_pattern,
        snagcraft_io::ImageFormat::Png,
        stamp,
    );
    ui.label(
        egui::RichText::new(format!("Preview: {preview}"))
            .weak()
            .size(10.5)
            .monospace(),
    );
    ui.add_space(4.0);
    acts.changed |= ui
        .checkbox(&mut next.split_folders, "Separate recording folder")
        .on_hover_text("Off: recordings land with the screenshots")
        .changed();
    ui.horizontal(|ui| {
        let shown = match (&next.recording_dir, next.split_folders) {
            (Some(dir), true) => dir.clone(),
            _ => "Same as the save folder".to_string(),
        };
        ui.add(
            egui::Label::new(egui::RichText::new(shown).size(11.0).monospace())
                .truncate()
                .sense(Sense::hover()),
        );
        #[cfg(feature = "native")]
        if ui
            .add_enabled(next.split_folders, egui::Button::new("Change…"))
            .clicked()
        {
            acts.pick_rec_dir = true;
        }
        if next.recording_dir.is_some() && ui.button("Use the save folder").clicked() {
            acts.reset_rec_dir = true;
        }
    });

    ui.separator();
    section(ui, "Global hotkeys");
    acts.changed |= ui
        .checkbox(&mut next.hotkeys_enabled, "Enable global hotkeys")
        .on_hover_text("Capture while another app has focus (Wayland GlobalShortcuts portal)")
        .changed();
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
            .add_enabled(next.hotkeys_enabled, egui::Button::new("Register hotkeys"))
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
    if !next.hotkeys_enabled {
        ui.label(
            egui::RichText::new(
                "Off: PrintScreen and Shift+PrintScreen still capture inside SnagCraft.",
            )
            .weak()
            .size(10.5),
        );
    }

    ui.separator();
    section(ui, "Troubleshooting");
    ui.label(
        egui::RichText::new("Recording not starting? Copy the logs into a bug report.")
            .weak()
            .size(10.5),
    );
    ui.horizontal(|ui| {
        if ui.button("Copy diagnostics").clicked() {
            acts.copy_diagnostics = true;
        }
    });
    if !status.is_empty() {
        // Bounded so a long status can never widen the panel (and push the
        // Theme row off-screen).
        let short: String = status.chars().take(120).collect();
        ui.label(egui::RichText::new(short).weak().size(10.5));
    }

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
    if matches!(theme, Theme::Dark) {
        // W17: unchecked checkboxes and idle buttons were nearly invisible
        // (DARK_BORDER on DARK_SURFACE ≈ 1.4:1). A lighter idle outline makes
        // the affordance readable without touching the panel hairlines.
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(86, 93, 104));
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.bg_stroke = Stroke::new(1.5, accent);
    ctx.set_visuals(v);
    // W17: egui's default floating scroll bars are INVISIBLE when dormant
    // (dormant_*_opacity = 0) — `AlwaysVisible` alone draws nothing until the
    // pointer hovers the area. Give dormant bars a faint but real presence so
    // below-fold content reads as reachable (settings, shortcuts sheet).
    for t in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.style_mut_of(t, |style| {
            style.spacing.scroll.dormant_handle_opacity = 0.45;
            style.spacing.scroll.dormant_background_opacity = 0.12;
        });
    }
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
                // W14 live parity (app.ts:433, :993-995, :3163): a 48px nib
                // reference box — ink-coloured dot at the nib's 1:1 diameter,
                // value label beside it. The rail range IS nib px (2..44), so
                // the dot maps 1:1 into the box.
                ui.horizontal(|ui| {
                    let (resp, painter) =
                        ui.allocate_painter(vec2(48.0, 48.0), egui::Sense::hover());
                    let r = resp.rect;
                    resp.on_hover_text(format!("Nib preview — {:.0} px at 1:1", app.brush_width));
                    painter.rect_stroke(
                        r,
                        CornerRadius::same(6),
                        Stroke::new(1.0, pal_border(ui)),
                        StrokeKind::Inside,
                    );
                    let d = app.brush_width.clamp(BRUSH_MIN, BRUSH_MAX);
                    painter.circle_filled(r.center(), d / 2.0, app.color);
                    ui.label(
                        egui::RichText::new(format!("{:.0} px", app.brush_width))
                            .color(pal_muted(ui))
                            .size(11.0),
                    );
                });
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
    // Read-only sheets: Esc closes, every other key is theirs. Checked first so
    // a stray `?` or letter cannot act on the canvas behind the sheet.
    if app.shortcuts_open || app.about_open {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            app.shortcuts_open = false;
            app.about_open = false;
        }
        return;
    }

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

    // W25: the monitor picker owns Esc while it is up (native only).
    #[cfg(feature = "native")]
    if app.monitor_picker_open {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            app.monitor_picker_open = false;
        }
        return;
    }

    // A canvas dialog owns Esc (and the keyboard) while it is up.
    if app.new_canvas_open || app.resize_open {
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            app.new_canvas_open = false;
            app.resize_open = false;
        }
        return;
    }

    // Ctrl+scroll zooms the canvas: egui folds ctrl+wheel into `zoom_delta`,
    // so the raw wheel delta is already gone by the time keys are read.
    let zoom = ctx.input(|i| i.zoom_delta());
    if (zoom - 1.0).abs() > 1e-4 {
        app.zoom_view(zoom);
    }

    ctx.input_mut(|i| {
        // `?` opens the keyboard-shortcuts sheet — live app.ts:5138-5146.
        if i.consume_key(Modifiers::SHIFT, Key::Questionmark) {
            app.shortcuts_open = true;
        }
        // W11: PrintScreen is the in-app fallback while global hotkeys are off
        // (or the portal refused them) — live app.ts:5128-5132.
        if i.consume_key(Modifiers::SHIFT, Key::PrintScreen) {
            app.capture_region();
        }
        if i.consume_key(Modifiers::NONE, Key::PrintScreen) {
            app.capture_full();
        }
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
        // Same predicate as the HUD: armed (starting) or active (frames rolling).
        if (app.scroll.armed || app.scroll.active) && i.consume_key(Modifiers::NONE, Key::Enter) {
            app.stop_scrolling();
        }
        // W20: Esc discards a running scroll — the live card's Cancel.
        if (app.scroll.armed || app.scroll.active) && i.consume_key(Modifiers::NONE, Key::Escape) {
            app.cancel_scrolling();
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
        // Ctrl+N: the New-canvas dialog (live app.ts new-canvas).
        if i.consume_key(Modifiers::CTRL, Key::N) {
            app.open_new_canvas();
        }
        // Ctrl+O: open an image file into the editor (File → Open…).
        if i.consume_key(Modifiers::CTRL, Key::O) {
            app.open_dialog();
        }
        // Ctrl+S saves a flattened capture into the save folder, Ctrl+Shift+S is
        // Save As… — the live toolbar's two buttons on the keyboard.
        if i.consume_key(Modifiers::CTRL, Key::S) {
            if i.modifiers.shift {
                app.save_as_dialog();
            } else {
                app.save_now();
            }
        }
        // Ctrl+. ends a running interval capture (timelapse) — the keyboard
        // twin of the Stop button, matching the live app (app.ts:5181-5188).
        // Only while a timelapse is running, so the key stays free otherwise.
        if app.timelapse.running && i.consume_key(Modifiers::CTRL, Key::Period) {
            app.stop_timelapse();
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
            && frame.w >= 8.0
            && frame.h >= 8.0
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

/// Compact centered countdown pill (B4#5): the shrunk variant of the live
/// app's countdown — no scrim and no editor chrome, just the number, so the
/// desktop stays usable while it counts.
fn countdown_hud(ctx: &egui::Context, app: &mut SnagCraftApp) {
    let Some(countdown) = app.countdown else {
        return;
    };
    let mut countdown = countdown;
    if let Some(deadline) = countdown.deadline {
        if Instant::now() >= deadline {
            if countdown.remaining <= 1 {
                app.countdown = None;
                return;
            }
            countdown.remaining -= 1;
            countdown.deadline = Some(Instant::now() + Duration::from_secs(1));
        }
        // Keep the seconds ticking while nothing else repaints.
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    app.countdown = Some(countdown);
    let remaining = countdown.remaining;

    egui::Area::new(egui::Id::unique("countdown-hud"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(pal_surface(ui))
                .stroke(Stroke::new(1.0, pal_border(ui)))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(20, 14))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("CAPTURE IN").size(10.5).strong().weak());
                        ui.label(
                            egui::RichText::new(remaining.to_string())
                                .size(52.0)
                                .strong()
                                .color(pal_accent(ui)),
                        );
                        ui.label(
                            egui::RichText::new("Screen capture starts automatically")
                                .size(11.0)
                                .color(pal_fg(ui)),
                        );
                    });
                });
        });
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
                    // W18: the live pill's scope tag (app.ts:2308-2310).
                    if app.rec.region.is_some() {
                        ui.label(
                            egui::RichText::new("Region")
                                .size(11.0)
                                .color(pal_muted(ui)),
                        );
                    }
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
                    // W20: the live card's Cancel — discard, write nothing.
                    if ui
                        .button("Cancel")
                        .on_hover_text("Discard this scrolling capture")
                        .clicked()
                    {
                        app.cancel_scrolling();
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

/// File → Upload…: POST the current image to the user's own endpoint and copy
/// the link it returns. Native only — the web build has no curl.
///
/// Always enabled: the click is what reports "nothing to upload" or "set an
/// upload URL", which is more useful than a greyed-out item with no reason.
#[cfg(feature = "native")]
fn file_menu(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    ui.menu_button("File", |ui| {
        ui.set_min_width(250.0);
        if ui
            .button("New…")
            .on_hover_text("A blank canvas in eight preset sizes (Ctrl+N)")
            .clicked()
        {
            app.open_new_canvas();
            ui.close();
        }
        ui.separator();
        if ui
            .button("Save")
            .on_hover_text("Save a flattened copy into your save folder (Ctrl+S)")
            .clicked()
        {
            app.save_now();
            ui.close();
        }
        if ui
            .button("Save As…")
            .on_hover_text("Choose name and folder (Ctrl+Shift+S)")
            .clicked()
        {
            app.save_as_dialog();
            ui.close();
        }
        // W19: the rebuild's editable format stays reachable, explicitly
        // labeled — the live app has no native save, so this is an addition.
        if ui
            .button("Save editable (.snagcraft)…")
            .on_hover_text("Save annotations too — reopen and keep editing later")
            .clicked()
        {
            app.save_dialog();
            ui.close();
        }
        ui.separator();
        if ui
            .button("Upload…")
            .on_hover_text("POST the image to your endpoint and copy the link")
            .clicked()
        {
            app.upload_image_now();
            ui.close();
        }
        ui.separator();
        // Help lives under File so the sheet is one click away from anywhere.
        if menu_row(ui, "Keyboard shortcuts", "?") {
            app.shortcuts_open = true;
            ui.close();
        }
        // The version doubles as the menu hint, like every other row's keys.
        if menu_row(ui, "About SnagCraft…", APP_VERSION) {
            app.about_open = true;
            ui.close();
        }
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
            // W25: pick one monitor — in-app picker where no OS picker exists
            // (Windows/macOS); on Linux the system portal picker is used.
            if ui
                .add_enabled(!capturing, egui::Button::new("Choose screen…"))
                .on_hover_text("Capture one specific monitor")
                .clicked()
            {
                app.open_monitor_picker();
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

/// W22: the live tile's format badge (`renderFilm`: the extension uppercased,
/// a generic fallback when there is none).
fn recent_badge(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_default();
    if ext.is_empty() {
        "IMG".to_string()
    } else {
        ext
    }
}

/// W22: the live tile's time caption (`clockLabel(item.ts)`) — local HH:MM.
fn recent_time(ts: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(ts)
        .format("%H:%M")
        .to_string()
}

/// Decode `path` into a thumbnail texture (desktop). The web build has no file
/// handle to re-read, so its tiles keep the placeholder glyph.
fn load_thumb(
    ctx: &egui::Context,
    path: &std::path::Path,
    name: &str,
) -> Option<egui::TextureHandle> {
    #[cfg(feature = "native")]
    {
        let img = snagcraft_io::recent::thumbnail(path, 64)?;
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        Some(ctx.load_texture(
            format!("recent-{name}"),
            color,
            egui::TextureOptions::LINEAR,
        ))
    }
    #[cfg(not(feature = "native"))]
    {
        let _ = (ctx, path, name);
        None
    }
}

/// The Recents footer (`app.ts:924-933`): title + count, Hide, and a horizontal
/// track of thumbnails. Clicking a tile loads it back through `doc.open`.
fn filmstrip(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    if !app.film_open {
        return;
    }
    let ctx = ui.ctx().clone();
    egui::Panel::bottom("filmstrip")
        .frame(
            egui::Frame::new()
                .fill(pal_surface(ui))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .stroke(Stroke::new(1.0, pal_border(ui))),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Recent").strong().size(11.0));
                ui.label(
                    egui::RichText::new(app.recent.len().to_string())
                        .color(pal_muted(ui))
                        .size(11.0),
                );
                if ui
                    .small_button("Hide")
                    .on_hover_text("Hide recent captures")
                    .clicked()
                {
                    app.film_open = false;
                }
            });
            if app.recent.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Captures from this session land here — click one to load it back.",
                    )
                    .weak()
                    .size(10.5),
                );
                return;
            }
            let mut to_open: Option<PathBuf> = None;
            egui::ScrollArea::horizontal()
                .id_salt("film-track")
                .max_height(84.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.horizontal(|ui| {
                        // W28: live MEDIA_VIDEO_THUMBS — only the newest 8 video
                        // items get an ffmpeg thumbnail; the rest keep the film
                        // placeholder. Count videos by position (newest first).
                        let mut video_seen = 0usize;
                        for index in 0..app.recent.len() {
                            let item_path = app.recent[index].path.clone();
                            #[cfg(feature = "native")]
                            let is_video = snagcraft_io::recent::is_video_container(&item_path);
                            #[cfg(not(feature = "native"))]
                            let is_video = false;
                            if is_video {
                                video_seen += 1;
                            }
                            if app.recent[index].thumb.is_none() {
                                let path = app.recent[index].path.clone();
                                let name = app.recent[index].name.clone();
                                #[cfg(feature = "native")]
                                if is_video {
                                    if video_seen <= 8 {
                                        app.request_video_thumb(path, name);
                                    }
                                } else {
                                    app.recent[index].thumb = load_thumb(&ctx, &path, &name);
                                }
                                #[cfg(not(feature = "native"))]
                                {
                                    app.recent[index].thumb = load_thumb(&ctx, &path, &name);
                                }
                            }
                            // W22: the live vertical film-item (style.css
                            // :3411-3490) — thumbnail on top, format badge +
                            // time caption below, accent ring on the loaded
                            // capture, hover lift. Replaces the horizontal
                            // image-and-text chip.
                            let path = app.recent[index].path.clone();
                            let name = app.recent[index].name.clone();
                            let ts = app.recent[index].ts;
                            let active = app.recent_active.as_ref() == Some(&path);
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(112.0, 74.0), egui::Sense::click());
                            let accent = pal_accent(ui);
                            let fill = if active {
                                accent.linear_multiply(0.14)
                            } else if resp.hovered() {
                                pal_hover(ui)
                            } else {
                                ui.visuals().faint_bg_color
                            };
                            let border = if active || resp.hovered() {
                                accent
                            } else {
                                pal_border(ui)
                            };
                            ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
                            ui.painter().rect_stroke(
                                rect,
                                CornerRadius::same(7),
                                Stroke::new(1.0, border),
                                StrokeKind::Inside,
                            );
                            let pad = 4.0;
                            let thumb_size = vec2(112.0 - 2.0 * pad, 48.0);
                            let thumb_rect = Rect::from_min_size(
                                pos2(rect.left() + pad, rect.top() + pad),
                                thumb_size,
                            );
                            match &app.recent[index].thumb {
                                Some(texture) => {
                                    ui.put(
                                        thumb_rect,
                                        egui::Image::from_texture((texture.id(), thumb_size))
                                            .fit_to_exact_size(thumb_size)
                                            .corner_radius(6),
                                    );
                                }
                                None => {
                                    // Video/unreadable: the live film placeholder.
                                    // Painter-drawn — the bundled fonts have no
                                    // ▶ glyph (kittest panics on missing glyphs).
                                    let muted = pal_muted(ui);
                                    ui.painter().rect_filled(
                                        thumb_rect,
                                        CornerRadius::same(6),
                                        ui.visuals().extreme_bg_color,
                                    );
                                    let frame = thumb_rect.shrink(10.0);
                                    ui.painter().rect_stroke(
                                        frame,
                                        CornerRadius::same(3),
                                        Stroke::new(1.0, muted),
                                        StrokeKind::Inside,
                                    );
                                    // Sprocket holes along both edges.
                                    for i in 0..3 {
                                        let y = frame.top()
                                            + 3.0
                                            + i as f32 * ((frame.height() - 9.0) / 2.0);
                                        for x in [frame.left() + 2.0, frame.right() - 5.0] {
                                            ui.painter().rect_filled(
                                                Rect::from_min_size(pos2(x, y), vec2(3.0, 3.0)),
                                                CornerRadius::same(1),
                                                muted,
                                            );
                                        }
                                    }
                                    // Play triangle in the middle.
                                    let c = frame.center();
                                    ui.painter().add(egui::epaint::PathShape::convex_polygon(
                                        vec![
                                            pos2(c.x - 3.0, c.y - 5.0),
                                            pos2(c.x - 3.0, c.y + 5.0),
                                            pos2(c.x + 6.0, c.y),
                                        ],
                                        muted,
                                        Stroke::NONE,
                                    ));
                                }
                            }
                            let badge = recent_badge(&path);
                            let time = recent_time(ts);
                            let cap_y = rect.top() + pad + 48.0 + 4.0;
                            let galley = ui.painter().layout_no_wrap(
                                badge.clone(),
                                egui::FontId::proportional(9.0),
                                accent,
                            );
                            let badge_rect = Rect::from_min_size(
                                pos2(rect.left() + pad, cap_y),
                                galley.size() + vec2(8.0, 2.0),
                            );
                            ui.painter().rect_filled(
                                badge_rect,
                                CornerRadius::same(4),
                                pal_surface(ui),
                            );
                            ui.painter().galley(
                                pos2(badge_rect.left() + 4.0, badge_rect.top() + 1.0),
                                galley,
                                accent,
                            );
                            ui.painter().text(
                                pos2(rect.right() - pad, cap_y),
                                egui::Align2::RIGHT_TOP,
                                &time,
                                egui::FontId::proportional(10.5),
                                pal_muted(ui),
                            );
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::Button,
                                    true,
                                    format!("{name} · {badge} · {time}"),
                                )
                            });
                            let resp = resp.on_hover_text(format!(
                                "{name} · {time} · {badge}\n{}",
                                path.display()
                            ));
                            if resp.clicked() {
                                to_open = Some(path);
                            }
                        }
                    });
                });
            if let Some(path) = to_open {
                app.dispatch(
                    "doc.open",
                    serde_json::json!({"path": path.to_string_lossy()}),
                );
                app.recent_active = Some(path.clone());
                app.status = format!("Opened {}", path.display());
            }
        });
}

/// The whole shell in one fn — shared by eframe and headless kittest.
pub fn app_ui(ui: &mut egui::Ui, app: &mut SnagCraftApp) {
    let ctx = ui.ctx().clone();
    app.main_ctx = Some(ctx.clone());
    app.apply_settings();
    app.poll_platform(&ctx);
    #[cfg(not(feature = "native"))]
    app.poll_dropped(&ctx);
    #[cfg(not(feature = "native"))]
    app.poll_web_clipboard();
    app.poll_capture();
    app.poll_media();
    // W28: land finished off-thread video thumbnails onto the Recent tiles.
    #[cfg(feature = "native")]
    app.poll_video_thumbs(&ctx);
    app.scan_recent_folder();
    if app.recording()
        || app.scroll.armed
        || app.scroll.active
        || app.timelapse.armed
        || app.timelapse.running
        || app.timelapse.assembling
    {
        // Elapsed clock + frame counts keep ticking while nothing else does.
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    app.handle_close_request(&ctx);

    media_hud(ui, app);
    countdown_hud(&ctx, app);

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
                    // W17: group the strip — edit ops | document ops | menus.
                    ui.separator();
                    if ui
                        .button("Resize…")
                        .on_hover_text("Resize the canvas — annotations scale with it")
                        .clicked()
                    {
                        app.open_resize();
                    }
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
                        .on_hover_text("Save a flattened copy into your save folder (Ctrl+S)")
                        .clicked()
                    {
                        app.save_now();
                    }
                    if ui.button("Open").on_hover_text("Open an image").clicked() {
                        app.open_dialog();
                    }
                    ui.separator();
                    let recent_chip = egui::Button::new(
                        egui::RichText::new(format!("Recent ({})", app.recent_count())).size(11.0),
                    )
                    .selected(app.film_open);
                    if ui
                        .add(recent_chip)
                        .on_hover_text("Show or hide recent captures")
                        .clicked()
                    {
                        app.film_open = !app.film_open;
                    }
                    #[cfg(feature = "native")]
                    file_menu(ui, app);
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

    filmstrip(ui, app);

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
            let fit = (avail.width() / iw as f32).min(avail.height() / ih as f32);
            // Ctrl+scroll multiplies the fit scale; 1.0 keeps the old view.
            let scale = (fit * app.view_zoom).clamp(0.01, 8.0);
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
                    Tool::Select => {
                        // W14: diagonal resize cursors over the corner handles.
                        let mut icon = egui::CursorIcon::Default;
                        if let Some(p) = resp.hover_pos().map(&to_image) {
                            let sel = app.selected.and_then(|id| {
                                app.sess().doc().and_then(|d| d.get(id)).and_then(obj_rect)
                            });
                            if let Some(r) = sel {
                                let tol = 10.0 / scale;
                                for (i, c) in rect_corners(r).iter().enumerate() {
                                    if (p.x - c.x).abs() <= tol && (p.y - c.y).abs() <= tol {
                                        icon = if i == 0 || i == 3 {
                                            egui::CursorIcon::ResizeNwSe
                                        } else {
                                            egui::CursorIcon::ResizeNeSw
                                        };
                                        break;
                                    }
                                }
                            }
                        }
                        icon
                    }
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
                    // W14: double-click a Text/Callout to re-edit it in place
                    // (live fabric Textbox/IText behavior via applyInteractivity).
                    if resp.double_clicked()
                        && let Some(p) = hover_img
                    {
                        let hit = app.sess().doc().and_then(|d| d.hit_test(p));
                        if let Some(id) = hit {
                            let obj = app.sess().doc().and_then(|d| d.get(id)).cloned();
                            match obj {
                                Some(AnnotObject::Text {
                                    pos,
                                    text,
                                    size,
                                    bold,
                                    italic,
                                    family,
                                    color,
                                    ..
                                }) => {
                                    app.selected = Some(id);
                                    app.font_size = size;
                                    app.bold = bold;
                                    app.italic = italic;
                                    app.font_family = family;
                                    app.color = Color32::from_rgba_unmultiplied(
                                        color.r, color.g, color.b, color.a,
                                    );
                                    app.editing = Some(TextEdit {
                                        pos,
                                        buf: text,
                                        focus_requested: false,
                                        target: Some(id),
                                    });
                                }
                                Some(AnnotObject::Callout {
                                    rect,
                                    text,
                                    size,
                                    family,
                                    color,
                                    ..
                                }) => {
                                    app.selected = Some(id);
                                    app.font_size = size;
                                    app.font_family = family;
                                    app.color = Color32::from_rgba_unmultiplied(
                                        color.r, color.g, color.b, color.a,
                                    );
                                    app.editing = Some(TextEdit {
                                        pos: Point::new(rect.x + 8.0, rect.y + 8.0),
                                        buf: text,
                                        focus_requested: false,
                                        target: Some(id),
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                    if resp.clicked()
                        && let Some(p) = hover_img
                    {
                        app.selected = {
                            let sess = app.sess();
                            sess.doc().and_then(|d| d.hit_test(p))
                        };
                    }
                    let sel_rect = app
                        .selected
                        .and_then(|id| app.sess().doc().and_then(|d| d.get(id)).and_then(obj_rect));
                    if resp.drag_started()
                        && let Some(p) = press_img
                    {
                        // Corner handles win over the body move.
                        let mut started = false;
                        if let (Some(r), Some(id)) = (sel_rect, app.selected) {
                            let tol = 10.0 / scale;
                            let corners = rect_corners(r);
                            for (i, c) in corners.iter().enumerate() {
                                if (p.x - c.x).abs() <= tol && (p.y - c.y).abs() <= tol {
                                    app.drag = Some(Drag::Resize {
                                        id,
                                        anchor: corners[3 - i],
                                    });
                                    app.drag_cur = Some(p);
                                    started = true;
                                    break;
                                }
                            }
                        }
                        if !started && let Some(id) = app.selected {
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
                    }
                    if matches!(
                        app.drag,
                        Some(Drag::Move { .. }) | Some(Drag::Resize { .. })
                    ) && let Some(p) = pointer_img
                    {
                        app.drag_cur = Some(p);
                    }
                    if resp.drag_stopped() {
                        match (app.drag.take(), app.drag_cur.take()) {
                            (Some(Drag::Move { id, start }), Some(end)) => {
                                let (dx, dy) = (end.x - start.x, end.y - start.y);
                                if dx.abs() > 0.5 || dy.abs() > 0.5 {
                                    app.dispatch(
                                        "edit.move",
                                        serde_json::json!({"id": id, "dx": dx, "dy": dy}),
                                    );
                                }
                            }
                            (Some(Drag::Resize { id, anchor }), Some(end)) => {
                                let r = GRect::from_corners(anchor, end);
                                if r.w >= 2.0 && r.h >= 2.0 {
                                    app.dispatch(
                                        "edit.resize",
                                        serde_json::json!({"id": id, "rect": rect_json(r)}),
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                    // Live drag preview.
                    match (&app.drag, app.drag_cur) {
                        (Some(Drag::Move { id, start }), Some(cur)) => {
                            if let Some(obj) = app.sess().doc().and_then(|d| d.get(*id)) {
                                let b = obj.bounds();
                                let r = Rect::from_min_size(
                                    to_screen(Point::new(
                                        b.x + cur.x - start.x,
                                        b.y + cur.y - start.y,
                                    )),
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
                        (Some(Drag::Resize { anchor, .. }), Some(cur)) => {
                            let r = GRect::from_corners(*anchor, cur);
                            let sr = Rect::from_min_size(
                                to_screen(Point::new(r.x, r.y)),
                                vec2(r.w * scale, r.h * scale),
                            );
                            ui.painter().rect_stroke(
                                sr,
                                CornerRadius::same(3),
                                Stroke::new(1.0, ACCENT),
                                StrokeKind::Outside,
                            );
                        }
                        _ => {}
                    }
                    // Selection outline + corner handles for rect objects.
                    if let Some(r) = sel_rect {
                        let sr = Rect::from_min_size(
                            to_screen(Point::new(r.x, r.y)),
                            vec2(r.w * scale, r.h * scale),
                        );
                        ui.painter().rect_stroke(
                            sr.expand(3.0),
                            CornerRadius::same(3),
                            Stroke::new(1.0, ACCENT.linear_multiply(0.6)),
                            StrokeKind::Outside,
                        );
                        if !matches!(app.drag, Some(Drag::Resize { .. })) {
                            for c in rect_corners(r) {
                                let sc = to_screen(c);
                                ui.painter().rect_filled(
                                    Rect::from_center_size(sc, vec2(7.0, 7.0)),
                                    CornerRadius::same(1),
                                    ACCENT,
                                );
                            }
                        }
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
                            target: None,
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
                    // W14 live parity (tools.ts:1504-1507, MIN_DRAG=8): a plain
                    // click or tiny drag resets the crop instead of leaving a
                    // degenerate frame behind.
                    if resp.drag_stopped()
                        && let Some(frame) = app.crop
                        && (frame.w < 8.0 || frame.h < 8.0)
                    {
                        app.crop = None;
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
                    app.save_as_dialog();
                }
                if menu_row(ui, "Export PNG…", "") {
                    app.export_dialog();
                }
            });

            // ---- inline text editing ---------------------------------------
            if let Some(edit) = app.editing.as_ref().filter(|e| e.target.is_none()) {
                let pos = edit.pos;
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
    app.shortcuts_dialog(&ctx);
    app.about_dialog(&ctx);
    app.close_modal(&ctx);
    app.ocr_window(&ctx);
    app.timelapse_dialog(&ctx);
    app.new_canvas_dialog(&ctx);
    app.resize_dialog(&ctx);
    #[cfg(feature = "native")]
    app.monitor_picker_dialog(&ctx);
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

    /// W16: write a PNG to the system clipboard through the async Clipboard
    /// API. Needs user activation (the caller runs from a click/Ctrl+C) and
    /// clipboard-write permission; the browser rejects the promise otherwise
    /// and the caller surfaces the error with the Export fallback hint.
    pub async fn clipboard_write_png(bytes: &[u8]) -> Result<(), String> {
        let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let clipboard = window.navigator().clipboard();

        let parts = js_sys::Array::new();
        parts.push(&js_sys::Uint8Array::from(bytes));
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type("image/png");
        let blob =
            web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag).map_err(js_err)?;

        let record = js_sys::Object::new();
        js_sys::Reflect::set(&record, &"image/png".into(), &blob).map_err(js_err)?;
        let item = web_sys::ClipboardItem::new_with_record_from_str_to_blob_promise(&record)
            .map_err(js_err)?;
        let items = js_sys::Array::new();
        items.push(&item);
        wasm_bindgen_futures::JsFuture::from(clipboard.write(items.as_ref()))
            .await
            .map_err(js_err)?;
        Ok(())
    }

    /// W16: read the first `image/png` from the system clipboard. `Ok(None)`
    /// means the clipboard holds text or nothing — not an error. `read()`
    /// triggers the browser's clipboard-read permission prompt.
    pub async fn clipboard_read_png() -> Result<Option<Vec<u8>>, String> {
        let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let clipboard = window.navigator().clipboard();
        let value = wasm_bindgen_futures::JsFuture::from(clipboard.read())
            .await
            .map_err(js_err)?;
        let items: js_sys::Array = value
            .dyn_into()
            .map_err(|_| "clipboard.read() did not yield an array".to_string())?;
        for value in items.iter() {
            let item: web_sys::ClipboardItem = value
                .dyn_into()
                .map_err(|_| "clipboard entry is not a ClipboardItem".to_string())?;
            let has_png = item
                .types()
                .iter()
                .any(|t| t.as_string().is_some_and(|s| s == "image/png"));
            if !has_png {
                continue;
            }
            let value = wasm_bindgen_futures::JsFuture::from(item.get_type("image/png"))
                .await
                .map_err(js_err)?;
            let blob: web_sys::Blob = value
                .dyn_into()
                .map_err(|_| "getType() did not yield a Blob".to_string())?;
            let buffer = wasm_bindgen_futures::JsFuture::from(blob.array_buffer())
                .await
                .map_err(js_err)?;
            return Ok(Some(js_sys::Uint8Array::new(&buffer).to_vec()));
        }
        Ok(None)
    }
}

/// Host build without the `native` feature: there is no browser to hand a
/// download to, so the export path reports it instead of pretending.
#[cfg(all(not(feature = "native"), not(target_arch = "wasm32")))]
mod web {
    pub fn download(_name: &str, _mime: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("no browser in this build".to_string())
    }

    pub async fn clipboard_write_png(_bytes: &[u8]) -> Result<(), String> {
        Err("no browser in this build".to_string())
    }

    pub async fn clipboard_read_png() -> Result<Option<Vec<u8>>, String> {
        Err("no browser in this build".to_string())
    }
}
