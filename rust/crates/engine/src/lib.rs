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

use serde_json::{Value, json};
use snagcraft_doc::{AnnotObject, BlurMode, Document};
use snagcraft_geom::Size;
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
    /// Bundled OFL faces for CPU text raster (family, bold, face). Built once
    /// from the text crate's embedded FontDb — identical in UI, CLI and export.
    faces: Vec<(String, bool, ab_glyph::FontArc)>,
    /// Persisted preferences, read once at startup. The engine never writes the
    /// file — persistence is the app's job after `settings.set` is dispatched.
    settings: Settings,
    /// Running timelapse (W4c): handle + event channel, drained by timelapse.poll.
    timelapse: Option<snagcraft_capture::timelapse::TimelapseHandle>,
    timelapse_rx: Option<std::sync::mpsc::Receiver<snagcraft_capture::timelapse::TimelapseEvent>>,
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
            faces,
            settings: snagcraft_io::settings::load(),
            timelapse: None,
            timelapse_rx: None,
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
    }

    pub fn dispatch(&mut self, id: &str, params: Value) -> Result<Value, CommandError> {
        let spec = registry().into_iter().find(|c| c.id == id);
        match spec {
            Some(c) => (c.run)(self, params),
            None => Err(CommandError::UnknownCommand(id.to_string())),
        }
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

// ---- commands ----

fn cmd_session_info(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(match s.doc() {
        Some(d) => json!({
            "has_doc": true, "width": d.size.w, "height": d.size.h,
            "objects": d.objects.len(), "can_undo": s.can_undo(), "can_redo": s.can_redo(),
        }),
        None => json!({"has_doc": false, "can_undo": false, "can_redo": false}),
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

fn cmd_capture_full(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let surface = snagcraft_capture::capture_full_blocking()
        .map_err(|e| CommandError::Capture(e.to_string()))?;
    let (w, h) = (surface.width(), surface.height());
    s.replace_doc(Document::new(surface));
    Ok(json!({"width": w, "height": h}))
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
    s.replace_doc(Document::white(Size::new(w as u32, h as u32)));
    Ok(json!({"width": w as u32, "height": h as u32}))
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

fn cmd_doc_save(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let d = s.require_doc()?;
    let path = p_str(&p, "path")?;
    let bytes = snagcraft_format::save(d).map_err(|e| CommandError::Format(e.to_string()))?;
    std::fs::write(path, bytes).map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"saved": path}))
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
    let mut img = s.render()?;
    if let Some(w) = p.get("watermark").filter(|w| w.is_object()) {
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
            let font = s.faces.first().map(|(_, _, f)| f);
            snagcraft_compose::apply_watermark(&mut img, text, opacity, corner, font, size);
        }
    }
    snagcraft_io::save(&img, std::path::Path::new(path), format, quality)
        .map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"exported": path}))
}

fn cmd_ocr_available(_s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(json!({"available": snagcraft_io::ocr::ocr_available()}))
}

fn cmd_ocr_languages(_s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    Ok(json!({"languages": snagcraft_io::ocr::ocr_languages()}))
}

fn cmd_ocr_run(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let lang = p.get("lang").and_then(Value::as_str).unwrap_or("eng");
    let img = s.render()?;
    let text =
        snagcraft_io::ocr::run_ocr(&img, lang).map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"text": text}))
}

fn cmd_upload_open(_s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let url = p_str(&p, "url")?;
    snagcraft_io::upload::open_url(url).map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"opened": url}))
}

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

fn cmd_timelapse_stop(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    match &s.timelapse {
        Some(h) => {
            h.stop();
            Ok(json!({"stopped": true}))
        }
        None => Ok(json!({"stopped": false})),
    }
}

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

fn cmd_clipboard_copy(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let img = s.render()?;
    let (w, h) = (img.width(), img.height());
    snagcraft_io::clipboard::copy_image(&img).map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"copied": true, "width": w, "height": h}))
}

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
        CommandSpec {
            id: "capture.full",
            label: "Capture full screen",
            params_doc: "{}",
            run: cmd_capture_full,
        },
        CommandSpec {
            id: "doc.new",
            label: "New white canvas",
            params_doc: "{w, h}",
            run: cmd_doc_new,
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
            params_doc: "{path}",
            run: cmd_doc_save,
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
        CommandSpec {
            id: "clipboard.copy",
            label: "Copy image to clipboard",
            params_doc: "{}",
            run: cmd_clipboard_copy,
        },
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
        CommandSpec {
            id: "ocr.available",
            label: "OCR availability",
            params_doc: "{}",
            run: cmd_ocr_available,
        },
        CommandSpec {
            id: "ocr.languages",
            label: "OCR languages",
            params_doc: "{}",
            run: cmd_ocr_languages,
        },
        CommandSpec {
            id: "ocr.run",
            label: "Grab text from the current image",
            params_doc: "{lang?}",
            run: cmd_ocr_run,
        },
        CommandSpec {
            id: "upload.open",
            label: "Open an http(s) URL in the browser",
            params_doc: "{url}",
            run: cmd_upload_open,
        },
        CommandSpec {
            id: "timelapse.start",
            label: "Start interval capture",
            params_doc: "{period_secs, count?, dir}",
            run: cmd_timelapse_start,
        },
        CommandSpec {
            id: "timelapse.stop",
            label: "Stop interval capture",
            params_doc: "{}",
            run: cmd_timelapse_stop,
        },
        CommandSpec {
            id: "timelapse.poll",
            label: "Drain timelapse events",
            params_doc: "{}",
            run: cmd_timelapse_poll,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(a["available"], true, "tesseract installed on this host");
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
