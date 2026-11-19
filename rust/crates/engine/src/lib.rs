#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L5 engine: Session + command registry. Every user-visible action is a command
//! with a stable id and JSON params — the UI, CLI, control channel and MCP all
//! dispatch the same registry (PhotoCraft model).
//!
//! # Feature split (`native` / web core)
//!
//! The default `native` feature is the desktop engine, unchanged: 45 commands,
//! including capture, recording, scrolling capture, timelapse, OCR, upload and
//! clipboard. `--no-default-features` builds the web core, which compiles for
//! `wasm32-unknown-unknown`: `snagcraft-capture`, `snagcraft-recording` and
//! `snagcraft-io`'s arboard clipboard are dropped, their session state and
//! handlers disappear with them, and the registry exposes only the 22
//! doc/edit/settings commands.
//!
//! Native-only command ids are therefore **absent** from [`registry`] (and from
//! `commands.list`) on web rather than stubbed: [`Session::dispatch`] answers
//! `CommandError::UnknownCommand` for them. Core `doc.*` commands stay — they
//! compile to wasm and simply fail at runtime where `std::fs` has no backing
//! store; a web host talks to the doc/edit APIs directly.
//!
//! ## Web core commands
//! `session.info`, `commands.list`, `settings.get`, `settings.set`, `doc.new`,
//! `doc.resize`, `doc.open`, `doc.close`, `doc.inspect`, `doc.save`, `doc.save_as`,
//! `doc.load`, `doc.export`,
//! `edit.add`, `edit.remove`, `edit.move`, `edit.blur`, `edit.crop`,
//! `edit.bucket`, `edit.duplicate`, `edit.reorder`, `edit.undo`, `edit.redo`.

use serde_json::{Value, json};
use snagcraft_doc::{AnnotObject, BlurMode, Document};
use snagcraft_geom::{Point, Rect, Size};
use snagcraft_io::settings::Settings;
use snagcraft_ops::History;
use snagcraft_raster::Surface;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("no document open")]
    NoDoc,
    #[error("capture failed: {0}")]
    Capture(String),
    #[error("io: {0}")]
    Io(String),
    #[error("format: {0}")]
    Format(String),
}

pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub params_doc: &'static str,
    pub run: fn(&mut Session, Value) -> Result<Value, CommandError>,
}

pub struct Session {
    doc: Option<Document>,
    history: History,
    /// `history.depth()` when the document was last saved/exported/replaced.
    /// Dirty state is derived from this pair, never from a hand-set flag, so
    /// undoing back to the saved state makes the document clean again.
    saved_depth: usize,
    /// Bundled OFL faces for CPU text raster (family, bold, face). Built once
    /// from the text crate's embedded FontDb — identical in UI, CLI and export.
    faces: Vec<(String, bool, ab_glyph::FontArc)>,
    /// Persisted preferences, read once at startup. The engine never writes the
    /// file — persistence is the app's job after `settings.set` is dispatched.
    settings: Settings,
    // Everything below is desktop state: portal capture, recording and the
    // scrolling stitcher own OS handles, threads and event channels that have
    // no wasm equivalent, so the whole block is `native`-only. A web session is
    // just doc + history + settings.
    /// Running timelapse (W4c): handle + event channel, drained by timelapse.poll.
    #[cfg(feature = "native")]
    timelapse: Option<snagcraft_capture::timelapse::TimelapseHandle>,
    #[cfg(feature = "native")]
    timelapse_rx: Option<std::sync::mpsc::Receiver<snagcraft_capture::timelapse::TimelapseEvent>>,
    /// Running scrolling capture (W4b). spawn_scrolling blocks through the share
    /// dialog, so it runs on a thread; the handle arrives via scroll_start_rx.
    #[cfg(feature = "native")]
    scroll: Option<snagcraft_capture::scrolling::ScrollHandle>,
    #[cfg(feature = "native")]
    scroll_rx: Option<std::sync::mpsc::Receiver<snagcraft_capture::scrolling::ScrollEvent>>,
    #[cfg(feature = "native")]
    scroll_start_rx: Option<
        std::sync::mpsc::Receiver<
            Result<snagcraft_capture::scrolling::ScrollHandle, snagcraft_capture::CaptureError>,
        >,
    >,
    /// W20: a cancel arrived while the portal was still opening — apply it the
    /// moment the handle lands (the live card can cancel during the cast too).
    #[cfg(feature = "native")]
    scroll_cancel_pending: bool,
    /// Running recording (W4a). spawn_recording blocks through the share dialog,
    /// so it runs on a thread; the handle arrives via record_start_rx.
    #[cfg(feature = "native")]
    record: Option<snagcraft_recording::RecordingHandle>,
    #[cfg(feature = "native")]
    record_rx: Option<std::sync::mpsc::Receiver<snagcraft_recording::RecordingEvent>>,
    #[cfg(feature = "native")]
    record_start_rx: Option<
        std::sync::mpsc::Receiver<
            Result<snagcraft_recording::RecordingHandle, snagcraft_recording::RecordingError>,
        >,
    >,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let db = snagcraft_text::FontDb::bundled();
        let mut faces = Vec::new();
        for family in snagcraft_text::FontDb::families() {
            for bold in [false, true] {
                if let Some(face) = db.resolve_shared(family, bold) {
                    faces.push(((*family).to_string(), bold, face));
                }
            }
        }
        Self {
            doc: None,
            history: History::default(),
            saved_depth: 0,
            faces,
            settings: snagcraft_io::settings::load(),
            #[cfg(feature = "native")]
            timelapse: None,
            #[cfg(feature = "native")]
            timelapse_rx: None,
            #[cfg(feature = "native")]
            scroll: None,
            #[cfg(feature = "native")]
            scroll_rx: None,
            #[cfg(feature = "native")]
            scroll_start_rx: None,
            #[cfg(feature = "native")]
            scroll_cancel_pending: false,
            #[cfg(feature = "native")]
            record: None,
            #[cfg(feature = "native")]
            record_rx: None,
            #[cfg(feature = "native")]
            record_start_rx: None,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    pub fn doc(&self) -> Option<&Document> {
        self.doc.as_ref()
    }

    pub fn doc_mut(&mut self) -> Option<&mut Document> {
        self.doc.as_mut()
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Unsaved work: the history has moved since the last save/export/open.
    /// Undo back to the save point reports clean again, which a sticky flag
    /// could never do.
    pub fn dirty(&self) -> bool {
        self.history.depth() != self.saved_depth
    }

    /// Number of undo steps held — what `dirty` compares against.
    pub fn history_depth(&self) -> usize {
        self.history.depth()
    }

    fn require_doc(&self) -> Result<&Document, CommandError> {
        self.doc.as_ref().ok_or(CommandError::NoDoc)
    }

    /// Replace the document with a captured/opened surface (clears history).
    pub fn load_surface(&mut self, surface: Surface) {
        self.replace_doc(Document::new(surface));
    }

    fn replace_doc(&mut self, doc: Document) {
        self.doc = Some(doc);
        self.history.clear();
        self.saved_depth = 0;
    }

    pub fn dispatch(&mut self, id: &str, params: Value) -> Result<Value, CommandError> {
        let spec = registry().into_iter().find(|c| c.id == id);
        let result = match spec {
            Some(c) => (c.run)(self, params),
            None => Err(CommandError::UnknownCommand(id.to_string())),
        };
        // A successful write to disk is the save point the dirty flag compares
        // against; `doc.open`/`doc.new`/captures reset it in `replace_doc`.
        if result.is_ok() && matches!(id, "doc.save" | "doc.save_as" | "doc.export") {
            self.saved_depth = self.history.depth();
        }
        result
    }

    /// Composite the current document to a flat surface (export/preview).
    /// Per-object font family/bold from the bundled OFL db; italic = shear.
    /// Lookup mirrors FontDb::resolve semantics: exact family → Inter →
    /// first available (compose asks for ("", true) for step numbers).
    pub fn render(&self) -> Result<Surface, CommandError> {
        let doc = self.require_doc()?;
        let faces = &self.faces;
        let lookup = move |family: &str, bold: bool| {
            faces
                .iter()
                .find(|(f, b, _)| f.eq_ignore_ascii_case(family) && *b == bold)
                .or_else(|| {
                    faces
                        .iter()
                        .find(|(f, b, _)| f.eq_ignore_ascii_case("Inter") && *b == bold)
                })
                .or_else(|| faces.iter().find(|(_, b, _)| *b == bold))
                .or_else(|| faces.first())
                .map(|(_, _, face)| face)
        };
        Ok(snagcraft_compose::composite_styled(
            doc,
            &lookup,
            snagcraft_text::ITALIC_SHEAR,
        ))
    }

    /// Render the document and apply an optional watermark — the exact path
    /// `doc.export` uses, exposed so the web export (which encodes bytes in
    /// the browser rather than writing a file) produces identical output.
    /// `watermark` is the `{text, opacity, corner, size}` object; `None` or an
    /// empty/blank text renders plain.
    pub fn render_watermarked(&self, watermark: Option<&Value>) -> Result<Surface, CommandError> {
        let mut img = self.render()?;
        if let Some(w) = watermark.filter(|w| w.is_object()) {
            let text = w.get("text").and_then(Value::as_str).unwrap_or("");
            if !text.trim().is_empty() {
                let opacity = w.get("opacity").and_then(Value::as_f64).unwrap_or(0.45) as f32;
                let size = w.get("size").and_then(Value::as_f64).unwrap_or(24.0) as f32;
                let corner = match w
                    .get("corner")
                    .and_then(Value::as_str)
                    .unwrap_or("bottom-right")
                {
                    "bottom-left" => snagcraft_compose::Corner::BottomLeft,
                    "top-right" => snagcraft_compose::Corner::TopRight,
                    "top-left" => snagcraft_compose::Corner::TopLeft,
                    _ => snagcraft_compose::Corner::BottomRight,
                };
                let font = self.faces.first().map(|(_, _, f)| f);
                snagcraft_compose::apply_watermark(&mut img, text, opacity, corner, font, size);
            }
        }
        Ok(img)
    }
}

// ---- param helpers (hostile-input safe) ----

fn p_f32(v: &Value, key: &str) -> Result<f32, CommandError> {
    v.get(key)
        .and_then(Value::as_f64)
        .map(|x| x as f32)
        .filter(|x| x.is_finite())
        .ok_or_else(|| CommandError::InvalidParams(format!("missing/invalid float '{key}'")))
}

fn p_u64(v: &Value, key: &str) -> Result<u64, CommandError> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| CommandError::InvalidParams(format!("missing/invalid integer '{key}'")))
}

fn p_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, CommandError> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::InvalidParams(format!("missing/invalid string '{key}'")))
}

fn p_rect(v: &Value, key: &str) -> Result<snagcraft_geom::Rect, CommandError> {
    let r = v
        .get(key)
        .ok_or_else(|| CommandError::InvalidParams(format!("missing '{key}'")))?;
    Ok(snagcraft_geom::Rect::new(
        p_f32(r, "x")?,
        p_f32(r, "y")?,
        p_f32(r, "w")?,
        p_f32(r, "h")?,
    ))
}

fn p_color(v: &Value, key: &str) -> Result<snagcraft_color::Rgba, CommandError> {
    let c = v
        .get(key)
        .ok_or_else(|| CommandError::InvalidParams(format!("missing '{key}'")))?;
    let arr = c
        .as_array()
        .filter(|a| a.len() == 4)
        .ok_or_else(|| CommandError::InvalidParams(format!("'{key}' must be [r,g,b,a]")))?;
    let mut out = [0u8; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = arr
            .get(i)
            .and_then(Value::as_u64)
            .filter(|n| *n <= 255)
            .map(|n| n as u8)
            .ok_or_else(|| CommandError::InvalidParams(format!("'{key}[{i}]' must be 0..255")))?;
    }
    Ok(snagcraft_color::Rgba::from_array(out))
}

fn p_format(v: &Value) -> Result<snagcraft_io::ImageFormat, CommandError> {
    match v.get("format").and_then(Value::as_str).unwrap_or("png") {
        "png" => Ok(snagcraft_io::ImageFormat::Png),
        "jpeg" | "jpg" => Ok(snagcraft_io::ImageFormat::Jpeg),
        "webp" => Ok(snagcraft_io::ImageFormat::WebP),
        other => Err(CommandError::InvalidParams(format!(
            "unknown format '{other}'"
        ))),
    }
}

fn p_quality(v: &Value) -> u8 {
    v.get("quality")
        .and_then(Value::as_u64)
        .unwrap_or(90)
        .clamp(1, 100) as u8
}

/// `quality` when the caller sent one.
fn param_quality(v: &Value) -> Option<u8> {
    v.get("quality")
        .and_then(Value::as_u64)
        .map(|q| q.clamp(1, 100) as u8)
}

