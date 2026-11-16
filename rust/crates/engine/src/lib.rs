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
        }
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
    pub fn render(&self) -> Result<Surface, CommandError> {
        let doc = self.require_doc()?;
        let faces = &self.faces;
        let lookup = move |family: &str, bold: bool| {
            faces
                .iter()
                .find(|(f, b, _)| f.eq_ignore_ascii_case(family) && *b == bold)
                .map(|(_, _, face)| face)
        };
        Ok(snagcraft_compose::composite_styled(doc, &lookup, snagcraft_text::ITALIC_SHEAR))
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
    let c = v.get(key).ok_or_else(|| CommandError::InvalidParams(format!("missing '{key}'")))?;
    let arr = c.as_array().filter(|a| a.len() == 4)
        .ok_or_else(|| CommandError::InvalidParams(format!("'{key}' must be [r,g,b,a]")))?;
    let mut out = [0u8; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = arr.get(i).and_then(Value::as_u64).filter(|n| *n <= 255).map(|n| n as u8)
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
    let img = s.render()?;
    snagcraft_io::save(&img, std::path::Path::new(path), format, quality)
        .map_err(|e| CommandError::Io(e.to_string()))?;
    Ok(json!({"exported": path}))
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
    let cropped = d.crop(rect).ok_or_else(|| CommandError::InvalidParams("crop rect is empty or outside the image".into()))?;
    s.history.record(d);
    let (w, h) = (cropped.size.w, cropped.size.h);
    s.doc = Some(cropped);
    Ok(json!({"width": w, "height": h}))
}

fn cmd_edit_bucket(s: &mut Session, p: Value) -> Result<Value, CommandError> {
    let seed_obj = p.get("seed").ok_or_else(|| CommandError::InvalidParams("missing 'seed'".into()))?;
    let seed = snagcraft_geom::Point::new(p_f32(seed_obj, "x")?, p_f32(seed_obj, "y")?);
    let color = p_color(&p, "color")?;
    let tolerance = p.get("tolerance").and_then(Value::as_u64).unwrap_or(0).min(255) as u8;
    let d = s.doc.as_mut().ok_or(CommandError::NoDoc)?;
    s.history.record(d);
    let filled = snagcraft_algo::flood_fill(&d.base, seed, color, tolerance);
    d.base = std::sync::Arc::new(filled);
    Ok(json!({"filled": true}))
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
        let r = s.dispatch("edit.crop", json!({"rect": {"x": 8.0, "y": 8.0, "w": 32.0, "h": 24.0}})).unwrap();
        assert_eq!(r["width"], 32);
        assert_eq!(r["height"], 24);
        assert_eq!(s.doc().unwrap().size.w, 32);
        s.dispatch("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().size.w, 64);
        // empty / outside rect → error, doc untouched
        assert!(s.dispatch("edit.crop", json!({"rect": {"x": 100.0, "y": 100.0, "w": 5.0, "h": 5.0}})).is_err());
        assert_eq!(s.doc().unwrap().size.w, 64);
    }

    #[test]
    fn bucket_command_fills_and_undoes() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 16, "h": 16})).unwrap();
        s.dispatch("edit.bucket", json!({"seed": {"x": 8.0, "y": 8.0}, "color": [255, 0, 0, 255], "tolerance": 0})).unwrap();
        assert_eq!(s.doc().unwrap().base.get_pixel(8, 8).0, [255, 0, 0, 255]);
        s.dispatch("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc().unwrap().base.get_pixel(8, 8).0, [255, 255, 255, 255]);
        // out-of-bounds seed: no-op, no panic
        s.dispatch("edit.bucket", json!({"seed": {"x": 999.0, "y": 999.0}, "color": [0, 0, 0, 255]})).unwrap();
        assert_eq!(s.doc().unwrap().base.get_pixel(8, 8).0, [255, 255, 255, 255]);
    }

    #[test]
    fn blur_default_radius_is_parity_12() {
        let mut s = Session::new();
        s.dispatch("doc.new", json!({"w": 32, "h": 32})).unwrap();
        s.dispatch("edit.blur", json!({"rect": {"x": 0.0, "y": 0.0, "w": 8.0, "h": 8.0}})).unwrap();
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
        for y in 0..60 { for x in 0..200 { let p = img.get_pixel(x, y).0; if p[0] < 90 && p[1] < 90 && p[2] < 90 { dark += 1; } } }
        assert!(dark > 50, "bundled font must rasterize text: {dark} dark px");
    }

    #[test]
    fn registry_ids_unique() {
        let ids: Vec<&str> = registry().iter().map(|c| c.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len());
    }
}
