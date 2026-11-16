#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L3 — the native `.snagcraft` document format, version 1.
//!
//! Layout is JSON (readable, diffable, forward-inspectable):
//!
//! ```json
//! {
//!   "version": 1,
//!   "size": { "w": 1280, "h": 720 },
//!   "next_id": 9,
//!   "objects": [ { "kind": "Arrow", "id": 1, "from": {...}, "to": {...}, ... } ],
//!   "base_png": "<base64 of a PNG-encoded RGBA base image>"
//! }
//! ```
//!
//! The base image is PNG (lossless, so a save/load cycle is pixel-exact); the annotation
//! objects keep their plain serde shape so the same JSON is also readable by tooling.
//!
//! ## Compile-time field guard
//!
//! `doc → manifest` is written by destructuring [`Document`] **without** a `..` rest pattern and
//! by naming **every** field of **every** [`AnnotObject`] variant in its match arm. Adding a field
//! to the document model therefore breaks compilation *here*, instead of silently dropping it from
//! saved files. Do not "fix" such a break with `..` or `_` — extend the mapping.

use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde_json::{Value, json};
use snagcraft_doc::{AnnotObject, Document, ObjId};
use snagcraft_geom::Size;
use snagcraft_raster::{MAX_DIM, Surface};

/// On-disk format revision written by [`save`]; newer revisions are refused by [`load`].
pub const FORMAT_VERSION: u32 = 1;

/// Hostile-input cap on manifest bytes (256 MB).
pub const MAX_INPUT_BYTES: usize = 256 * 1024 * 1024;

/// Hostile-input cap on annotation objects in one document.
pub const MAX_OBJECTS: usize = 100_000;

/// L3 errors. A malformed or hostile file is always an `Err`, never a panic.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("io error: {0}")]
    Io(String),
    #[error("invalid manifest json: {0}")]
    Json(String),
    #[error("invalid base64 payload: {0}")]
    Base64(String),
    /// The file is newer than this build understands: `file version` reported.
    #[error("unsupported .snagcraft version {0} (this build writes {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
    #[error("invalid base image: {0}")]
    InvalidImage(String),
    #[error("document is too large")]
    TooLarge,
}

/// What [`load`] deserializes before the base image is decoded.
#[derive(Debug, Deserialize)]
struct RawManifest {
    version: u32,
    size: Size,
    next_id: ObjId,
    objects: Vec<AnnotObject>,
    base_png: String,
}

/// Just enough of the manifest to check the version before trusting the rest of the schema.
#[derive(Debug, Deserialize)]
struct VersionProbe {
    version: u32,
}

/// Serialize `doc` into `.snagcraft` bytes.
pub fn save(doc: &Document) -> Result<Vec<u8>, FormatError> {
    let manifest = manifest_json(doc)?;
    serde_json::to_vec(&manifest).map_err(|e| FormatError::Json(e.to_string()))
}

/// Parse `.snagcraft` bytes back into a [`Document`], preserving ids and `next_id`.
pub fn load(bytes: &[u8]) -> Result<Document, FormatError> {
    if bytes.is_empty() {
        return Err(FormatError::Json("empty input".to_string()));
    }
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(FormatError::TooLarge);
    }

    let probe: VersionProbe =
        serde_json::from_slice(bytes).map_err(|e| FormatError::Json(e.to_string()))?;
    if probe.version != FORMAT_VERSION {
        return Err(FormatError::UnsupportedVersion(probe.version));
    }

    let raw: RawManifest =
        serde_json::from_slice(bytes).map_err(|e| FormatError::Json(e.to_string()))?;
    // Re-checked on the real parse so the field is load-bearing, not decoration.
    if raw.version != FORMAT_VERSION {
        return Err(FormatError::UnsupportedVersion(raw.version));
    }
    if raw.objects.len() > MAX_OBJECTS {
        return Err(FormatError::TooLarge);
    }
    if raw.size.w == 0 || raw.size.h == 0 {
        return Err(FormatError::InvalidImage(
            "manifest declares a zero-sized canvas".to_string(),
        ));
    }

    let png = BASE64
        .decode(raw.base_png.as_bytes())
        .map_err(|e| FormatError::Base64(e.to_string()))?;
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| FormatError::InvalidImage(e.to_string()))?;
    let base = img.to_rgba8();

    let (w, h) = base.dimensions();
    if w == 0 || h == 0 {
        return Err(FormatError::InvalidImage(
            "base image has a zero dimension".to_string(),
        ));
    }
    if w > MAX_DIM || h > MAX_DIM {
        return Err(FormatError::TooLarge);
    }
    // The manifest size must agree with the actual base pixels — a mismatch
    // would desync every object coordinate from the image (hostile file rule).
    if w != raw.size.w || h != raw.size.h {
        return Err(FormatError::InvalidImage(format!(
            "manifest size {}x{} does not match base image {w}x{h}",
            raw.size.w, raw.size.h
        )));
    }

    // Hostile-input pass: clamp shadow ranges, cap brush point counts.
    let mut objects = raw.objects;
    for obj in &mut objects {
        obj.sanitize();
    }

    Ok(Document {
        size: raw.size,
        base: Arc::new(base),
        objects,
        next_id: raw.next_id,
    })
}