/// Format for a save: explicit `format` param, else the path's extension, else
/// `settings.default_format`. Unknown values are refused (explicit param) or
/// fall back to PNG (hostile settings file).
fn image_format(
    p: &Value,
    path: Option<&str>,
    settings: &Settings,
) -> Result<snagcraft_io::ImageFormat, CommandError> {
    use snagcraft_io::ImageFormat;
    if let Some(explicit) = p.get("format") {
        let name = explicit
            .as_str()
            .ok_or_else(|| CommandError::InvalidParams("format must be a string".into()))?;
        return match name.to_ascii_lowercase().as_str() {
            "png" => Ok(ImageFormat::Png),
            "jpeg" | "jpg" => Ok(ImageFormat::Jpeg),
            "webp" => Ok(ImageFormat::WebP),
            other => Err(CommandError::InvalidParams(format!(
                "unknown format '{other}'"
            ))),
        };
    }
    if let Some(ext) = path.and_then(|p| std::path::Path::new(p).extension())
        && let Some(ext) = ext.to_str()
    {
        match ext.to_ascii_lowercase().as_str() {
            "png" => return Ok(ImageFormat::Png),
            "jpg" | "jpeg" => return Ok(ImageFormat::Jpeg),
            "webp" => return Ok(ImageFormat::WebP),
            _ => {}
        }
    }
    Ok(match settings.default_format.as_str() {
        "jpeg" | "jpg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::WebP,
        _ => ImageFormat::Png,
    })
}

// ---- commands ----

fn cmd_session_info(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(match s.doc() {
        Some(d) => json!({
            "has_doc": true, "width": d.size.w, "height": d.size.h,
            "objects": d.objects.len(), "can_undo": s.can_undo(), "can_redo": s.can_redo(),
            "dirty": s.dirty(), "history_depth": s.history_depth(),
        }),
        None => json!({"has_doc": false, "can_undo": false, "can_redo": false, "dirty": false}),
    })
}

fn cmd_commands_list(_s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(Value::Array(
        registry()
            .into_iter()
            .map(|c| json!({"id": c.id, "label": c.label, "params": c.params_doc}))
            .collect(),
    ))
}

#[cfg(feature = "native")]
fn cmd_capture_full(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let surface = snagcraft_capture::capture_full_blocking()
        .map_err(|e| CommandError::Capture(e.to_string()))?;
    let (w, h) = (surface.width(), surface.height());
    s.replace_doc(Document::new(surface));
    Ok(json!({"width": w, "height": h}))
}

/// W25: `capture.monitors {}` — list connected outputs for the in-app picker.
/// Empty on Linux (the portal provides its own picker); populated on
/// Windows/macOS via xcap. The UI opens the picker only when this is non-empty.
#[cfg(feature = "native")]
fn cmd_capture_monitors(_s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let monitors =
        snagcraft_capture::list_monitors().map_err(|e| CommandError::Capture(e.to_string()))?;
    Ok(json!({ "monitors": monitors }))
}

/// W25: `capture.monitor {index}` — capture one specific output and open it.
/// The issue #1 fix: where no OS picker exists (Windows/macOS) the user picks
/// the screen in-app. Linux routes monitor choice through the portal picker.
#[cfg(feature = "native")]
fn cmd_capture_monitor(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let index =
        p.get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| CommandError::InvalidParams("missing 'index'".into()))? as usize;
    let surface = snagcraft_capture::capture_monitor_index(index)
        .map_err(|e| CommandError::Capture(e.to_string()))?;
    let (w, h) = (surface.width(), surface.height());
    s.replace_doc(Document::new(surface));
    Ok(json!({"width": w, "height": h, "index": index}))
}

fn cmd_doc_new(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let w = p_f32(&p, "w")?;
    let h = p_f32(&p, "h")?;
    if w < 1.0
        || h < 1.0
        || w > snagcraft_raster::MAX_DIM as f32
        || h > snagcraft_raster::MAX_DIM as f32
    {
        return Err(CommandError::InvalidParams("size out of range".into()));
    }
    // W23: the live dialog's white/transparent choice (default white).
    let transparent = p.get("bg").and_then(Value::as_str) == Some("transparent");
    let size = Size::new(w as u32, h as u32);
    s.replace_doc(if transparent {
        Document::transparent(size)
    } else {
        Document::white(size)
    });
    Ok(json!({"width": w as u32, "height": h as u32}))
}

/// `doc.resize {w,h}` — re-sample the base to a new canvas size and carry the
/// annotations with it, mirroring the live `resizeImage` (editor.ts:1000): the
/// ink follows the image's box, so every coordinate scales by `(sx, sy)` and
/// strokes/type by the geometric mean of the two. Hostile `w`/`h` are clamped
/// into `1..=MAX_DIM` instead of rejected, so no caller can allocate past cap.
fn cmd_doc_resize(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let w = p_f32(&p, "w")?;
    let h = p_f32(&p, "h")?;
    if !w.is_finite() || !h.is_finite() {
        return Err(CommandError::InvalidParams("size must be finite".into()));
    }
    let cap = snagcraft_raster::MAX_DIM as f32;
    let new_w = w.round().clamp(1.0, cap) as u32;
    let new_h = h.round().clamp(1.0, cap) as u32;
    let d = s.doc.as_ref().ok_or(CommandError::NoDoc)?;
    let (ow, oh) = (d.size.w, d.size.h);
    if ow == 0 || oh == 0 {
        return Err(CommandError::InvalidParams("document has no canvas".into()));
    }
    if new_w == ow && new_h == oh {
        return Ok(json!({"width": new_w, "height": new_h, "resized": false}));
    }
    let base = snagcraft_algo::resize_region(
        &d.base,
        Rect::new(0.0, 0.0, ow as f32, oh as f32),
        new_w,
        new_h,
    );
    let sx = new_w as f32 / ow as f32;
    let sy = new_h as f32 / oh as f32;
    let objects: Vec<AnnotObject> = d.objects.iter().map(|o| scale_object(o, sx, sy)).collect();
    let next_id = d.next_id;
    let mut next = Document::new(base);
    next.objects = objects;
    next.next_id = next_id;
    s.history.record(d);
    s.doc = Some(next);
    Ok(json!({"width": new_w, "height": new_h, "resized": true}))
}

/// Scale one annotation with the canvas — the engine twin of the live editor's
/// ink-rebase in `layout()`: points by `(sx, sy)`, pens/type by their mean.
fn scale_object(obj: &AnnotObject, sx: f32, sy: f32) -> AnnotObject {
    let s = (sx * sy).sqrt();
    let mut out = obj.clone();
    match &mut out {
        AnnotObject::Arrow {
            from, to, width, ..
        } => {
            *from = Point::new(from.x * sx, from.y * sy);
            *to = Point::new(to.x * sx, to.y * sy);
            *width *= s;
        }
        AnnotObject::Shape { rect, width, .. } => {
            scale_rect(rect, sx, sy);
            *width *= s;
        }
        AnnotObject::Text { pos, size, .. } => {
            *pos = Point::new(pos.x * sx, pos.y * sy);
            *size *= s;
        }
        AnnotObject::Blur { rect, radius, .. } => {
            scale_rect(rect, sx, sy);
            *radius = ((*radius as f32) * s).round().clamp(1.0, 512.0) as u32;
        }
        AnnotObject::Step { pos, radius, .. } => {
            *pos = Point::new(pos.x * sx, pos.y * sy);
            *radius *= s;
        }
        AnnotObject::Callout { rect, size, .. } => {
            scale_rect(rect, sx, sy);
            *size *= s;
        }
        AnnotObject::Highlighter { rect, .. }
        | AnnotObject::Spotlight { rect, .. }
        | AnnotObject::Magnify { rect, .. } => scale_rect(rect, sx, sy),
        AnnotObject::BrushStroke { points, width, .. } => {
            for pt in points.iter_mut() {
                *pt = Point::new(pt.x * sx, pt.y * sy);
            }
            *width *= s;
        }
    }
    out
}

fn scale_rect(rect: &mut Rect, sx: f32, sy: f32) {
    rect.x *= sx;
    rect.y *= sy;
    rect.w *= sx;
    rect.h *= sy;
}

fn cmd_doc_open(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let path = p_str(&p, "path")?;
    let surface = snagcraft_io::open(std::path::Path::new(path))
        .map_err(|e| CommandError::Io(e.to_string()))?;
    let (w, h) = (surface.width(), surface.height());
    s.replace_doc(Document::new(surface));
    Ok(json!({"width": w, "height": h}))
}

fn cmd_doc_close(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    s.doc = None;
    s.history.clear();
    Ok(json!({"closed": true}))
}

fn cmd_doc_inspect(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let d = s.require_doc()?;
    Ok(json!({"size": {"w": d.size.w, "h": d.size.h}, "next_id": d.next_id, "objects": d.objects}))
}

/// W19: a save suffix must stay a simple stem fragment — no separators, no
/// dots, bounded length — so a hostile param cannot escape the save folder.
fn sanitize_suffix(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect()
}

/// Insert `suffix` before the extension of `name` (no extension: append).
fn apply_suffix(name: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(i) => format!("{}{}.{}", &name[..i], suffix, &name[i + 1..]),
        None => format!("{name}{suffix}"),
    }
}

/// `doc.save` — with an explicit `path` this writes the editable `.snagcraft`
/// document (W1 behaviour, unchanged). Without one it is the desktop Save
/// button: render the watermarked composite, drop it in the save folder (the
/// `save_dir` setting, else `$HOME/Pictures/SnagCraft`) under the expanded
/// `filename_pattern` (plus the optional `suffix`, W19), and report
/// `{saved: <path>}`.
fn cmd_doc_save(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    if s.doc().is_none() {
        return Err(CommandError::NoDoc);
    }
    let explicit = p
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty());
    if let Some(path) = explicit {
        let d = s.require_doc()?;
        let bytes = snagcraft_format::save(d).map_err(|e| CommandError::Format(e.to_string()))?;
        std::fs::write(path, bytes).map_err(|e| CommandError::Io(e.to_string()))?;
        return Ok(json!({"saved": path}));
    }

    let settings = s.settings().clone();
    let format = image_format(&p, None, &settings)?;
    let quality = param_quality(&p).unwrap_or(settings.default_quality.clamp(1, 100));
    let dir = snagcraft_io::files::resolve_save_dir(settings.save_dir.as_deref());
    let name = snagcraft_io::files::build_filename_now(&settings.filename_pattern, format);
    // W19: the live Save writes a copy with an "-edited" suffix
    // (writeImage, app.ts:1811-1831); the caller supplies the suffix.
    let suffix = sanitize_suffix(p.get("suffix").and_then(Value::as_str).unwrap_or(""));
    let name = apply_suffix(&name, &suffix);
    let img = s.render_watermarked(p.get("watermark"))?;
    let path = snagcraft_io::recent::save_in_dir(&img, &dir, &name, format, quality)
        .map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"saved": path.to_string_lossy(), "dir": dir.to_string_lossy()}))
}

/// `doc.save_as {path, format?, quality?, watermark?}` — always writes a flat
/// image to the exact path the user chose (the Save As dialog's target).
///
/// `format` defaults to the path's extension, then to `settings.default_format`;
/// `quality` defaults to `settings.default_quality`. An empty path is refused
/// rather than silently writing into the working directory.
fn cmd_doc_save_as(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    if s.doc().is_none() {
        return Err(CommandError::NoDoc);
    }
    let path = p_str(&p, "path")?.trim().to_string();
    if path.is_empty() {
        return Err(CommandError::InvalidParams("empty save path".into()));
    }
    let settings = s.settings().clone();
    let format = image_format(&p, Some(&path), &settings)?;
    let quality = param_quality(&p).unwrap_or(settings.default_quality.clamp(1, 100));
    let img = s.render_watermarked(p.get("watermark"))?;
    snagcraft_io::save(&img, std::path::Path::new(&path), format, quality)
        .map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"saved": path, "format": format.extension()}))
}

fn cmd_doc_load(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let path = p_str(&p, "path")?;
    let bytes = std::fs::read(path).map_err(|e| CommandError::Io(e.to_string()))?;
    let doc = snagcraft_format::load(&bytes).map_err(|e| CommandError::Format(e.to_string()))?;
    let (w, h) = (doc.size.w, doc.size.h);
    s.replace_doc(doc);
    Ok(json!({"width": w, "height": h}))
}

fn cmd_doc_export(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let path = p_str(&p, "path")?;
    let format = p_format(&p)?;
    let quality = p_quality(&p);
    let img = s.render_watermarked(p.get("watermark"))?;
    snagcraft_io::save(&img, std::path::Path::new(path), format, quality)
        .map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"exported": path}))
}

#[cfg(feature = "native")]
fn cmd_ocr_available(_s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(json!({"available": snagcraft_io::ocr::ocr_available()}))
}

#[cfg(feature = "native")]
fn cmd_ocr_languages(_s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(json!({"languages": snagcraft_io::ocr::ocr_languages()}))
}

#[cfg(feature = "native")]
fn cmd_ocr_run(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let lang = p.get("lang").and_then(Value::as_str).unwrap_or("eng");
    let img = s.render()?;
    let text =
        snagcraft_io::ocr::run_ocr(&img, lang).map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"text": text}))
}

#[cfg(feature = "native")]
fn cmd_upload_open(_s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let url = p_str(&p, "url")?;
    snagcraft_io::upload::open_url(url).map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"opened": url}))
}

