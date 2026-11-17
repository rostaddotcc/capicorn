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
//! The default `native` feature is the desktop engine, unchanged: 43 commands,
//! including capture, recording, scrolling capture, timelapse, OCR, upload and
//! clipboard. `--no-default-features` builds the web core, which compiles for
//! `wasm32-unknown-unknown`: `snagcraft-capture`, `snagcraft-recording` and
//! `snagcraft-io`'s arboard clipboard are dropped, their session state and
//! handlers disappear with them, and the registry exposes only the 21
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
//! `doc.open`, `doc.close`, `doc.inspect`, `doc.save`, `doc.load`, `doc.export`,
//! `edit.add`, `edit.remove`, `edit.move`, `edit.blur`, `edit.crop`,
//! `edit.bucket`, `edit.duplicate`, `edit.reorder`, `edit.undo`, `edit.redo`.

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

#[cfg(feature = "native")]
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

#[cfg(feature = "native")]
fn cmd_record_start(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    if s.record.is_some() || s.record_start_rx.is_some() {
        return Err(CommandError::InvalidParams(
            "recording already running".into(),
        ));
    }
    let format = match p.get("format").and_then(Value::as_str).unwrap_or("mp4") {
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
    let audio = snagcraft_recording::AudioConfig {
        source: p
            .get("audio_source")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
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

#[cfg(feature = "native")]
fn cmd_scroll_poll(s: &mut Session, _p: Value) -> Result<Value, CommandError> {
    let mut events = Vec::new();
    if let Some(hrx) = &s.scroll_start_rx {
        match hrx.try_recv() {
            Ok(Ok(handle)) => {
                s.scroll = Some(handle);
                s.scroll_start_rx = None;
                events.push(json!({"type": "started"}));
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
        #[cfg(feature = "native")]
        CommandSpec {
            id: "clipboard.copy",
            label: "Copy image to clipboard",
            params_doc: "{}",
            run: cmd_clipboard_copy,
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
            params_doc: "{format: mp4|gif, region?, audio_source?, fps?, output}",
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
    fn native_registry_keeps_all_43_commands() {
        // Desktop contract: every command the UI, CLI and MCP know is present.
        // Bump deliberately when a wave adds a command.
        let ids: Vec<&str> = registry().iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 43, "native registry drifted: {ids:?}");
        for native_only in [
            "capture.full",
            "record.start",
            "scroll.poll",
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