/// Build the manifest value for `doc`.
///
/// The `let Document { .. } = doc` line below has **no rest pattern** on purpose: a new field on
/// [`Document`] is a compile error here. Same for every arm of [`object_json`].
fn manifest_json(doc: &Document) -> Result<Value, FormatError> {
    let Document {
        size,
        base,
        objects,
        next_id,
    } = doc;

    if objects.len() > MAX_OBJECTS {
        return Err(FormatError::TooLarge);
    }

    let png = encode_png_base(base)?;
    let encoded: Vec<Value> = objects.iter().map(object_json).collect();

    Ok(json!({
        "version": FORMAT_VERSION,
        "size": { "w": size.w, "h": size.h },
        "next_id": *next_id,
        "objects": encoded,
        "base_png": BASE64.encode(png),
    }))
}

/// PNG-encode the base surface (lossless: round-trips pixel-exactly).
fn encode_png_base(base: &Surface) -> Result<Vec<u8>, FormatError> {
    let (w, h) = base.dimensions();
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(FormatError::TooLarge);
    }
    let mut cur = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(base.clone())
        .write_to(&mut cur, image::ImageFormat::Png)
        .map_err(|e| FormatError::Io(format!("base png encode failed: {e}")))?;
    Ok(cur.into_inner())
}