/// `upload.send {endpoint?}` — render the current document to PNG and POST it to
/// the user's own endpoint, returning `{link}`.
///
/// `endpoint` defaults to the persisted `settings.upload_url`; no document or no
/// endpoint is a caller mistake (`InvalidParams`), a hostile endpoint is too, and
/// only the transport/HTTP failure is `Io`. Nothing here knows any provider: the
/// user configures the URL, the app posts and hands back what came back.
#[cfg(feature = "native")]
fn cmd_upload_send(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    if s.doc.is_none() {
        return Err(CommandError::InvalidParams(
            "no image to upload; open or capture a document first".into(),
        ));
    }
    let explicit = p
        .get("endpoint")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty());
    let endpoint = match explicit {
        Some(endpoint) => endpoint.to_string(),
        None => s.settings.upload_url.trim().to_string(),
    };
    if endpoint.is_empty() {
        return Err(CommandError::InvalidParams(
            "no upload endpoint configured; set settings.upload_url".into(),
        ));
    }

    let img = s.render_watermarked(None)?;
    let png = snagcraft_io::encode(&img, snagcraft_io::ImageFormat::Png, 100)
        .map_err(|e| CommandError::Io(e.to_string()))?;
    let link = snagcraft_io::upload::upload_image(&endpoint, &png).map_err(|e| {
        use snagcraft_io::upload::UploadError;
        match e {
            UploadError::Endpoint | UploadError::NoImage => {
                CommandError::InvalidParams(e.to_string())
            }
            other => CommandError::Io(other.to_string()),
        }
    })?;
    Ok(json!({ "link": link }))
}

#[cfg(feature = "native")]
fn cmd_timelapse_start(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    if s.timelapse.is_some() {
        return Err(CommandError::InvalidParams(
            "timelapse already running".into(),
        ));
    }
    let period = p.get("period_secs").and_then(Value::as_u64).unwrap_or(30) as u32;
    let count = p.get("count").and_then(Value::as_u64).map(|c| c as u32);
    let dir = p_str(&p, "dir")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let config = snagcraft_capture::timelapse::TimelapseConfig {
        period_secs: period,
        count,
        dir: std::path::PathBuf::from(dir),
    };
    let handle = snagcraft_capture::timelapse::spawn_timelapse(config, tx)
        .map_err(|e| CommandError::Capture(e.to_string()))?;
    s.timelapse = Some(handle);
    s.timelapse_rx = Some(rx);
    Ok(json!({"started": true}))
}

#[cfg(feature = "native")]
fn cmd_timelapse_stop(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match &s.timelapse {
        Some(h) => {
            h.stop();
            Ok(json!({"stopped": true}))
        }
        None => Ok(json!({"stopped": false})),
    }
}

#[cfg(feature = "native")]
fn cmd_timelapse_poll(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let mut events = Vec::new();
    if let Some(rx) = &s.timelapse_rx {
        while let Ok(ev) = rx.try_recv() {
            let j = match ev {
                snagcraft_capture::timelapse::TimelapseEvent::Frame { path, index } => {
                    json!({"type": "frame", "path": path.to_string_lossy(), "index": index})
                }
                snagcraft_capture::timelapse::TimelapseEvent::Done { frames } => {
                    s.timelapse = None;
                    json!({"type": "done", "frames": frames})
                }
                snagcraft_capture::timelapse::TimelapseEvent::Failed(msg) => {
                    s.timelapse = None;
                    json!({"type": "failed", "message": msg})
                }
            };
            events.push(j);
        }
    }
    Ok(json!({"events": events, "running": s.timelapse.is_some()}))
}

#[cfg(feature = "native")]
fn cmd_timelapse_assemble(_s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let dir = p_str(&p, "dir")?;
    let dir_path = std::path::Path::new(dir);
    if !dir_path.is_dir() {
        return Err(CommandError::InvalidParams(format!(
            "frame folder does not exist: {dir}"
        )));
    }
    // Default glob = exactly the names timelapse.start writes.
    let pattern = p
        .get("pattern")
        .and_then(Value::as_str)
        .unwrap_or(snagcraft_capture::timelapse::FRAME_GLOB);
    let fps = p
        .get("fps")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .min(u64::from(u32::MAX)) as u32;
    let out = match p.get("out").and_then(Value::as_str) {
        Some(out) if !out.trim().is_empty() => std::path::PathBuf::from(out),
        _ => dir_path.join(snagcraft_recording::DEFAULT_OUTPUT_NAME),
    };
    let report =
        snagcraft_recording::assemble_timelapse(dir_path, pattern, fps, &out).map_err(|err| {
            match err {
                // Caller mistakes stay InvalidParams; only the child/disk is IO.
                snagcraft_recording::AssembleError::NoFrames(msg) => {
                    CommandError::InvalidParams(format!("no timelapse frames: {msg}"))
                }
                snagcraft_recording::AssembleError::Pattern(msg) => {
                    CommandError::InvalidParams(msg)
                }
                other => CommandError::Io(other.to_string()),
            }
        })?;
    Ok(json!({
        "video": report.video.to_string_lossy(),
        "frames": report.frames,
        "fps": report.fps,
    }))
}

/// W13: resolve a take's audio from explicit params, falling back to the
/// persisted settings (the W11 format precedent). `audio_source` names the
/// microphone; `system_audio` adds the default output's monitor. Both on means
/// one mixed AAC track; neither means a silent take.
#[cfg(feature = "native")]
fn record_audio_config(p: &Value, settings: &Settings) -> snagcraft_recording::AudioConfig {
    snagcraft_recording::AudioConfig {
        source: p
            .get("audio_source")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| settings.mic_source.clone()),
        system_audio: p
            .get("system_audio")
            .and_then(Value::as_bool)
            .unwrap_or(settings.system_audio),
    }
}

#[cfg(feature = "native")]
fn cmd_record_start(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    if s.record.is_some() || s.record_start_rx.is_some() {
        return Err(CommandError::InvalidParams(
            "recording already running".into(),
        ));
    }
    // W11: an omitted format follows the persisted recording-format setting.
    let setting_format = s.settings().recording_format.clone();
    let requested = p
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or(setting_format.as_str());
    let format = match requested {
        "mp4" => snagcraft_recording::RecordingFormat::Mp4,
        "gif" => snagcraft_recording::RecordingFormat::Gif,
        other => {
            return Err(CommandError::InvalidParams(format!(
                "unknown format '{other}'"
            )));
        }
    };
    let region = match p.get("region").filter(|r| r.is_object()) {
        Some(r) => Some(snagcraft_recording::Region {
            left: p_f32(r, "x")? as i32,
            top: p_f32(r, "y")? as i32,
            width: p_f32(r, "w")? as u32,
            height: p_f32(r, "h")? as u32,
        }),
        None => None,
    };
    let audio = record_audio_config(&p, s.settings());
    let fps = p.get("fps").and_then(Value::as_u64).unwrap_or(30) as u32;
    let output = p_str(&p, "output")?;
    let config = snagcraft_recording::RecordingConfig {
        format,
        region,
        audio,
        fps,
        output: std::path::PathBuf::from(output),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let (htx, hrx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = htx.send(snagcraft_recording::spawn_recording(config, tx));
    });
    s.record_rx = Some(rx);
    s.record_start_rx = Some(hrx);
    Ok(json!({"starting": true}))
}

#[cfg(feature = "native")]
fn cmd_record_stop(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match &s.record {
        Some(h) => {
            h.stop();
            Ok(json!({"stopping": true}))
        }
        None => Ok(json!({"stopping": false, "starting": s.record_start_rx.is_some()})),
    }
}

#[cfg(feature = "native")]
fn cmd_record_pause(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match &s.record {
        Some(h) => {
            h.pause();
            Ok(json!({"paused": true}))
        }
        None => Ok(json!({"paused": false})),
    }
}

#[cfg(feature = "native")]
fn cmd_record_resume(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match &s.record {
        Some(h) => {
            h.resume();
            Ok(json!({"resumed": true}))
        }
        None => Ok(json!({"resumed": false})),
    }
}

#[cfg(feature = "native")]
fn cmd_record_state(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(match &s.record {
        Some(h) => json!({"active": h.is_active(), "paused": h.is_paused()}),
        None => json!({"active": false, "paused": false, "starting": s.record_start_rx.is_some()}),
    })
}

#[cfg(feature = "native")]
fn cmd_record_poll(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let mut events = Vec::new();
    if let Some(hrx) = &s.record_start_rx {
        match hrx.try_recv() {
            Ok(Ok(handle)) => {
                s.record = Some(handle);
                s.record_start_rx = None;
                events.push(json!({"type": "handle"}));
            }
            Ok(Err(e)) => {
                s.record_start_rx = None;
                s.record_rx = None;
                events.push(json!({"type": "failed", "message": e.to_string()}));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                s.record_start_rx = None;
            }
        }
    }
    if let Some(rx) = &s.record_rx {
        while let Ok(ev) = rx.try_recv() {
            let j = match ev {
                snagcraft_recording::RecordingEvent::Started { width, height } => {
                    json!({"type": "started", "width": width, "height": height})
                }
                snagcraft_recording::RecordingEvent::Stopped { path } => {
                    s.record = None;
                    json!({"type": "stopped", "path": path.to_string_lossy()})
                }
                snagcraft_recording::RecordingEvent::Failed(msg) => {
                    s.record = None;
                    json!({"type": "failed", "message": msg})
                }
                snagcraft_recording::RecordingEvent::Paused => json!({"type": "paused"}),
                snagcraft_recording::RecordingEvent::Resumed => json!({"type": "resumed"}),
            };
            events.push(j);
        }
    }
    Ok(json!({"events": events, "running": s.record.is_some() || s.record_start_rx.is_some()}))
}

#[cfg(feature = "native")]
fn cmd_audio_sources(_s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let sources = snagcraft_recording::list_audio_sources()
        .map_err(|e| CommandError::Capture(e.to_string()))?;
    Ok(
        json!({"sources": sources.iter().map(|a| json!({"id": a.id, "name": a.name, "kind": a.kind})).collect::<Vec<_>>()}),
    )
}

#[cfg(feature = "native")]
fn cmd_scroll_start(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    if s.scroll.is_some() || s.scroll_start_rx.is_some() {
        return Err(CommandError::InvalidParams(
            "scrolling capture already running".into(),
        ));
    }
    let rect = p_rect(&p, "region")?;
    let max_height = p.get("max_height").and_then(Value::as_u64).unwrap_or(40000) as u32;
    let output = p_str(&p, "output")?;
    let config = snagcraft_capture::scrolling::ScrollConfig {
        region: rect,
        max_height,
        output: std::path::PathBuf::from(output),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let (htx, hrx) = std::sync::mpsc::channel();
    // spawn_scrolling blocks through the OS share dialog — never on the UI thread.
    std::thread::spawn(move || {
        let _ = htx.send(snagcraft_capture::scrolling::spawn_scrolling(config, tx));
    });
    s.scroll_rx = Some(rx);
    s.scroll_start_rx = Some(hrx);
    Ok(json!({"starting": true}))
}

#[cfg(feature = "native")]
fn cmd_scroll_stop(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match s.scroll.take() {
        Some(h) => {
            h.stop(); // graceful: stitches + saves what it has, then Done
            Ok(json!({"stopped": true}))
        }
        None => Ok(json!({"stopped": false, "starting": s.scroll_start_rx.is_some()})),
    }
}

/// W20: `scroll.cancel` — discard the partial scroll (the live card's Cancel,
/// `scrolling_cancel`). While the portal is still opening there is no handle
/// yet; the cancel is remembered and applied the moment one lands.
#[cfg(feature = "native")]
fn cmd_scroll_cancel(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match s.scroll.take() {
        Some(h) => {
            h.cancel(); // discards: no stitch, no file, then Cancelled
            Ok(json!({"cancelled": true}))
        }
        None => {
            let starting = s.scroll_start_rx.is_some();
            if starting {
                s.scroll_cancel_pending = true;
            }
            Ok(json!({"cancelled": starting, "starting": starting}))
        }
    }
}

#[cfg(feature = "native")]
fn cmd_scroll_poll(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let mut events = Vec::new();
    if let Some(hrx) = &s.scroll_start_rx {
        match hrx.try_recv() {
            Ok(Ok(handle)) => {
                s.scroll_start_rx = None;
                // W20: a cancel that arrived during the portal wait applies now.
                if s.scroll_cancel_pending {
                    s.scroll_cancel_pending = false;
                    s.scroll_rx = None;
                    handle.cancel();
                    events.push(json!({"type": "cancelled"}));
                } else {
                    s.scroll = Some(handle);
                    events.push(json!({"type": "started"}));
                }
            }
            Ok(Err(e)) => {
                s.scroll_start_rx = None;
                s.scroll_rx = None;
                events.push(json!({"type": "failed", "message": e.to_string()}));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                s.scroll_start_rx = None;
            }
        }
    }
    if let Some(rx) = &s.scroll_rx {
        while let Ok(ev) = rx.try_recv() {
            let j = match ev {
                snagcraft_capture::scrolling::ScrollEvent::Captured {
                    index,
                    total_height,
                } => {
                    json!({"type": "captured", "index": index, "total_height": total_height})
                }
                snagcraft_capture::scrolling::ScrollEvent::Done {
                    path,
                    width,
                    height,
                } => {
                    s.scroll = None;
                    json!({"type": "done", "path": path.to_string_lossy(), "width": width, "height": height})
                }
                snagcraft_capture::scrolling::ScrollEvent::Failed(msg) => {
                    s.scroll = None;
                    json!({"type": "failed", "message": msg})
                }
                snagcraft_capture::scrolling::ScrollEvent::Stopped => {
                    s.scroll = None;
                    json!({"type": "stopped"})
                }
                snagcraft_capture::scrolling::ScrollEvent::Cancelled => {
                    s.scroll = None;
                    json!({"type": "cancelled"})
                }
            };
            events.push(j);
        }
    }
    Ok(json!({"events": events, "running": s.scroll.is_some() || s.scroll_start_rx.is_some()}))
}

fn cmd_edit_add(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let obj_value = p
        .get("object")
        .ok_or_else(|| CommandError::InvalidParams("missing 'object'".into()))?;
    let obj: AnnotObject = serde_json::from_value(obj_value.clone())
        .map_err(|e| CommandError::InvalidParams(e.to_string()))?;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    let id = d.add(obj);
    Ok(json!({"id": id}))
}

fn cmd_edit_remove(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let id = p_u64(&p, "id")?;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    Ok(json!({"removed": d.remove(id)}))
}

fn cmd_edit_move(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let id = p_u64(&p, "id")?;
    let dx = p_f32(&p, "dx")?;
    let dy = p_f32(&p, "dy")?;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    Ok(json!({"moved": d.shift(id, dx, dy)}))
}

/// W14: replace a Text/Callout object's text (inline re-edit in Select mode).
fn cmd_edit_set_text(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let id = p_u64(&p, "id")?;
    let text = p
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| CommandError::InvalidParams("text must be a string".into()))?
        .to_string();
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    Ok(json!({"updated": d.set_text(id, text)}))
}

