//! Undo/redo built on copy-on-write tiles.
//!
//! A pixel edit stores the *previous* `Arc` of every tile it touched, so
//! recording costs a pointer clone per tile and memory grows only by the
//! tiles that actually changed. Undo swaps the stored tiles back in and
//! keeps the swapped-out ones as the redo entry.

use std::collections::VecDeque;

use ahash::AHashSet;

use crate::document::{Document, StructureSnapshot};
use crate::grid::TileGrid;
use crate::layer::{LayerId, LayerProps};
use crate::tile::{TileCoord, TileRef};

pub enum Edit {
    Pixels { layer: LayerId, tiles: Vec<(TileCoord, Option<TileRef>)> },
    Props { layer: LayerId, props: LayerProps },
    Structure(Box<StructureSnapshot>),
}

impl Edit {
    /// Apply this edit to `doc`, returning the edit that reverses it.
    fn apply(self, doc: &mut Document) -> Edit {
        match self {
            Edit::Pixels { layer, mut tiles } => {
                if let Some((grid, dirty)) = doc.paint_target(layer) {
                    for (c, t) in &mut tiles {
                        *t = grid.replace(*c, t.take());
                        dirty.mark(*c);
                    }
                }
                Edit::Pixels { layer, tiles }
            }
            Edit::Props { layer, props } => {
                let old = doc.set_props(layer, props.clone()).unwrap_or(props);
                doc.mark_layer_dirty(layer);
                Edit::Props { layer, props: old }
            }
            Edit::Structure(snap) => Edit::Structure(Box::new(doc.swap_structure(*snap))),
        }
    }

    fn touched_layer(&self) -> Option<LayerId> {
        match self {
            Edit::Pixels { layer, .. } | Edit::Props { layer, .. } => Some(*layer),
            Edit::Structure(_) => None,
        }
    }
}

/// Collects the pre-stroke state of every tile a stroke writes.
///
/// Call [`PixelRecorder::before_write`] before mutating a tile; buffers are
/// reused across strokes, so recording allocates only when a stroke covers
/// more tiles than any previous one.
#[derive(Default)]
pub struct PixelRecorder {
    layer: Option<LayerId>,
    seen: AHashSet<TileCoord>,
    tiles: Vec<(TileCoord, Option<TileRef>)>,
}

impl PixelRecorder {
    pub fn begin(&mut self, layer: LayerId) {
        self.layer = Some(layer);
        self.seen.clear();
        self.tiles.clear();
    }

    pub fn is_recording(&self) -> bool {
        self.layer.is_some()
    }

    #[inline]
    pub fn before_write(&mut self, grid: &TileGrid, c: TileCoord) {
        if self.layer.is_some() && self.seen.insert(c) {
            self.tiles.push((c, grid.get_ref(c).cloned()));
        }
    }

    /// Finish recording. `None` when the stroke touched nothing.
    pub fn finish(&mut self) -> Option<Edit> {
        let layer = self.layer.take()?;
        if self.tiles.is_empty() {
            return None;
        }
        let cap = self.tiles.capacity();
        let tiles = std::mem::replace(&mut self.tiles, Vec::with_capacity(cap));
        Some(Edit::Pixels { layer, tiles })
    }
}

pub struct History {
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(200)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self { undo: VecDeque::new(), redo: Vec::new(), limit: limit.max(1) }
    }

    /// Record an edit that has already been applied to the document.
    pub fn push(&mut self, edit: Edit) {
        self.redo.clear();
        if self.undo.len() == self.limit {
            self.undo.pop_front();
        }
        self.undo.push_back(edit);
    }

    /// Record a property change, merging with the previous entry when it
    /// edits the same layer's props (e.g. dragging an opacity slider).
    pub fn push_props(&mut self, layer: LayerId, before: LayerProps, coalesce: bool) {
        if coalesce
            && let Some(Edit::Props { layer: l, .. }) = self.undo.back()
                && *l == layer && self.redo.is_empty() {
                    return; // keep the oldest "before" state
                }
        self.push(Edit::Props { layer, props: before });
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo one step. Returns the layer it affected, if any.
    pub fn undo(&mut self, doc: &mut Document) -> Option<LayerId> {
        let edit = self.undo.pop_back()?;
        let layer = edit.touched_layer();
        self.redo.push(edit.apply(doc));
        layer
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<LayerId> {
        let edit = self.redo.pop()?;
        let layer = edit.touched_layer();
        self.undo.push_back(edit.apply(doc));
        layer
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(doc: &mut Document, rec: &mut PixelRecorder, x: usize, v: u16) -> Option<Edit> {
        let id = doc.active();
        rec.begin(id);
        let c = TileCoord::new(0, 0);
        let (grid, _) = doc.paint_target(id).unwrap();
        rec.before_write(grid, c);
        grid.get_mut_or_create(c)[0][x] = [v; 4];
        rec.finish()
    }

    fn pixel(doc: &Document, x: usize) -> [u16; 4] {
        doc.active_layer().raster().unwrap().get(TileCoord::new(0, 0)).map(|t| t[0][x]).unwrap_or([0; 4])
    }

    #[test]
    fn undo_redo_pixels() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        h.push(stroke(&mut doc, &mut rec, 0, 100).unwrap());
        h.push(stroke(&mut doc, &mut rec, 1, 200).unwrap());
        assert_eq!((pixel(&doc, 0)[0], pixel(&doc, 1)[0]), (100, 200));

        h.undo(&mut doc);
        assert_eq!((pixel(&doc, 0)[0], pixel(&doc, 1)[0]), (100, 0));
        h.undo(&mut doc);
        assert_eq!(pixel(&doc, 0)[0], 0);
        assert!(doc.active_layer().raster().unwrap().is_empty(), "first stroke created the tile");

        h.redo(&mut doc);
        h.redo(&mut doc);
        assert_eq!((pixel(&doc, 0)[0], pixel(&doc, 1)[0]), (100, 200));
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        h.push(stroke(&mut doc, &mut rec, 0, 1).unwrap());
        h.undo(&mut doc);
        assert!(h.can_redo());
        h.push(stroke(&mut doc, &mut rec, 0, 2).unwrap());
        assert!(!h.can_redo());
    }

    #[test]
    fn structure_undo_restores_deleted_layer() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::default();
        let snap = doc.snapshot_structure();
        let added = doc.add_raster_layer();
        h.push(Edit::Structure(Box::new(snap)));
        assert!(doc.layer(added).is_some());
        h.undo(&mut doc);
        assert!(doc.layer(added).is_none());
        h.redo(&mut doc);
        assert!(doc.layer(added).is_some());
        assert_eq!(doc.active(), added);
    }

    #[test]
    fn props_coalesce_keeps_first_state() {
        let mut doc = Document::new(64, 64, 72);
        let id = doc.active();
        let mut h = History::default();
        for op in [0.8, 0.6, 0.4] {
            let mut p = doc.layer(id).unwrap().props.clone();
            p.opacity = op;
            let before = doc.set_props(id, p).unwrap();
            h.push_props(id, before, true);
        }
        assert_eq!(h.undo_len(), 1);
        h.undo(&mut doc);
        assert_eq!(doc.layer(id).unwrap().props.opacity, 1.0);
    }

    #[test]
    fn limit_drops_oldest() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::new(2);
        let mut rec = PixelRecorder::default();
        for v in 1..=3 {
            h.push(stroke(&mut doc, &mut rec, 0, v).unwrap());
        }
        assert_eq!(h.undo_len(), 2);
    }
}