/// One object → manifest JSON. Every variant names **every** field (compile-time guard).
fn object_json(obj: &AnnotObject) -> Value {
    match obj {
        AnnotObject::Arrow {
            id,
            from,
            to,
            color,
            width,
            shadow,
        } => json!({
            "kind": "Arrow",
            "id": id,
            "from": from,
            "to": to,
            "color": color,
            "width": width,
            "shadow": shadow,
        }),
        AnnotObject::Shape {
            id,
            shape,
            rect,
            color,
            width,
            shadow,
        } => json!({
            "kind": "Shape",
            "id": id,
            "shape": shape,
            "rect": rect,
            "color": color,
            "width": width,
            "shadow": shadow,
        }),
        AnnotObject::Text {
            id,
            pos,
            text,
            color,
            size,
            bold,
            italic,
            family,
            shadow,
        } => json!({
            "kind": "Text",
            "id": id,
            "pos": pos,
            "text": text,
            "color": color,
            "size": size,
            "bold": bold,
            "italic": italic,
            "family": family,
            "shadow": shadow,
        }),
        AnnotObject::Blur {
            id,
            rect,
            mode,
            radius,
        } => json!({
            "kind": "Blur",
            "id": id,
            "rect": rect,
            "mode": mode,
            "radius": radius,
        }),
        AnnotObject::Step {
            id,
            pos,
            number,
            color,
            radius,
            shadow,
        } => json!({
            "kind": "Step",
            "id": id,
            "pos": pos,
            "number": number,
            "color": color,
            "radius": radius,
            "shadow": shadow,
        }),
        AnnotObject::Highlighter { id, rect, color } => json!({
            "kind": "Highlighter",
            "id": id,
            "rect": rect,
            "color": color,
        }),
        AnnotObject::Spotlight { id, rect, dim } => json!({
            "kind": "Spotlight",
            "id": id,
            "rect": rect,
            "dim": dim,
        }),
        AnnotObject::Callout {
            id,
            rect,
            text,
            color,
            size,
            family,
            shadow,
        } => json!({
            "kind": "Callout",
            "id": id,
            "rect": rect,
            "text": text,
            "color": color,
            "size": size,
            "family": family,
            "shadow": shadow,
        }),
        AnnotObject::Magnify { id, rect, zoom } => json!({
            "kind": "Magnify",
            "id": id,
            "rect": rect,
            "zoom": zoom,
        }),
        AnnotObject::BrushStroke {
            id,
            points,
            color,
            width,
        } => json!({
            "kind": "BrushStroke",
            "id": id,
            "points": points,
            "color": color,
            "width": width,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagcraft_color::Rgba;
    use snagcraft_doc::{BlurMode, MAX_BRUSH_POINTS, Shadow, ShapeKind};
    use snagcraft_geom::{Point, Rect};

    fn base_surface(w: u32, h: u32) -> Surface {
        Surface::from_fn(w, h, |x, y| {
            image::Rgba([
                (x * 5 % 251) as u8,
                (y * 3 % 241) as u8,
                ((x * 31 + y * 17) % 256) as u8,
                255,
            ])
        })
    }

    /// One document carrying every object kind, inserted through `add` so ids are 1..=10.
    fn doc_all_kinds() -> Document {
        let mut d = Document::new(base_surface(33, 21));
        let glow = Shadow {
            enabled: true,
            color: Rgba::new(0, 0, 0, 180),
            blur: 7,
            offset_x: 2.5,
            offset_y: 5.0,
        };
        d.add(AnnotObject::Arrow {
            id: 0,
            from: Point::new(1.0, 2.0),
            to: Point::new(30.5, 18.25),
            color: Rgba::new(255, 0, 0, 255),
            width: 3.5,
            shadow: glow,
        });
        d.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Star,
            rect: Rect::new(2.0, 3.0, 10.0, 12.0),
            color: Rgba::new(0, 128, 255, 200),
            width: 2.0,
            shadow: glow,
        });
        d.add(AnnotObject::Text {
            id: 0,
            pos: Point::new(4.0, 5.0),
            text: "hello — snag".to_string(),
            color: Rgba::BLACK,
            size: 14.0,
            bold: true,
            italic: true,
            family: "Inter".to_string(),
            shadow: Shadow::default(),
        });
        d.add(AnnotObject::Blur {
            id: 0,
            rect: Rect::new(6.0, 7.0, 8.0, 9.0),
            mode: BlurMode::Pixelate,
            radius: 10,
        });
        d.add(AnnotObject::Step {
            id: 0,
            pos: Point::new(12.0, 13.0),
            number: 7,
            color: Rgba::new(10, 20, 30, 255),
            radius: 11.5,
            shadow: glow,
        });
        d.add(AnnotObject::Highlighter {
            id: 0,
            rect: Rect::new(1.5, 2.5, 20.0, 6.0),
            color: Rgba::new(255, 255, 0, 90),
        });
        d.add(AnnotObject::Spotlight {
            id: 0,
            rect: Rect::new(3.0, 3.0, 17.0, 9.0),
            dim: 0.6,
        });
        d.add(AnnotObject::Callout {
            id: 0,
            rect: Rect::new(0.0, 0.0, 30.0, 20.0),
            text: "note".to_string(),
            color: Rgba::new(40, 40, 40, 255),
            size: 13.0,
            family: "Inter".to_string(),
            shadow: glow,
        });
        d.add(AnnotObject::Magnify {
            id: 0,
            rect: Rect::new(5.0, 6.0, 11.0, 9.0),
            zoom: 2.5,
        });
        d.add(AnnotObject::BrushStroke {
            id: 0,
            points: vec![
                Point::new(1.0, 1.0),
                Point::new(7.5, 4.25),
                Point::new(12.0, 15.0),
            ],
            color: Rgba::new(0, 200, 10, 255),
            width: 4.5,
        });
        d
    }

    fn manifest_of(doc: &Document) -> Value {
        serde_json::from_slice(&save(doc).unwrap()).unwrap()
    }

    #[test]
    fn round_trip_all_ten_kinds() {
        let doc = doc_all_kinds();
        let back = load(&save(&doc).unwrap()).unwrap();

        assert_eq!(back.size, doc.size);
        assert_eq!(back.next_id, doc.next_id);
        assert_eq!(back.next_id, 11);
        assert_eq!(back.objects.len(), 10);
        assert_eq!(back.objects, doc.objects);
        assert_eq!(back.base.dimensions(), doc.base.dimensions());
        assert_eq!(back.base.as_raw(), doc.base.as_raw());
    }

    #[test]
    fn manifest_is_versioned_json_with_base64_png() {
        let v = manifest_of(&doc_all_kinds());
        assert_eq!(v["version"], json!(FORMAT_VERSION));
        assert_eq!(v["size"], json!({"w": 33, "h": 21}));
        assert_eq!(v["next_id"], json!(11));
        let objects = v["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 10);
        let kinds: Vec<&str> = objects
            .iter()
            .map(|o| o["kind"].as_str().unwrap_or("<missing>"))
            .collect();
        assert_eq!(
            kinds,
            [
                "Arrow",
                "Shape",
                "Text",
                "Blur",
                "Step",
                "Highlighter",
                "Spotlight",
                "Callout",
                "Magnify",
                "BrushStroke"
            ]
        );
        let b64 = v["base_png"].as_str().unwrap();
        let png = BASE64.decode(b64).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn object_fields_all_survive() {
        let v = manifest_of(&doc_all_kinds());
        let objects = v["objects"].as_array().unwrap();
        // Spot the fields that a lossy mapping would drop.
        assert_eq!(objects[0]["width"], json!(3.5));
        // Shadow is written for every kind that carries it, field by field.
        assert_eq!(objects[0]["shadow"]["enabled"], json!(true));
        assert_eq!(objects[0]["shadow"]["blur"], json!(7));
        assert_eq!(objects[0]["shadow"]["offset_x"], json!(2.5f32));
        assert_eq!(objects[0]["shadow"]["offset_y"], json!(5.0f32));
        assert_eq!(objects[0]["shadow"]["color"]["a"], json!(180));
        // A disabled shadow is still written explicitly (not omitted).
        assert_eq!(objects[2]["shadow"]["enabled"], json!(false));
        assert_eq!(objects[4]["shadow"]["enabled"], json!(true));
        assert_eq!(objects[7]["shadow"], objects[0]["shadow"]);
        assert_eq!(objects[1]["shape"], json!("Star"));
        assert_eq!(objects[2]["bold"], json!(true));
        assert_eq!(objects[2]["italic"], json!(true));
        assert_eq!(objects[3]["mode"], json!("Pixelate"));
        assert_eq!(objects[3]["radius"], json!(10));
        assert_eq!(objects[6]["dim"], json!(0.6f32));
        assert_eq!(objects[7]["text"], json!("note"));
        assert_eq!(objects[8]["zoom"], json!(2.5f32));
        assert_eq!(
            objects[9]["points"],
            json!([{"x": 1.0, "y": 1.0}, {"x": 7.5, "y": 4.25}, {"x": 12.0, "y": 15.0}])
        );
        assert_eq!(objects[9]["width"], json!(4.5f32));
    }

    #[test]
    fn triangle_and_line_shape_kinds_round_trip() {
        let mut doc = Document::new(base_surface(12, 12));
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Triangle,
            rect: Rect::new(1.0, 1.0, 5.0, 5.0),
            color: Rgba::BLACK,
            width: 1.5,
            shadow: Shadow::default(),
        });
        doc.add(AnnotObject::Shape {
            id: 0,
            shape: ShapeKind::Line,
            rect: Rect::new(2.0, 2.0, 6.0, 6.0),
            color: Rgba::BLACK,
            width: 1.5,
            shadow: Shadow::default(),
        });
        let v = manifest_of(&doc);
        assert_eq!(v["objects"][0]["shape"], json!("Triangle"));
        assert_eq!(v["objects"][1]["shape"], json!("Line"));
        let back = load(&save(&doc).unwrap()).unwrap();
        assert_eq!(back.objects, doc.objects);
    }

    /// A v1 file written before shadows existed must still load, with `Shadow::default()`.
    #[test]
    fn old_file_without_shadow_fields_loads_defaults() {
        let doc = doc_all_kinds();
        let mut v = manifest_of(&doc);
        let mut stripped = 0;
        for obj in v["objects"].as_array_mut().unwrap() {
            if let Some(map) = obj.as_object_mut()
                && map.remove("shadow").is_some()
            {
                stripped += 1;
            }
        }
        assert_eq!(
            stripped, 5,
            "Arrow/Shape/Text/Step/Callout all carry a shadow field"
        );

        let back = load(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(back.objects.len(), 10);
        let mut checked = 0;
        for o in &back.objects {
            match o {
                AnnotObject::Arrow { shadow, .. }
                | AnnotObject::Shape { shadow, .. }
                | AnnotObject::Text { shadow, .. }
                | AnnotObject::Step { shadow, .. }
                | AnnotObject::Callout { shadow, .. } => {
                    assert_eq!(*shadow, Shadow::default());
                    checked += 1;
                }
                _ => {}
            }
        }
        assert_eq!(checked, 5);
        // Everything else still round-trips.
        assert_eq!(back.next_id, 11);
    }

    #[test]
    fn load_caps_hostile_shadow_and_brush_points() {
        let mut doc = Document::new(base_surface(9, 9));
        // `add` sanitizes, so splice the hostile values in afterwards.
        doc.objects.push(AnnotObject::Shape {
            id: 1,
            shape: ShapeKind::Rect,
            rect: Rect::new(1.0, 1.0, 2.0, 2.0),
            color: Rgba::BLACK,
            width: 1.0,
            shadow: Shadow {
                enabled: true,
                color: Rgba::BLACK,
                blur: 9_999,
                offset_x: 1_000.0,
                offset_y: -1_000.0,
            },
        });
        doc.objects.push(AnnotObject::BrushStroke {
            id: 2,
            points: vec![Point::new(1.0, 1.0); MAX_BRUSH_POINTS + 10],
            color: Rgba::BLACK,
            width: 1.0,
        });
        doc.next_id = 3;

        let back = load(&save(&doc).unwrap()).unwrap();
        match &back.objects[0] {
            AnnotObject::Shape { shadow, .. } => {
                assert_eq!(shadow.blur, snagcraft_doc::SHADOW_BLUR_MAX);
                assert_eq!(shadow.offset_x, snagcraft_doc::SHADOW_OFFSET_MAX);
                assert_eq!(shadow.offset_y, -snagcraft_doc::SHADOW_OFFSET_MAX);
            }
            _ => panic!("wrong kind"),
        }
        match &back.objects[1] {
            AnnotObject::BrushStroke { points, .. } => {
                assert_eq!(points.len(), MAX_BRUSH_POINTS)
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn rejects_future_version() {
        let mut v = manifest_of(&doc_all_kinds());
        v["version"] = json!(2);
        let err = load(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert!(matches!(err, FormatError::UnsupportedVersion(2)));
    }

    #[test]
    fn rejects_garbage_and_empty_input() {
        assert!(matches!(load(b""), Err(FormatError::Json(_))));
        assert!(matches!(load(b"hello world"), Err(FormatError::Json(_))));
        assert!(matches!(load(b"{ not json"), Err(FormatError::Json(_))));
        // Valid JSON, wrong shape.
        assert!(matches!(
            load(br#"{"version":1}"#),
            Err(FormatError::Json(_))
        ));
    }

    #[test]
    fn rejects_bad_base64() {
        let mut v = manifest_of(&doc_all_kinds());
        v["base_png"] = json!("!!! not base64 !!!");
        assert!(matches!(
            load(&serde_json::to_vec(&v).unwrap()),
            Err(FormatError::Base64(_))
        ));
    }

    #[test]
    fn rejects_non_png_base_image() {
        let mut v = manifest_of(&doc_all_kinds());
        v["base_png"] = json!(BASE64.encode(b"definitely not a png"));
        assert!(matches!(
            load(&serde_json::to_vec(&v).unwrap()),
            Err(FormatError::InvalidImage(_))
        ));
    }

    #[test]
    fn rejects_zero_sized_canvas() {
        let mut v = manifest_of(&doc_all_kinds());
        v["size"] = json!({"w": 0, "h": 0});
        assert!(matches!(
            load(&serde_json::to_vec(&v).unwrap()),
            Err(FormatError::InvalidImage(_))
        ));
    }

    #[test]
    fn empty_document_round_trips_and_keeps_next_id() {
        let mut doc = Document::new(base_surface(5, 4));
        doc.next_id = 4242;
        let back = load(&save(&doc).unwrap()).unwrap();
        assert!(back.objects.is_empty());
        assert_eq!(back.next_id, 4242);
        assert_eq!(back.base.as_raw(), doc.base.as_raw());
    }

    #[test]
    fn save_rejects_oversized_canvas() {
        // `Document::white` clamps, so build the illegal surface directly.
        let base = Surface::from_pixel(MAX_DIM + 1, 4, image::Rgba([0, 0, 0, 255]));
        let doc = Document {
            size: Size::new(MAX_DIM + 1, 4),
            base: std::sync::Arc::new(base),
            objects: Vec::new(),
            next_id: 1,
        };
        assert!(matches!(save(&doc), Err(FormatError::TooLarge)));
    }

    #[test]
    fn load_rejects_size_mismatch_with_base() {
        let doc = Document::white(Size::new(16, 16));
        let bytes = save(&doc).unwrap();
        // Tamper: manifest claims 32x16 while the base PNG stays 16x16.
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["size"]["w"] = serde_json::json!(32);
        let tampered = serde_json::to_vec(&v).unwrap();
        assert!(matches!(load(&tampered), Err(FormatError::InvalidImage(_))));
    }
}