/// W14: resize a rect-based object (select-mode corner handles). Point/path
/// kinds report `resized: false` — they have no rect to set.
fn cmd_edit_resize(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let id = p_u64(&p, "id")?;
    let rect = p_rect(&p, "rect")?;
    if rect.w < 2.0 || rect.h < 2.0 {
        return Err(CommandError::InvalidParams(
            "the resized object needs at least 2x2 pixels".into(),
        ));
    }
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    Ok(json!({"resized": d.set_rect(id, rect)}))
}

fn cmd_edit_blur(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let rect = p_rect(&p, "rect")?;
    let mode = match p.get("mode").and_then(Value::as_str).unwrap_or("gaussian") {
        "gaussian" => BlurMode::Gaussian,
        "pixelate" => BlurMode::Pixelate,
        "blackout" => BlurMode::Blackout,
        other => {
            return Err(CommandError::InvalidParams(format!(
                "unknown blur mode '{other}'"
            )));
        }
    };
    let radius = p
        .get("radius")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(snagcraft_algo::BLUR_RADIUS_DEFAULT))
        .clamp(
            u64::from(snagcraft_algo::BLUR_RADIUS_MIN),
            u64::from(snagcraft_algo::BLUR_RADIUS_MAX),
        ) as u32;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    let id = d.add(AnnotObject::Blur {
        id: 0,
        rect,
        mode,
        radius,
    });
    Ok(json!({"id": id}))
}

fn cmd_edit_crop(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let rect = p_rect(&p, "rect")?;
    let d = s.doc.as_ref().ok_or(CommandError::NoDoc)?;
    let cropped = d.crop(rect).ok_or_else(|| {
        CommandError::InvalidParams("crop rect is empty or outside the image".into())
    })?;
    s.history.record(d);
    let (w, h) = (cropped.size.w, cropped.size.h);
    s.doc = Some(cropped);
    Ok(json!({"width": w, "height": h}))
}

fn cmd_edit_bucket(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let seed_obj = p
        .get("seed")
        .ok_or_else(|| CommandError::InvalidParams("missing 'seed'".into()))?;
    let seed = snagcraft_geom::Point::new(p_f32(seed_obj, "x")?, p_f32(seed_obj, "y")?);
    let color = p_color(&p, "color")?;
    let tolerance = p
        .get("tolerance")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(255) as u8;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    let filled = snagcraft_algo::flood_fill(&d.base, seed, color, tolerance);
    d.base = std::sync::Arc::new(filled);
    Ok(json!({"filled": true}))
}

/// Hostile-input caps for the diagnostics report: logs larger than this are
/// skipped with a note rather than read into memory.
#[cfg(feature = "native")]
const MAX_DIAG_LOG_BYTES: u64 = 1024 * 1024;

/// Longest log tail the report keeps per file.
#[cfg(feature = "native")]
const DIAG_TAIL_LINES: usize = 200;

/// Last `max` lines of `text` (bounded, never panics on hostile UTF-8).
#[cfg(feature = "native")]
fn tail_lines(text: &str, max: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(max);
    all[start..].join("\n")
}

/// Read a log tail, refusing oversized files. The error string is user-facing:
/// it lands in the pasted report exactly like the live command's.
#[cfg(feature = "native")]
fn log_tail(path: &std::path::Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_DIAG_LOG_BYTES => {
            format!("(skipped: {} bytes, over the 1 MiB cap)", meta.len())
        }
        Ok(_) => match std::fs::read_to_string(path) {
            Ok(text) => tail_lines(text.trim_end(), DIAG_TAIL_LINES),
            Err(e) => format!("(could not read: {e})"),
        },
        Err(e) => format!("(could not read: {e})"),
    }
}

/// Newest `*.rec.log` in `dir` (per-take recording log written by the encoder).
#[cfg(feature = "native")]
fn latest_rec_log(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".rec.log"))
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// The full diagnostics text (pure: no clipboard, no globals) — the live
/// `recording_diagnostics` content in `src-tauri/src/commands.rs:457-501`.
#[cfg(feature = "native")]
fn diagnostics_report(
    home: Option<&std::path::Path>,
    save_dir: &std::path::Path,
    version: &str,
    os: &str,
) -> String {
    let mut out = String::from("=== SnagCraft recording diagnostics ===\n");
    out.push_str(&format!("\nversion: {version}\nos: {os}\n"));
    let portal = home
        .map(|h| h.join(".cache").join("snagcraft-portal.log"))
        .unwrap_or_else(|| std::path::PathBuf::from("~/.cache/snagcraft-portal.log"));
    out.push_str(&format!("\n--- portal log ({}) ---\n", portal.display()));
    out.push_str(&log_tail(&portal));
    out.push('\n');
    match latest_rec_log(save_dir) {
        Some(path) => {
            out.push_str(&format!("\n--- latest take log ({}) ---\n", path.display()));
            out.push_str(&log_tail(&path));
        }
        None => out.push_str("\n(no per-take .rec.log found yet)"),
    }
    out
}

/// `PRETTY_NAME` from `/etc/os-release`, else the constant OS/ARCH pair.
#[cfg(feature = "native")]
fn os_description() -> String {
    let base = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    let Ok(text) = std::fs::read_to_string("/etc/os-release") else {
        return base;
    };
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|name| format!("{name} — {base}").replace('"', ""))
        .unwrap_or(base)
}

#[cfg(feature = "native")]
fn cmd_diagnostics_copy(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let save_dir = s
        .settings()
        .save_dir
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(snagcraft_io::files::default_save_dir);
    let text = diagnostics_report(
        home.as_deref(),
        &save_dir,
        env!("CARGO_PKG_VERSION"),
        &os_description(),
    );
    snagcraft_io::clipboard::copy_text(&text)
        .map_err(|e| CommandError::Io(format!("diagnostics: {e}")))?;
    Ok(json!({"copied": true, "bytes": text.len()}))
}

#[cfg(feature = "native")]
fn cmd_clipboard_copy(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let img = s.render()?;
    let (w, h) = (img.width(), img.height());
    snagcraft_io::clipboard::copy_image(&img).map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"copied": true, "width": w, "height": h}))
}

#[cfg(feature = "native")]
fn cmd_clipboard_paste(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match snagcraft_io::clipboard::paste_image().map_err(|e| CommandError::Io(e.to_string()))? {
        Some(img) => {
            let (w, h) = (img.width(), img.height());
            s.load_surface(img);
            Ok(json!({"pasted": true, "width": w, "height": h}))
        }
        None => Ok(json!({"pasted": false})),
    }
}

fn cmd_edit_duplicate(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let id = p_u64(&p, "id")?;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    let Some(obj) = d.get(id).cloned() else {
        return Err(CommandError::InvalidParams(format!(
            "no object with id {id}"
        )));
    };
    s.history.record(d);
    // Classic duplicate: offset so the copy is visibly distinct.
    let new_id = d.add(obj.shifted(16.0, 16.0));
    Ok(json!({"id": new_id}))
}

fn cmd_edit_reorder(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let id = p_u64(&p, "id")?;
    let to = p_str(&p, "to")?;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    let Some(pos) = d.objects.iter().position(|o| o.id() == id) else {
        return Err(CommandError::InvalidParams(format!(
            "no object with id {id}"
        )));
    };
    s.history.record(d);
    let obj = d.objects.remove(pos);
    match to {
        "front" => d.objects.push(obj),
        "back" => d.objects.insert(0, obj),
        other => {
            return Err(CommandError::InvalidParams(format!(
                "'to' must be front|back, got '{other}'"
            )));
        }
    }
    Ok(json!({"reordered": to}))
}

fn cmd_edit_undo(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let current = s.doc.clone().ok_or(CommandError::NoDoc)?;
    match s.history.undo(&current) {
        Some(prev) => {
            s.doc = Some(prev);
            Ok(json!({"undone": true}))
        }
        None => Ok(json!({"undone": false})),
    }
}

fn cmd_edit_redo(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let current = s.doc.clone().ok_or(CommandError::NoDoc)?;
    match s.history.redo(&current) {
        Some(next) => {
            s.doc = Some(next);
            Ok(json!({"redone": true}))
        }
        None => Ok(json!({"redone": false})),
    }
}

// ---- settings (L4 persistence lives in io; the engine only holds state) ----

fn settings_value(s: &Settings) -> Result<Value, CommandError> {
    serde_json::to_value(s)
        .map_err(|e| CommandError::InvalidParams(format!("settings serialize: {e}")))
}

fn cmd_settings_get(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    settings_value(s.settings())
}

/// Partial update: overlay the incoming object onto the current settings and
/// deserialize the result, so omitted fields stay untouched. Unknown keys are
/// ignored by serde; wrong types / out-of-range values are rejected and leave
/// the session's settings unchanged. Persistence is the app's job, not ours.
fn cmd_settings_set(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let incoming = p
        .get("settings")
        .ok_or_else(|| CommandError::InvalidParams("missing 'settings'".into()))?;
    let obj = incoming
        .as_object()
        .ok_or_else(|| CommandError::InvalidParams("'settings' must be an object".into()))?;
    let mut merged = settings_value(s.settings())?;
    let map = merged
        .as_object_mut()
        .ok_or_else(|| CommandError::InvalidParams("settings payload is not an object".into()))?;
    for (key, value) in obj {
        map.insert(key.clone(), value.clone());
    }
    let next: Settings = serde_json::from_value(merged)
        .map_err(|e| CommandError::InvalidParams(format!("invalid settings: {e}")))?;
    next.validate()
        .map_err(|e| CommandError::InvalidParams(e.to_string()))?;
    *s.settings_mut() = next;
    settings_value(s.settings())
}

