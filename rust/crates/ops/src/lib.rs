#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! L2 history: bounded undo/redo over cheap Document snapshots
//! (Arc-shared base image; only the object vec is cloned).

use snagcraft_doc::Document;

pub const MAX_UNDO: usize = 100;

#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Document>,
    redo: Vec<Document>,
}

impl History {
    /// Record the pre-mutation state; clears the redo branch.
    pub fn record(&mut self, before: &Document) {
        self.undo.push(before.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &Document) -> Option<Document> {
        let prev = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(prev)
    }

    pub fn redo(&mut self, current: &Document) -> Option<Document> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagcraft_color::Rgba;
    use snagcraft_doc::AnnotObject;
    use snagcraft_geom::{Rect, Size};
    use std::sync::Arc;

    fn rect_obj() -> AnnotObject {
        AnnotObject::Shape {
            id: 0,
            shape: snagcraft_doc::ShapeKind::Rect,
            rect: Rect::new(1.0, 1.0, 5.0, 5.0),
            color: Rgba::BLACK,
            width: 1.0,
        }
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut doc = Document::white(Size::new(16, 16));
        let mut h = History::default();
        h.record(&doc);
        doc.add(rect_obj());
        assert_eq!(doc.objects.len(), 1);
        doc = h.undo(&doc).unwrap();
        assert!(doc.objects.is_empty());
        assert!(h.can_redo());
        doc = h.redo(&doc).unwrap();
        assert_eq!(doc.objects.len(), 1);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut doc = Document::white(Size::new(16, 16));
        let mut h = History::default();
        h.record(&doc);
        doc.add(rect_obj());
        doc = h.undo(&doc).unwrap();
        h.record(&doc);
        doc.add(rect_obj());
        assert!(!h.can_redo());
    }

    #[test]
    fn bounded_and_clear() {
        let mut doc = Document::white(Size::new(8, 8));
        let mut h = History::default();
        for _ in 0..(MAX_UNDO + 25) {
            h.record(&doc);
            doc.add(rect_obj());
        }
        let mut count = 0;
        while let Some(prev) = h.undo(&doc) {
            doc = prev;
            count += 1;
        }
        assert_eq!(count, MAX_UNDO);
        h.clear();
        assert!(!h.can_undo() && !h.can_redo());
    }

    #[test]
    fn undo_restores_arc_shared_base() {
        let mut doc = Document::white(Size::new(8, 8));
        let base = doc.base.clone();
        let mut h = History::default();
        h.record(&doc);
        doc.base = Arc::new(snagcraft_raster::filled(Size::new(8, 8), Rgba::BLACK));
        doc = h.undo(&doc).unwrap();
        assert!(Arc::ptr_eq(&base, &doc.base));
    }
}
