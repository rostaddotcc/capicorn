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

/// Undo depth. Live parity: the editor keeps the last 80 steps.
pub const MAX_HISTORY: usize = 80;

/// Old name, kept so existing call sites keep compiling.
#[deprecated(note = "renamed to MAX_HISTORY (80 steps, live parity)")]
pub const MAX_UNDO: usize = MAX_HISTORY;

#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Document>,
    redo: Vec<Document>,
}

impl History {
    /// Record the pre-mutation state; clears the redo branch.
    pub fn record(&mut self, before: &Document) {
        self.undo.push(before.clone());
        if self.undo.len() > MAX_HISTORY {
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

    /// Number of undo steps currently held (<= [`MAX_HISTORY`]).
    pub fn depth(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snagcraft_color::Rgba;
    use snagcraft_doc::{AnnotObject, Shadow};
    use snagcraft_geom::{Rect, Size};
    use std::sync::Arc;

    fn rect_obj() -> AnnotObject {
        AnnotObject::Shape {
            id: 0,
            shape: snagcraft_doc::ShapeKind::Rect,
            rect: Rect::new(1.0, 1.0, 5.0, 5.0),
            color: Rgba::BLACK,
            width: 1.0,
            shadow: Shadow::default(),
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
    fn bounded_to_eighty_and_clear() {
        assert_eq!(MAX_HISTORY, 80);
        let mut doc = Document::white(Size::new(8, 8));
        let mut h = History::default();
        for _ in 0..(MAX_HISTORY + 25) {
            h.record(&doc);
            doc.add(rect_obj());
        }
        // Bounded while recording, not only after unwinding.
        assert_eq!(h.depth(), MAX_HISTORY);

        let mut count = 0;
        while let Some(prev) = h.undo(&doc) {
            doc = prev;
            count += 1;
        }
        assert_eq!(count, MAX_HISTORY);
        h.clear();
        assert!(!h.can_undo() && !h.can_redo());
        assert_eq!(h.depth(), 0);
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