/// The W1 command registry. Later waves extend this list (new modules per area).
pub fn registry() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "session.info",
            label: "Session info",
            params_doc: "{}",
            run: cmd_session_info,
        },
        CommandSpec {
            id: "commands.list",
            label: "List commands",
            params_doc: "{}",
            run: cmd_commands_list,
        },
        CommandSpec {
            id: "settings.get",
            label: "Get settings",
            params_doc: "{}",
            run: cmd_settings_get,
        },
        CommandSpec {
            id: "settings.set",
            label: "Update settings",
            params_doc: "{settings: partial Settings JSON}",
            run: cmd_settings_set,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "capture.full",
            label: "Capture full screen",
            params_doc: "{}",
            run: cmd_capture_full,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "capture.monitors",
            label: "List connected monitors",
            params_doc: "{} — empty on Linux (system picker); xcap list on Windows/macOS",
            run: cmd_capture_monitors,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "capture.monitor",
            label: "Capture one monitor by index",
            params_doc: "{index} — the in-app picker's choice (Windows/macOS)",
            run: cmd_capture_monitor,
        },
        CommandSpec {
            id: "doc.new",
            label: "New blank canvas",
            params_doc: "{w, h, bg?} — bg \"white\" (default) or \"transparent\"",
            run: cmd_doc_new,
        },
        CommandSpec {
            id: "doc.resize",
            label: "Resize canvas",
            params_doc: "{w, h} — clamped to 1..=16384; annotations scale with the canvas",
            run: cmd_doc_resize,
        },
        CommandSpec {
            id: "doc.open",
            label: "Open image",
            params_doc: "{path}",
            run: cmd_doc_open,
        },
        CommandSpec {
            id: "doc.close",
            label: "Close document",
            params_doc: "{}",
            run: cmd_doc_close,
        },
        CommandSpec {
            id: "doc.inspect",
            label: "Inspect document",
            params_doc: "{}",
            run: cmd_doc_inspect,
        },
        CommandSpec {
            id: "doc.save",
            label: "Save native .snagcraft",
            params_doc: "{path?, suffix?} — no path saves a flat image into the save folder; suffix (e.g. \"-edited\") is inserted before the extension",
            run: cmd_doc_save,
        },
        CommandSpec {
            id: "doc.save_as",
            label: "Save flattened image as",
            params_doc: "{path, format: png|jpeg|webp, quality}",
            run: cmd_doc_save_as,
        },
        CommandSpec {
            id: "doc.load",
            label: "Load native .snagcraft",
            params_doc: "{path}",
            run: cmd_doc_load,
        },
        CommandSpec {
            id: "doc.export",
            label: "Export flat image",
            params_doc: "{path, format: png|jpeg|webp, quality}",
            run: cmd_doc_export,
        },
        CommandSpec {
            id: "edit.add",
            label: "Add annotation object",
            params_doc: "{object: AnnotObject JSON}",
            run: cmd_edit_add,
        },
        CommandSpec {
            id: "edit.remove",
            label: "Remove object",
            params_doc: "{id}",
            run: cmd_edit_remove,
        },
        CommandSpec {
            id: "edit.move",
            label: "Move object",
            params_doc: "{id, dx, dy}",
            run: cmd_edit_move,
        },
        CommandSpec {
            id: "edit.set_text",
            label: "Edit object text",
            params_doc: "{id, text}",
            run: cmd_edit_set_text,
        },
        CommandSpec {
            id: "edit.resize",
            label: "Resize object",
            params_doc: "{id, rect:{x,y,w,h}}",
            run: cmd_edit_resize,
        },
        CommandSpec {
            id: "edit.blur",
            label: "Add blur region",
            params_doc: "{rect:{x,y,w,h}, mode, radius}",
            run: cmd_edit_blur,
        },
        CommandSpec {
            id: "edit.crop",
            label: "Crop document",
            params_doc: "{rect:{x,y,w,h}}",
            run: cmd_edit_crop,
        },
        CommandSpec {
            id: "edit.bucket",
            label: "Flood fill from seed",
            params_doc: "{seed:{x,y}, color:[r,g,b,a], tolerance}",
            run: cmd_edit_bucket,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "clipboard.copy",
            label: "Copy image to clipboard",
            params_doc: "{}",
            run: cmd_clipboard_copy,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "diagnostics.copy",
            label: "Copy recording diagnostics to the clipboard",
            params_doc: "{} — portal log tail + latest take log tail + version/OS",
            run: cmd_diagnostics_copy,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "clipboard.paste",
            label: "Paste image from clipboard",
            params_doc: "{}",
            run: cmd_clipboard_paste,
        },
        CommandSpec {
            id: "edit.duplicate",
            label: "Duplicate object",
            params_doc: "{id}",
            run: cmd_edit_duplicate,
        },
        CommandSpec {
            id: "edit.reorder",
            label: "Bring to front / send to back",
            params_doc: "{id, to: front|back}",
            run: cmd_edit_reorder,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "ocr.available",
            label: "OCR availability",
            params_doc: "{}",
            run: cmd_ocr_available,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "ocr.languages",
            label: "OCR languages",
            params_doc: "{}",
            run: cmd_ocr_languages,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "ocr.run",
            label: "Grab text from the current image",
            params_doc: "{lang?}",
            run: cmd_ocr_run,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "upload.open",
            label: "Open an http(s) URL in the browser",
            params_doc: "{url}",
            run: cmd_upload_open,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "upload.send",
            label: "Upload the current image to the configured endpoint",
            params_doc: "{endpoint?}",
            run: cmd_upload_send,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "timelapse.start",
            label: "Start interval capture",
            params_doc: "{period_secs, count?, dir}",
            run: cmd_timelapse_start,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "timelapse.stop",
            label: "Stop interval capture",
            params_doc: "{}",
            run: cmd_timelapse_stop,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "timelapse.poll",
            label: "Drain timelapse events",
            params_doc: "{}",
            run: cmd_timelapse_poll,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "timelapse.assemble",
            label: "Assemble timelapse frames into an MP4",
            params_doc: "{dir, pattern?, fps?, out?}",
            run: cmd_timelapse_assemble,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "record.start",
            label: "Start screen recording",
            params_doc: "{format?: mp4|gif (default from settings), region?, audio_source? (default: settings mic), system_audio? (default: settings), fps?, output}",
            run: cmd_record_start,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "record.stop",
            label: "Stop recording (finalize file)",
            params_doc: "{}",
            run: cmd_record_stop,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "record.pause",
            label: "Pause recording",
            params_doc: "{}",
            run: cmd_record_pause,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "record.resume",
            label: "Resume recording",
            params_doc: "{}",
            run: cmd_record_resume,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "record.state",
            label: "Recording state",
            params_doc: "{}",
            run: cmd_record_state,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "record.poll",
            label: "Drain recording events",
            params_doc: "{}",
            run: cmd_record_poll,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "audio.sources",
            label: "List audio sources",
            params_doc: "{}",
            run: cmd_audio_sources,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "scroll.start",
            label: "Start scrolling capture",
            params_doc: "{region:{x,y,w,h}, max_height?, output}",
            run: cmd_scroll_start,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "scroll.stop",
            label: "Stop scrolling capture (stitch + save)",
            params_doc: "{}",
            run: cmd_scroll_stop,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "scroll.cancel",
            label: "Cancel scrolling capture (discard)",
            params_doc: "{}",
            run: cmd_scroll_cancel,
        },
        #[cfg(feature = "native")]
        CommandSpec {
            id: "scroll.poll",
            label: "Drain scrolling capture events",
            params_doc: "{}",
            run: cmd_scroll_poll,
        },
        CommandSpec {
            id: "edit.undo",
            label: "Undo",
            params_doc: "{}",
            run: cmd_edit_undo,
        },
        CommandSpec {
            id: "edit.redo",
            label: "Redo",
            params_doc: "{}",
            run: cmd_edit_redo,
        },
    ]
}

// Tests exercise the full desktop registry (capture/record/scroll/ocr/clipboard
// are dispatched by id), so they are native-only. `--no-default-features` has no
// test target that could run on wasm32; keeping the module gated lets the web
// gate use `--all-targets`.
#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    #[test]
    fn record_audio_defaults_follow_the_persisted_settings() {
        let settings = Settings {
            mic_source: Some("alsa_input.usb-mic".to_string()),
            system_audio: true,
            ..Settings::default()
        };

        // Omitted params inherit both settings (W11 format precedent).
        let audio = record_audio_config(&json!({}), &settings);
        assert_eq!(audio.source_id(), Some("alsa_input.usb-mic"));
        assert!(audio.system_audio);
        assert_eq!(audio.input_count(), 2);

        // Explicit params win over the settings, including turning things off.
        let audio = record_audio_config(
            &json!({"audio_source": "alsa_input.other", "system_audio": false}),
            &settings,
        );
        assert_eq!(audio.source_id(), Some("alsa_input.other"));
        assert!(!audio.system_audio);
        assert_eq!(audio.input_count(), 1);

        // A blank mic id is no microphone, but system audio still records.
        let audio = record_audio_config(&json!({"audio_source": "  "}), &settings);
        assert_eq!(audio.source_id(), None);
        assert!(audio.system_audio);
        assert_eq!(audio.input_count(), 1);

        // Silent settings + silent params = a silent take, never an error.
        let quiet = Settings::default();
        let audio = record_audio_config(&json!({}), &quiet);
        assert_eq!(audio.input_count(), 0);
    }

    #[test]
    fn edit_set_text_and_resize_mutate_with_history() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 200, "h": 150})).unwrap();
        s.dispatch(
            "edit.add",
            json!({"object": {"kind": "Text", "id": 0,
                "pos": {"x": 10.0, "y": 10.0}, "text": "before",
                "color": [0, 0, 0, 255], "size": 20.0,
                "bold": false, "italic": false}}),
        )
        .unwrap();
        s.dispatch(
            "edit.add",
            json!({"object": {"kind": "Shape", "id": 0, "shape": "Rect",
                "rect": {"x": 20.0, "y": 20.0, "w": 50.0, "h": 40.0},
                "color": [0, 0, 0, 255], "width": 2.0}}),
        )
        .unwrap();

        // Wrong kinds report false without erroring.
        let r = s
            .dispatch(
                "edit.resize",
                json!({"id": 1, "rect": {"x": 0.0, "y": 0.0, "w": 30.0, "h": 30.0}}),
            )
            .unwrap();
        assert_eq!(r["resized"], false);
        let r = s
            .dispatch("edit.set_text", json!({"id": 2, "text": "nope"}))
            .unwrap();
        assert_eq!(r["updated"], false);

        // Text re-edit lands and undo restores the previous text.
        let r = s
            .dispatch("edit.set_text", json!({"id": 1, "text": "after"}))
            .unwrap();
        assert_eq!(r["updated"], true);
        let d = s.doc.as_ref().unwrap();
        assert!(matches!(d.get(1), Some(AnnotObject::Text { text, .. }) if text == "after"));
        s.dispatch("edit.undo", json!({})).unwrap();
        let d = s.doc.as_ref().unwrap();
        assert!(matches!(d.get(1), Some(AnnotObject::Text { text, .. }) if text == "before"));

        // Resize lands and undo restores the previous rect.
        let r = s
            .dispatch(
                "edit.resize",
                json!({"id": 2, "rect": {"x": 20.0, "y": 20.0, "w": 80.0, "h": 60.0}}),
            )
            .unwrap();
        assert_eq!(r["resized"], true);
        let d = s.doc.as_ref().unwrap();
        assert!(matches!(d.get(2), Some(AnnotObject::Shape { rect, .. }) if rect.w == 80.0));
        s.dispatch("edit.undo", json!({})).unwrap();
        let d = s.doc.as_ref().unwrap();
        assert!(matches!(d.get(2), Some(AnnotObject::Shape { rect, .. }) if rect.w == 50.0));

        // Degenerate rects are rejected; a missing doc is an error.
        assert!(
            s.dispatch(
                "edit.resize",
                json!({"id": 2, "rect": {"x": 0.0, "y": 0.0, "w": 1.0, "h": 40.0}})
            )
            .is_err()
        );
        let mut empty = Session::new();
        assert!(
            empty
                .dispatch("edit.set_text", json!({"id": 1, "text": "x"}))
                .is_err()
        );
    }

    #[test]
    fn new_add_undo_redo_inspect() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 64, "h": 48})).unwrap();
        let r = s.dispatch("edit.add", json!({"object": {"kind": "Shape", "id": 0, "shape": "Rect", "rect": {"x": 1.0, "y": 1.0, "w": 10.0, "h": 10.0}, "color": [0, 0, 0, 255], "width": 2.0}})).unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(s.doc().unwrap().objects.len(), 1);
        s.dispatch("edit.undo", json!({})).unwrap();
        assert!(s.doc().unwrap().objects.is_empty());
        s.dispatch("edit.redo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().objects.len(), 1);
        let info = s.dispatch("session.info", json!({})).unwrap();
        assert_eq!(info["objects"], 1);
        assert_eq!(info["width"], 64);
    }

    /// Dirty state is derived from history depth vs the last save point, so an
    /// edit dirties, an undo back to the save point cleans, and a fresh
    /// document/capture starts clean.
    #[test]
    fn dirty_tracks_history_depth() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 32, "h": 32})).unwrap();
        assert!(!s.dirty(), "a new document is clean");
        assert_eq!(
            s.dispatch("session.info", json!({})).unwrap()["dirty"],
            false
        );

        s.dispatch(
            "edit.add",
            json!({"object": {"kind": "Shape", "id": 0, "shape": "Rect",
            "rect": {"x": 1.0, "y": 1.0, "w": 4.0, "h": 4.0}, "color": [0, 0, 0, 255],
            "width": 1.0}}),
        )
        .unwrap();
        assert!(s.dirty());
        assert_eq!(
            s.dispatch("session.info", json!({})).unwrap()["dirty"],
            true
        );

        s.dispatch("edit.undo", json!({})).unwrap();
        assert!(!s.dirty(), "undoing back to the save point is clean again");

        s.dispatch("edit.redo", json!({})).unwrap();
        assert!(s.dirty(), "redo past the save point is dirty again");
    }

    /// A successful export/`.snagcraft` save is a save point: the same document
    /// with no further edits reports clean.
    #[test]
    fn save_marks_clean_and_later_edit_dirties() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 24, "h": 24})).unwrap();
        s.dispatch(
            "edit.add",
            json!({"object": {"kind": "Shape", "id": 0, "shape": "Rect",
            "rect": {"x": 1.0, "y": 1.0, "w": 4.0, "h": 4.0}, "color": [0, 0, 0, 255],
            "width": 1.0}}),
        )
        .unwrap();
        assert!(s.dirty());
        let dir = std::env::temp_dir().join("snagcraft-w12-dirty");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dirty.png");
        s.dispatch(
            "doc.export",
            json!({"path": path.to_string_lossy(), "format": "png", "quality": 90}),
        )
        .unwrap();
        assert!(!s.dirty(), "export is a save point");
        s.dispatch(
            "edit.add",
            json!({"object": {"kind": "Shape", "id": 0, "shape": "Rect",
            "rect": {"x": 2.0, "y": 2.0, "w": 4.0, "h": 4.0}, "color": [0, 0, 0, 255],
            "width": 1.0}}),
        )
        .unwrap();
        assert!(s.dirty(), "an edit after saving dirties again");
    }

    #[test]
    fn blur_command_adds_object() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 32, "h": 32})).unwrap();
        s.dispatch("edit.blur", json!({"rect": {"x": 2.0, "y": 2.0, "w": 10.0, "h": 10.0}, "mode": "pixelate", "radius": 5})).unwrap();
        match &s.doc().unwrap().objects[0] {
            AnnotObject::Blur { mode, radius, .. } => {
                assert_eq!(*mode, BlurMode::Pixelate);
                assert_eq!(*radius, 5);
            }
            _ => panic!("expected blur object"),
        }
    }

    #[test]
    fn native_save_load_round_trip() {
        let dir = std::env::temp_dir().join("snagcraft-engine-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rt.snagcraft");
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 16, "h": 16})).unwrap();
        s.dispatch(
            "edit.blur",
            json!({"rect": {"x": 0.0, "y": 0.0, "w": 8.0, "h": 8.0}}),
        )
        .unwrap();
        s.dispatch("doc.save", json!({"path": path.to_str().unwrap()}))
            .unwrap();
        s.dispatch("doc.close", json!({})).unwrap();
        assert!(s.doc().is_none());
        s.dispatch("doc.load", json!({"path": path.to_str().unwrap()}))
            .unwrap();
        assert_eq!(s.doc().unwrap().objects.len(), 1);
        std::fs::remove_file(&path).ok();
    }

    /// W9: no explicit path → `save_dir` + expanded `filename_pattern`, and a
    /// second save in the same second never overwrites the first.
    #[test]
    fn save_without_path_lands_in_save_dir_with_pattern() {
        let dir = std::env::temp_dir().join(format!("snagcraft-w9-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new();
        s.settings_mut().save_dir = Some(dir.to_string_lossy().to_string());
        s.settings_mut().filename_pattern = "shot-{date}".to_string();
        s.dispatch("doc.new", json!({"w": 12, "h": 9})).unwrap();

        let out = s.dispatch("doc.save", json!({})).unwrap();
        let saved = out["saved"].as_str().unwrap().to_string();
        assert!(
            saved.starts_with(&dir.to_string_lossy().to_string()),
            "{saved}"
        );
        let name = std::path::Path::new(&saved)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(name.starts_with("shot-"), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        let img = snagcraft_io::open(std::path::Path::new(&saved)).unwrap();
        assert_eq!(img.dimensions(), (12, 9));

        let again = s.dispatch("doc.save", json!({})).unwrap();
        let second = again["saved"].as_str().unwrap().to_string();
        assert_ne!(second, saved, "same-second saves must not overwrite");
        assert!(std::path::Path::new(&second).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W23: `doc.new` honors the live white/transparent background choice.
    #[test]
    fn doc_new_background_white_or_transparent() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 8, "h": 8})).unwrap();
        assert_eq!(
            s.doc().unwrap().base.get_pixel(0, 0).0[3],
            255,
            "default white"
        );
        s.dispatch("doc.new", json!({"w": 8, "h": 8, "bg": "transparent"}))
            .unwrap();
        assert_eq!(s.doc().unwrap().base.get_pixel(0, 0).0[3], 0, "transparent");
        s.dispatch("doc.new", json!({"w": 8, "h": 8, "bg": "white"}))
            .unwrap();
        assert_eq!(
            s.doc().unwrap().base.get_pixel(0, 0).0[3],
            255,
            "explicit white"
        );
        // Unknown bg falls back to white, never a transparent surprise.
        s.dispatch("doc.new", json!({"w": 8, "h": 8, "bg": "bogus"}))
            .unwrap();
        assert_eq!(
            s.doc().unwrap().base.get_pixel(0, 0).0[3],
            255,
            "fallback white"
        );
    }

    /// W19: the Save suffix lands before the extension, and a hostile suffix
    /// is sanitized to a plain stem fragment (no separators, no dots).
    #[test]
    fn save_suffix_lands_before_extension_and_is_sanitized() {
        let dir = std::env::temp_dir().join(format!("snagcraft-w19-suffix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new();
        s.settings_mut().save_dir = Some(dir.to_string_lossy().to_string());
        s.settings_mut().filename_pattern = "shot".to_string();
        s.dispatch("doc.new", json!({"w": 8, "h": 6})).unwrap();

        let out = s
            .dispatch("doc.save", json!({"suffix": "-edited"}))
            .unwrap();
        let name = std::path::Path::new(out["saved"].as_str().unwrap())
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(name, "shot-edited.png");

        // Hostile: separators and dots are stripped, the folder is never left.
        let out = s
            .dispatch("doc.save", json!({"suffix": "../evil/x."}))
            .unwrap();
        let saved = out["saved"].as_str().unwrap().to_string();
        assert!(saved.starts_with(&dir.to_string_lossy().to_string()));
        let name = std::path::Path::new(&saved)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(name, "shotevilx.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W9: `doc.save_as` writes a flat image to the exact path, infers the
    /// format from the extension, and refuses an empty path.
    #[test]
    fn save_as_writes_flat_image_with_format_inference() {
        let dir = std::env::temp_dir().join(format!("snagcraft-w9-saveas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 20, "h": 10})).unwrap();

        let png = dir.join("chosen.png");
        let out = s
            .dispatch("doc.save_as", json!({"path": png.to_str().unwrap()}))
            .unwrap();
        assert_eq!(out["format"], "png");
        assert_eq!(snagcraft_io::open(&png).unwrap().dimensions(), (20, 10));

        // The extension picks JPEG; the explicit param overrides both.
        let jpg = dir.join("chosen.jpg");
        s.dispatch("doc.save_as", json!({"path": jpg.to_str().unwrap()}))
            .unwrap();
        assert!(std::fs::metadata(&jpg).unwrap().len() > 0);
        let webp = dir.join("chosen-without-extension");
        let out = s
            .dispatch(
                "doc.save_as",
                json!({"path": webp.to_str().unwrap(), "format": "webp"}),
            )
            .unwrap();
        assert_eq!(out["format"], "webp");
        let bytes = std::fs::read(&webp).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");

        assert!(s.dispatch("doc.save_as", json!({"path": "  "})).is_err());
        assert!(
            s.dispatch(
                "doc.save_as",
                json!({"path": png.to_str().unwrap(), "format": "tiff"})
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_as_without_doc_is_no_doc() {
        let mut s = Session::new();
        s.dispatch("doc.close", json!({})).unwrap();
        assert!(matches!(
            s.dispatch("doc.save_as", json!({"path": "/tmp/x.png"})),
            Err(CommandError::NoDoc)
        ));
    }

    #[test]
    fn export_png_without_font() {
        let dir = std::env::temp_dir().join("snagcraft-engine-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("export.png");
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 16, "h": 16})).unwrap();
        s.dispatch(
            "doc.export",
            json!({"path": path.to_str().unwrap(), "format": "png"}),
        )
        .unwrap();
        let img = snagcraft_io::open(&path).unwrap();
        assert_eq!(img.dimensions(), (16, 16));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unknown_command_and_no_doc_errors() {
        let mut s = Session::new();
        assert!(matches!(
            s.dispatch("nope", json!({})),
            Err(CommandError::UnknownCommand(_))
        ));
        assert!(matches!(
            s.dispatch("edit.undo", json!({})),
            Err(CommandError::NoDoc)
        ));
        assert!(matches!(
            s.dispatch("doc.export", json!({"path": "/tmp/x.png"})),
            Err(CommandError::NoDoc)
        ));
    }

    #[test]
    fn adversarial_params_never_panic() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 8, "h": 8})).unwrap();
        let garbage = vec![
            json!({}),
            json!({"w": "huge", "h": null}),
            json!({"w": 1e30, "h": 1e30}),
            json!({"path": 123}),
            json!({"id": "x"}),
            json!({"object": {"kind": "Bogus"}}),
            json!({"rect": {"x": "NaN", "y": 0, "w": 1, "h": 1}}),
            json!({"mode": "laser"}),
            json!({"format": "tiff"}),
            json!({"quality": 999}),
        ];
        for cmd in registry() {
            if cmd.id == "capture.full" {
                continue; // live portal — not testable on a locked build host
            }
            if cmd.id == "upload.send" {
                continue; // network: the user's configured endpoint, if any
            }
            for g in &garbage {
                let _ = s.dispatch(cmd.id, g.clone());
            }
        }
    }

    #[test]
    fn crop_command_resizes_and_undoes() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 64, "h": 48})).unwrap();
        let r = s
            .dispatch(
                "edit.crop",
                json!({"rect": {"x": 8.0, "y": 8.0, "w": 32.0, "h": 24.0}}),
            )
            .unwrap();
        assert_eq!(r["width"], 32);
        assert_eq!(r["height"], 24);
        assert_eq!(s.doc().unwrap().size.w, 32);
        s.dispatch("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().size.w, 64);
        // empty / outside rect → error, doc untouched
        assert!(
            s.dispatch(
                "edit.crop",
                json!({"rect": {"x": 100.0, "y": 100.0, "w": 5.0, "h": 5.0}})
            )
            .is_err()
        );
        assert_eq!(s.doc().unwrap().size.w, 64);
    }

    #[test]
    fn bucket_command_fills_and_undoes() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 16, "h": 16})).unwrap();
        s.dispatch(
            "edit.bucket",
            json!({"seed": {"x": 8.0, "y": 8.0}, "color": [255, 0, 0, 255], "tolerance": 0}),
        )
        .unwrap();
        assert_eq!(s.doc().unwrap().base.get_pixel(8, 8).0, [255, 0, 0, 255]);
        s.dispatch("edit.undo", json!({})).unwrap();
        assert_eq!(
            s.doc().unwrap().base.get_pixel(8, 8).0,
            [255, 255, 255, 255]
        );
        // out-of-bounds seed: no-op, no panic
        s.dispatch(
            "edit.bucket",
            json!({"seed": {"x": 999.0, "y": 999.0}, "color": [0, 0, 0, 255]}),
        )
        .unwrap();
        assert_eq!(
            s.doc().unwrap().base.get_pixel(8, 8).0,
            [255, 255, 255, 255]
        );
    }

    #[test]
    fn blur_default_radius_is_parity_12() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 32, "h": 32})).unwrap();
        s.dispatch(
            "edit.blur",
            json!({"rect": {"x": 0.0, "y": 0.0, "w": 8.0, "h": 8.0}}),
        )
        .unwrap();
        match &s.doc().unwrap().objects[0] {
            snagcraft_doc::AnnotObject::Blur { radius, .. } => assert_eq!(*radius, 12),
            _ => panic!("expected blur"),
        }
    }

    #[test]
    fn render_uses_bundled_fonts_for_text() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 200, "h": 60})).unwrap();
        s.dispatch("edit.add", json!({"object": {"kind":"Text","id":0,"pos":{"x":5.0,"y":5.0},"text":"FontDb","color":[0,0,0,255],"size":24.0,"bold":false,"italic":false,"family":"Inter"}})).unwrap();
        let img = s.render().unwrap();
        let mut dark = 0;
        for y in 0..60 {
            for x in 0..200 {
                let p = img.get_pixel(x, y).0;
                if p[0] < 90 && p[1] < 90 && p[2] < 90 {
                    dark += 1;
                }
            }
        }
        assert!(
            dark > 50,
            "bundled font must rasterize text: {dark} dark px"
        );
    }

    #[test]
    fn step_number_rasterizes_via_font_fallback() {
        // compose asks the font callback for ("", true) for step numbers —
        // the lookup must fall back to Inter bold, not return None.
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 64, "h": 64})).unwrap();
        s.dispatch("edit.add", json!({"object": {"kind":"Step","id":0,"pos":{"x":32.0,"y":32.0},"number":1,"color":[0,122,255,255],"radius":16.0}})).unwrap();
        let img = s.render().unwrap();
        let mut white = 0;
        for y in 16..48 {
            for x in 16..48 {
                // Only count INSIDE the blue badge (r=16): the white base
                // outside the circle must not satisfy the assertion.
                let dx = x as f32 - 32.0;
                let dy = y as f32 - 32.0;
                if dx * dx + dy * dy > 144.0 {
                    continue;
                }
                let p = img.get_pixel(x, y).0;
                if p[0] > 230 && p[1] > 230 && p[2] > 230 {
                    white += 1;
                }
            }
        }
        assert!(white > 5, "step number glyph must render: {white} white px");
    }

    #[test]
    fn duplicate_offsets_and_undo() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 64, "h": 64})).unwrap();
        let r = s.dispatch("edit.add", json!({"object": {"kind":"Shape","id":0,"shape":"Rect","rect":{"x":1.0,"y":1.0,"w":10.0,"h":10.0},"color":[0,0,0,255],"width":2.0}})).unwrap();
        let id = r["id"].as_u64().unwrap();
        let dup = s.dispatch("edit.duplicate", json!({"id": id})).unwrap();
        let new_id = dup["id"].as_u64().unwrap();
        assert_ne!(id, new_id);
        assert_eq!(s.doc().unwrap().objects.len(), 2);
        let b = s.doc().unwrap().get(new_id).unwrap().bounds();
        assert!(b.x > 10.0, "duplicate offset: {b:?}");
        s.dispatch("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().objects.len(), 1);
        assert!(s.dispatch("edit.duplicate", json!({"id": 999})).is_err());
    }

    #[test]
    fn reorder_front_back() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 64, "h": 64})).unwrap();
        let a = s.dispatch("edit.add", json!({"object": {"kind":"Shape","id":0,"shape":"Rect","rect":{"x":0.0,"y":0.0,"w":5.0,"h":5.0},"color":[0,0,0,255],"width":1.0}})).unwrap()["id"].as_u64().unwrap();
        let b = s.dispatch("edit.add", json!({"object": {"kind":"Shape","id":0,"shape":"Rect","rect":{"x":0.0,"y":0.0,"w":5.0,"h":5.0},"color":[0,0,0,255],"width":1.0}})).unwrap()["id"].as_u64().unwrap();
        s.dispatch("edit.reorder", json!({"id": a, "to": "front"}))
            .unwrap();
        assert_eq!(s.doc().unwrap().objects.last().unwrap().id(), a);
        s.dispatch("edit.reorder", json!({"id": a, "to": "back"}))
            .unwrap();
        assert_eq!(s.doc().unwrap().objects.first().unwrap().id(), a);
        let _ = b;
        assert!(
            s.dispatch("edit.reorder", json!({"id": a, "to": "sideways"}))
                .is_err()
        );
        s.dispatch("edit.undo", json!({})).unwrap();
    }

    #[test]
    fn clipboard_commands_graceful_headless() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 8, "h": 8})).unwrap();
        // No clipboard session on the build host: must Err or Ok, never panic.
        let _ = s.dispatch("clipboard.copy", json!({}));
        let _ = s.dispatch("clipboard.paste", json!({}));
        assert!(s.doc().is_some());
    }

    #[test]
    fn ocr_and_upload_commands() {
        let mut s = Session::new();
        let a = s.dispatch("ocr.available", json!({})).unwrap();
        // Availability is host-dependent (tesseract is installed on the Linux CI
        // gates host, not on the Windows runner) — assert a well-formed answer,
        // not a specific value. The upload scheme guards below are the point.
        assert!(a["available"].is_boolean(), "ocr.available answers a bool");
        let l = s.dispatch("ocr.languages", json!({})).unwrap();
        assert!(l["languages"].as_array().is_some());
        // scheme guard: file:// must be rejected before any spawn
        assert!(
            s.dispatch("upload.open", json!({"url": "file:///etc/passwd"}))
                .is_err()
        );
        assert!(
            s.dispatch("upload.open", json!({"url": "javascript:alert(1)"}))
                .is_err()
        );
        assert!(s.dispatch("upload.open", json!({})).is_err());
    }

    fn have_curl() -> bool {
        std::process::Command::new("curl")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// One-shot local upload endpoint: records the raw request, replies with
    /// `response`. The body is read until idle because curl streams multipart
    /// from stdin with chunked encoding.
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn mock_upload_endpoint(response: &'static str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        use std::io::{Read, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(1500)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => request.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            stream.write_all(reply.as_bytes()).unwrap();
            let _ = stream.flush();
            request
        });
        (format!("http://{addr}/upload"), handle)
    }

    #[test]
    fn upload_send_posts_the_rendered_document_to_the_configured_endpoint() {
        if !have_curl() {
            eprintln!("skipping upload.send test: curl is not installed");
            return;
        }
        let (endpoint, server) = mock_upload_endpoint(r#"{"url":"https://example.com/i/abc.png"}"#);
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 12, "h": 9})).unwrap();
        s.settings_mut().upload_url = endpoint;

        let out = s.dispatch("upload.send", json!({})).unwrap();
        assert_eq!(out["link"], "https://example.com/i/abc.png");

        let request = server.join().unwrap();
        assert!(
            request.starts_with(b"POST /upload HTTP/1.1"),
            "not a multipart POST"
        );
        assert!(
            request.windows(4).any(|w| w == b"\x89PNG"),
            "the request body is not a PNG"
        );
    }

    #[test]
    fn upload_send_explicit_endpoint_overrides_settings() {
        if !have_curl() {
            eprintln!("skipping upload.send test: curl is not installed");
            return;
        }
        let (endpoint, server) = mock_upload_endpoint("https://example.com/override.png");
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 8, "h": 8})).unwrap();
        s.settings_mut().upload_url = "https://example.invalid/not-used".to_string();

        let out = s
            .dispatch("upload.send", json!({ "endpoint": endpoint }))
            .unwrap();
        assert_eq!(out["link"], "https://example.com/override.png");
        server.join().unwrap();
    }

    #[test]
    fn upload_send_requires_a_document_and_a_safe_endpoint() {
        let mut s = Session::new();
        // no document yet
        assert!(matches!(
            s.dispatch("upload.send", json!({})),
            Err(CommandError::InvalidParams(_))
        ));

        s.dispatch("doc.new", json!({"w": 8, "h": 8})).unwrap();
        s.settings_mut().upload_url = String::new();
        // nothing configured and no override
        assert!(matches!(
            s.dispatch("upload.send", json!({})),
            Err(CommandError::InvalidParams(_))
        ));
        // an explicit blank override falls back to settings, still empty
        assert!(matches!(
            s.dispatch("upload.send", json!({"endpoint": "   "})),
            Err(CommandError::InvalidParams(_))
        ));
        // hostile schemes never reach curl
        for endpoint in ["file:///etc/passwd", "javascript:alert(1)", "ftp://e.com/u"] {
            assert!(
                matches!(
                    s.dispatch("upload.send", json!({"endpoint": endpoint})),
                    Err(CommandError::InvalidParams(_))
                ),
                "{endpoint} must be rejected"
            );
        }
    }

    #[test]
    fn timelapse_commands_headless() {
        let mut s = Session::new();
        let stop = s.dispatch("timelapse.stop", json!({})).unwrap();
        assert_eq!(stop["stopped"], false);
        let poll = s.dispatch("timelapse.poll", json!({})).unwrap();
        assert_eq!(poll["running"], false);
        assert!(
            s.dispatch("timelapse.start", json!({"period_secs": 5}))
                .is_err()
        ); // missing dir
        // double-start guard: start once in a temp dir (portal capture will fail
        // headless, but the spawn itself must succeed), then reject a second.
        let dir = std::env::temp_dir().join("snagcraft-tl-test");
        let ok = s.dispatch(
            "timelapse.start",
            json!({"period_secs": 3600, "dir": dir.to_str().unwrap()}),
        );
        if ok.is_ok() {
            assert!(
                s.dispatch(
                    "timelapse.start",
                    json!({"period_secs": 3600, "dir": dir.to_str().unwrap()})
                )
                .is_err()
            );
            s.dispatch("timelapse.stop", json!({})).unwrap();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn timelapse_assemble_writes_an_h264_mp4() {
        if !snagcraft_recording::ffmpeg_available() {
            return; // skip-if-no-ffmpeg (offline evidence still runs on pn53)
        }
        let dir =
            std::env::temp_dir().join(format!("snagcraft-engine-assemble-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Odd frame size on purpose: the pad filter must make it even.
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 33, "h": 21})).unwrap();
        for index in 1..=5u32 {
            let frame = dir.join(format!("snagcraft-timelapse-{index:05}.png"));
            s.dispatch("doc.export", json!({"path": frame.to_str().unwrap()}))
                .unwrap();
        }
        // A decoy that the default glob must ignore.
        s.dispatch(
            "doc.export",
            json!({"path": dir.join("other.png").to_str().unwrap()}),
        )
        .unwrap();

        let r = s
            .dispatch(
                "timelapse.assemble",
                json!({"dir": dir.to_str().unwrap(), "fps": 12}),
            )
            .unwrap();
        assert_eq!(r["frames"], 5, "only the timelapse frames are counted");
        assert_eq!(r["fps"], 12);
        let video = r["video"].as_str().unwrap_or_default().to_string();
        assert_eq!(video, dir.join("timelapse.mp4").to_string_lossy());
        assert!(std::fs::metadata(&video).unwrap().len() > 1000);

        // ffprobe is the evidence: real h264, the frame count, even dimensions.
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,nb_frames,width,height",
                "-of",
                "default=nw=1",
                &video,
            ])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&probe.stdout);
        eprintln!("ffprobe {}: {}", video, text.trim().replace('\n', " "));
        assert!(text.contains("codec_name=h264"), "ffprobe said: {text}");
        assert!(text.contains("nb_frames=5"), "ffprobe said: {text}");
        assert!(text.contains("width=34"), "padded to even: {text}");
        assert!(text.contains("height=22"), "padded to even: {text}");
        // Evidence runs (`SNAGCRAFT_KEEP_ASSEMBLE_ARTIFACTS=1`) keep the MP4
        // around for a manual ffprobe; normal runs leave no scratch behind.
        if std::env::var_os("SNAGCRAFT_KEEP_ASSEMBLE_ARTIFACTS").is_none() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn timelapse_assemble_is_hostile_safe() {
        let dir = std::env::temp_dir().join(format!(
            "snagcraft-engine-assemble-hostile-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir_str = dir.to_str().unwrap().to_string();

        // Missing dir, empty dir, traversal, absolute path, shell metachars.
        assert!(matches!(
            s_dispatch_assemble(json!({"dir": dir.join("nope").to_str().unwrap()})),
            Err(CommandError::InvalidParams(_))
        ));
        assert!(matches!(
            s_dispatch_assemble(json!({"dir": dir_str})),
            Err(CommandError::InvalidParams(_))
        ));
        for pattern in [
            "../*.png",
            "..",
            "/etc/*.png",
            "*.png; rm -rf /",
            "$(id)*.png",
        ] {
            assert!(
                matches!(
                    s_dispatch_assemble(json!({"dir": dir_str, "pattern": pattern})),
                    Err(CommandError::InvalidParams(_))
                ),
                "pattern {pattern:?} must be rejected before any spawn"
            );
        }
        // No 'dir' at all, and a non-string dir.
        assert!(matches!(
            s_dispatch_assemble(json!({})),
            Err(CommandError::InvalidParams(_))
        ));
        assert!(matches!(
            s_dispatch_assemble(json!({"dir": 42})),
            Err(CommandError::InvalidParams(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn s_dispatch_assemble(params: Value) -> Result<Value, CommandError> {
        let mut s = Session::new();
        s.dispatch("timelapse.assemble", params)
    }

    #[test]
    fn export_watermark_changes_corner_pixels() {
        let dir = std::env::temp_dir().join("snagcraft-wm-test");
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain.png");
        let marked = dir.join("marked.png");
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 200, "h": 100})).unwrap();
        s.dispatch("doc.export", json!({"path": plain.to_str().unwrap()}))
            .unwrap();
        s.dispatch("doc.export", json!({"path": marked.to_str().unwrap(),
            "watermark": {"text": "SnagCraft", "opacity": 0.9, "corner": "bottom-right", "size": 20}})).unwrap();
        let a = snagcraft_io::open(&plain).unwrap();
        let b = snagcraft_io::open(&marked).unwrap();
        let mut diff = 0;
        for y in 60..100 {
            for x in 100..200 {
                if a.get_pixel(x, y) != b.get_pixel(x, y) {
                    diff += 1;
                }
            }
        }
        assert!(
            diff > 20,
            "watermark must alter the bottom-right region: {diff} px"
        );
        // top-left untouched
        assert_eq!(a.get_pixel(5, 5), b.get_pixel(5, 5));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scroll_commands_headless() {
        let mut s = Session::new();
        let stop = s.dispatch("scroll.stop", json!({})).unwrap();
        assert_eq!(stop["stopped"], false);
        // missing region → error before any thread
        assert!(
            s.dispatch("scroll.start", json!({"output": "/tmp/x.png"}))
                .is_err()
        );
        // valid config: start returns immediately (spawn blocks on the share
        // dialog in its thread — on a locked host it waits for the portal
        // timeout, so do NOT assert termination here).
        let dir = std::env::temp_dir().join("snagcraft-scroll-test");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("scroll.png");
        let r = s.dispatch("scroll.start", json!({"region": {"x": 0.0, "y": 0.0, "w": 200.0, "h": 200.0}, "output": out.to_str().unwrap()})).unwrap();
        assert_eq!(r["starting"], true);
        // double-start rejected while pending
        assert!(s.dispatch("scroll.start", json!({"region": {"x": 0.0, "y": 0.0, "w": 200.0, "h": 200.0}, "output": out.to_str().unwrap()})).is_err());
        let p = s.dispatch("scroll.poll", json!({})).unwrap();
        assert!(p["running"].as_bool().unwrap_or(false));
        let stop = s.dispatch("scroll.stop", json!({})).unwrap();
        assert_eq!(stop["stopped"], false); // handle not arrived yet
        assert_eq!(stop["starting"], true);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// W20: `scroll.cancel` is a graceful no-op with no run, and remembers the
    /// cancel while the portal is still opening (applied when the handle lands).
    #[test]
    fn scroll_cancel_headless() {
        let mut s = Session::new();
        let r = s.dispatch("scroll.cancel", json!({})).unwrap();
        assert_eq!(r["cancelled"], false);
        assert_eq!(r["starting"], false);

        let dir = std::env::temp_dir().join("snagcraft-scroll-cancel-test");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("scroll.png");
        s.dispatch(
            "scroll.start",
            json!({"region": {"x": 0.0, "y": 0.0, "w": 200.0, "h": 200.0}, "output": out.to_str().unwrap()}),
        )
        .unwrap();
        // Handle not arrived yet (portal wait): the cancel is pending.
        let r = s.dispatch("scroll.cancel", json!({})).unwrap();
        assert_eq!(r["cancelled"], true);
        assert_eq!(r["starting"], true);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_commands_headless() {
        let mut s = Session::new();
        assert_eq!(
            s.dispatch("record.stop", json!({})).unwrap()["stopping"],
            false
        );
        assert_eq!(
            s.dispatch("record.pause", json!({})).unwrap()["paused"],
            false
        );
        assert_eq!(
            s.dispatch("record.state", json!({})).unwrap()["active"],
            false
        );
        assert!(
            s.dispatch(
                "record.start",
                json!({"format": "avi", "output": "/tmp/x.avi"})
            )
            .is_err()
        );
        assert!(
            s.dispatch("record.start", json!({"format": "mp4"}))
                .is_err()
        );
        let a = s.dispatch("audio.sources", json!({})).unwrap();
        assert!(a["sources"].as_array().is_some());
        let dir = std::env::temp_dir().join("snagcraft-rec-test");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("take.mp4");
        let r = s
            .dispatch(
                "record.start",
                json!({"format": "mp4", "output": out.to_str().unwrap()}),
            )
            .unwrap();
        assert_eq!(r["starting"], true);
        assert!(
            s.dispatch(
                "record.start",
                json!({"format": "mp4", "output": out.to_str().unwrap()})
            )
            .is_err()
        );
        let p = s.dispatch("record.poll", json!({})).unwrap();
        assert!(p["running"].as_bool().unwrap_or(false));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn native_registry_keeps_all_51_commands() {
        // Desktop contract: every command the UI, CLI and MCP know is present.
        // Bump deliberately when a wave adds a command.
        let ids: Vec<&str> = registry().iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 51, "native registry drifted: {ids:?}");
        for native_only in [
            "capture.full",
            "capture.monitors",
            "capture.monitor",
            "record.start",
            "scroll.poll",
            "scroll.cancel",
            "timelapse.start",
            "timelapse.assemble",
            "ocr.run",
            "upload.open",
            "upload.send",
            "clipboard.copy",
            "audio.sources",
        ] {
            assert!(ids.contains(&native_only), "missing {native_only}");
        }
    }

    #[test]
    fn registry_ids_unique() {
        let ids: Vec<&str> = registry().iter().map(|c| c.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
    }

    // ---- settings ----

    #[test]
    fn settings_get_returns_defaults() {
        let mut s = Session::new();
        let got = s.dispatch("settings.get", json!({})).unwrap();
        assert_eq!(got, serde_json::to_value(s.settings()).unwrap());
        // A real settings file on the host would legitimately differ — only
        // assert parity defaults when nothing is persisted.
        if !snagcraft_io::settings::path().exists() {
            let expected = serde_json::to_value(Settings::default()).unwrap();
            assert_eq!(got, expected);
            assert_eq!(got["theme"], "Light");
            assert_eq!(got["default_quality"], 90);
            assert_eq!(got["default_format"], "png");
            assert_eq!(got["save_dir"], Value::Null);
            assert_eq!(got["filename_pattern"], "SnagCraft-{date}-{time}");
            assert_eq!(got["hotkey_region"], "SHIFT+Print");
        }
    }

    #[test]
    fn settings_set_partial_merge_keeps_other_fields() {
        let mut s = Session::new();
        *s.settings_mut() = Settings::default();
        s.settings_mut().hotkey_full = "CTRL+Print".to_string();

        let r = s
            .dispatch(
                "settings.set",
                json!({"settings": {"default_quality": 75, "theme": "Dark"}}),
            )
            .unwrap();
        assert_eq!(s.settings().default_quality, 75);
        assert_eq!(s.settings().theme, snagcraft_io::settings::Theme::Dark);
        assert_eq!(s.settings().default_format, "png");
        assert_eq!(s.settings().hotkey_full, "CTRL+Print");
        assert_eq!(s.settings().filename_pattern, "SnagCraft-{date}-{time}");
        // The reply is the applied settings, not just the patch.
        assert_eq!(r["default_quality"], 75);
        assert_eq!(r["hotkey_full"], "CTRL+Print");

        // save_dir round-trips through null → None and a string → Some.
        s.dispatch("settings.set", json!({"settings": {"save_dir": "/tmp/x"}}))
            .unwrap();
        assert_eq!(s.settings().save_dir.as_deref(), Some("/tmp/x"));
        s.dispatch("settings.set", json!({"settings": {"save_dir": null}}))
            .unwrap();
        assert_eq!(s.settings().save_dir, None);
    }

    #[test]
    fn settings_set_invalid_values_err_and_leave_state() {
        let mut s = Session::new();
        *s.settings_mut() = Settings::default();
        let bad = [
            json!({"default_quality": 0}),
            json!({"default_quality": 255}),
            json!({"default_quality": 300}),
            json!({"default_quality": -1}),
            json!({"default_quality": "high"}),
            json!({"default_format": "tiff"}),
            json!({"default_format": 42}),
        ];
        for patch in bad {
            assert!(
                s.dispatch("settings.set", json!({"settings": patch}))
                    .is_err(),
                "expected rejection for {patch}"
            );
            assert_eq!(s.settings(), &Settings::default());
        }
    }

    #[test]
    fn settings_commands_adversarial_params_never_panic() {
        let mut s = Session::new();
        let garbage = [
            Value::Null,
            json!([]),
            json!("settings"),
            json!(0),
            json!({"settings": null}),
            json!({"settings": []}),
            json!({"settings": "x"}),
            json!({"settings": {"theme": 3}}),
            json!({"settings": {"minimize_to_tray": "yes"}}),
            json!({"settings": {"unknown_future_key": 1}}),
            json!({"settings": {}}),
        ];
        for p in garbage {
            let _ = s.dispatch("settings.set", p);
        }
        let _ = s.dispatch("settings.get", Value::Null);
        let _ = s.dispatch("settings.get", json!([]));
        // Session state stays coherent and serializable.
        let got = s.dispatch("settings.get", json!({})).unwrap();
        assert!(got.is_object());
    }
}

#[cfg(test)]
mod w10_canvas_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp
    )]

    use super::*;

    fn seeded() -> Session {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 400, "h": 200})).unwrap();
        s.dispatch(
            "edit.add",
            json!({"object": {"kind": "Shape", "id": 0, "shape": "Rect",
                "rect": {"x": 100.0, "y": 50.0, "w": 80.0, "h": 40.0},
                "color": [255, 0, 0, 255], "width": 2.0}}),
        )
        .unwrap();
        s
    }

    #[test]
    fn doc_resize_scales_canvas_and_annotations() {
        let mut s = seeded();
        let out = s
            .dispatch("doc.resize", json!({"w": 800, "h": 400}))
            .unwrap();
        assert_eq!(out["width"], 800);
        assert_eq!(out["resized"], true);
        let d = s.doc().unwrap();
        assert_eq!((d.size.w, d.size.h), (800, 400));
        assert_eq!(d.base.dimensions(), (800, 400));
        assert_eq!(d.objects.len(), 1, "annotations survive the resize");
        // rect 100,50,80,40 at 2x, stroke 2 -> 4, bounds pad = width.
        let b = d.objects[0].bounds();
        assert!((b.x - 196.0).abs() < 0.5, "x scaled: {b:?}");
        assert!((b.y - 96.0).abs() < 0.5, "y scaled: {b:?}");
        assert!((b.w - 168.0).abs() < 0.5, "w scaled: {b:?}");
        assert_eq!(d.next_id, 2, "id counter is preserved");
        assert!(s.can_undo(), "resize is undoable like crop");
        s.dispatch("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().size.w, 400, "undo restores the canvas");
    }

    #[test]
    fn doc_resize_clamps_hostile_sizes_and_is_a_noop_at_the_same_size() {
        let mut s = seeded();
        // 0 clamps up to 1, 999_999 clamps down to MAX_DIM (no allocation bomb).
        let out = s
            .dispatch("doc.resize", json!({"w": 0, "h": 999_999}))
            .unwrap();
        assert_eq!(out["width"], 1);
        assert_eq!(out["height"], snagcraft_raster::MAX_DIM);
        let mut same = seeded();
        let out = same
            .dispatch("doc.resize", json!({"w": 400, "h": 200}))
            .unwrap();
        assert_eq!(out["resized"], false, "same size is a no-op");
        assert_eq!(same.doc().unwrap().size.w, 400);
    }

    #[cfg(feature = "native")]
    #[test]
    fn diagnostics_report_tails_portal_and_newest_take_log() {
        let root = std::env::temp_dir().join(format!("snagcraft-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shots = root.join("shots");
        std::fs::create_dir_all(shots.join(".cache")).unwrap();
        std::fs::create_dir_all(root.join(".cache")).unwrap();
        let portal = root.join(".cache").join("snagcraft-portal.log");

        // Missing logs are reported, not fatal.
        let empty = diagnostics_report(Some(&root), &shots, "9.9.9", "linux test");
        assert!(empty.contains("=== SnagCraft recording diagnostics ==="));
        assert!(empty.contains("version: 9.9.9"));
        assert!(empty.contains("os: linux test"));
        assert!(empty.contains("could not read"));
        assert!(empty.contains("(no per-take .rec.log found yet)"));

        // 300 lines in, only the last 200 come out.
        let body: String = (0..300).map(|i| format!("line-{i}\n")).collect();
        std::fs::write(&portal, &body).unwrap();
        let full = diagnostics_report(Some(&root), &shots, "9.9.9", "linux test");
        assert!(full.contains("line-299"), "newest line kept");
        assert!(
            !full.contains("line-99\n"),
            "line 99 falls outside the tail"
        );
        assert!(full.contains("line-100"), "tail starts at line 100");

        // The newest .rec.log wins.
        let older = shots.join("take-1.rec.log");
        let newer = shots.join("take-2.rec.log");
        std::fs::write(&older, "older take\n").unwrap();
        std::fs::write(&newer, "newer take\n").unwrap();
        let epoch = std::time::SystemTime::UNIX_EPOCH;
        std::fs::File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(epoch + std::time::Duration::from_secs(1_000))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&newer)
            .unwrap()
            .set_modified(epoch + std::time::Duration::from_secs(2_000))
            .unwrap();
        let picked = diagnostics_report(Some(&root), &shots, "9.9.9", "linux test");
        assert!(picked.contains("take-2.rec.log"), "newest take log picked");
        assert!(picked.contains("newer take"));
        assert!(!picked.contains("older take"));

        // An oversized portal log is skipped instead of read into memory.
        std::fs::write(&portal, vec![b'x'; MAX_DIAG_LOG_BYTES as usize + 1]).unwrap();
        let capped = diagnostics_report(Some(&root), &shots, "9.9.9", "linux test");
        assert!(capped.contains("over the 1 MiB cap"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn w11_settings_cluster_round_trips_through_the_engine() {
        let mut s = Session::new();
        let got = s
            .dispatch(
                "settings.set",
                json!({"settings": {
                    "interval_secs": 300, "mic_source": "alsa_input.test",
                    "system_audio": true, "hotkeys_enabled": true,
                    "recording_format": "gif", "split_folders": true,
                    "recording_dir": "/tmp/rec"}}),
            )
            .unwrap();
        assert_eq!(got["interval_secs"], 300);
        assert_eq!(got["mic_source"], "alsa_input.test");
        assert_eq!(got["system_audio"], true);
        assert_eq!(got["hotkeys_enabled"], true);
        assert_eq!(got["recording_format"], "gif");
        assert_eq!(got["split_folders"], true);
        assert_eq!(got["recording_dir"], "/tmp/rec");
        // A fresh read returns the same cluster.
        let again = s.dispatch("settings.get", json!({})).unwrap();
        assert_eq!(again["recording_format"], "gif");

        // Invalid values are rejected and leave the session untouched.
        assert!(
            s.dispatch(
                "settings.set",
                json!({"settings": {"recording_format": "mkv"}})
            )
            .is_err()
        );
        assert!(
            s.dispatch("settings.set", json!({"settings": {"interval_secs": 0}}))
                .is_err()
        );
        assert_eq!(
            s.dispatch("settings.get", json!({})).unwrap()["recording_format"],
            "gif"
        );
    }
}
