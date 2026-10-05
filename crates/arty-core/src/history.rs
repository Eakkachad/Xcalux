//! Undo/redo built on copy-on-write tiles.
//!
//! A pixel edit stores the *previous* `Arc` of every tile it touched, so
//! recording costs a pointer clone per tile and memory grows only by the
//! tiles that actually changed. Undo swaps the stored tiles back in and
//! keeps the swapped-out ones as the redo entry.

use std::collections::VecDeque;

use ahash::AHashSet;

use crate::document::{DirtyRegion, Document, StructureSnapshot};
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

    /// Put the pre-stroke tile back for every recorded tile `select` picks, marking it dirty.
    /// Recording continues, so repainting still yields one Edit::Pixels holding the original tiles.
    pub fn restore(&self, grid: &mut TileGrid, dirty: &mut DirtyRegion, mut select: impl FnMut(TileCoord) -> bool) {
        for (c, old) in &self.tiles {
            if select(*c) {
                grid.replace(*c, old.clone());
                dirty.mark(*c);
            }
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
    /// Layer whose props entry on top of `undo` belongs to the gesture that
    /// is still running, and may absorb further coalescing pushes.
    props_open: Option<LayerId>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(200)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self { undo: VecDeque::new(), redo: Vec::new(), limit: limit.max(1), props_open: None }
    }

    /// Record an edit that has already been applied to the document.
    pub fn push(&mut self, edit: Edit) {
        self.props_open = None;
        self.redo.clear();
        if self.undo.len() == self.limit {
            self.undo.pop_front();
        }
        self.undo.push_back(edit);
    }

    /// Record a property change. `coalesce` marks it as part of a continuous
    /// gesture (e.g. dragging an opacity slider): the first such push opens
    /// an entry, and later coalescing pushes for the same layer merge into
    /// it until [`History::end_props_gesture`] or any other history operation
    /// closes it. Entries recorded before the gesture are never merged into.
    pub fn push_props(&mut self, layer: LayerId, before: LayerProps, coalesce: bool) {
        if coalesce && self.props_open == Some(layer) {
            return; // keep the gesture's oldest "before" state
        }
        self.push(Edit::Props { layer, props: before });
        self.props_open = coalesce.then_some(layer);
    }

    /// Close the open props gesture so the next coalescing
    /// [`History::push_props`] starts a new undo step. Call at both edges of
    /// a gesture: when it starts (so it cannot merge into an earlier one)
    /// and when it ends.
    pub fn end_props_gesture(&mut self) {
        self.props_open = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo one step. Returns the layer it affected, if any.
    pub fn undo(&mut self, doc: &mut Document) -> Option<LayerId> {
        self.props_open = None;
        let edit = self.undo.pop_back()?;
        let layer = edit.touched_layer();
        self.redo.push(edit.apply(doc));
        layer
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<LayerId> {
        self.props_open = None;
        let edit = self.redo.pop()?;
        let layer = edit.touched_layer();
        self.undo.push_back(edit.apply(doc));
        layer
    }

    pub fn clear(&mut self) {
        self.props_open = None;
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
    use crate::blend::BlendMode;

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
    fn restore_puts_back_pre_stroke_tiles_and_keeps_one_edit() {
        let mut doc = Document::new(256, 64, 72);
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        // An existing tile (0,0) from an earlier stroke.
        h.push(stroke(&mut doc, &mut rec, 0, 100).unwrap());
        let id = doc.active();
        let (a, b) = (TileCoord::new(0, 0), TileCoord::new(1, 0));
        let old_a = doc.active_layer().raster().unwrap().get_ref(a).unwrap().clone();

        // A stroke writes the existing tile and creates a new one.
        rec.begin(id);
        let (grid, _) = doc.paint_target(id).unwrap();
        for (c, v) in [(a, 7), (b, 9)] {
            rec.before_write(grid, c);
            grid.get_mut_or_create(c)[0][1] = [v; 4];
        }
        let (grid, dirty) = doc.paint_target(id).unwrap();
        let mut drained = Vec::new();
        dirty.drain_into(&mut drained);
        rec.restore(grid, dirty, |_| true);
        let raster = doc.active_layer().raster().unwrap();
        assert!(std::sync::Arc::ptr_eq(raster.get_ref(a).unwrap(), &old_a), "existing tile is the pre-stroke Arc");
        assert!(raster.get_ref(b).is_none(), "a tile the stroke created is removed");
        let (_, dirty) = doc.paint_target(id).unwrap();
        assert!(!dirty.is_clean(), "restored tiles are marked dirty");

        // Repaint (as a replay would); the restored tiles are not re-recorded.
        let (grid, _) = doc.paint_target(id).unwrap();
        for (c, v) in [(a, 11), (b, 12)] {
            rec.before_write(grid, c);
            grid.get_mut_or_create(c)[0][1] = [v; 4];
        }
        assert_eq!(pixel(&doc, 1)[0], 11);
        let edit = rec.finish().unwrap();
        let Edit::Pixels { ref tiles, .. } = edit else { panic!("pixel edit") };
        assert_eq!(tiles.len(), 2, "one edit holding each tile once");
        h.push(edit);
        h.undo(&mut doc);
        let raster = doc.active_layer().raster().unwrap();
        assert!(std::sync::Arc::ptr_eq(raster.get_ref(a).unwrap(), &old_a));
        assert!(raster.get_ref(b).is_none());
        assert_eq!(pixel(&doc, 0)[0], 100);
        assert_eq!(pixel(&doc, 1)[0], 0);
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
        let added = doc.add_raster_layer().unwrap();
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

    fn set_opacity(doc: &mut Document, h: &mut History, op: f32, coalesce: bool) {
        let id = doc.active();
        let mut p = doc.layer(id).unwrap().props.clone();
        p.opacity = op;
        let before = doc.set_props(id, p).unwrap();
        h.push_props(id, before, coalesce);
    }

    #[test]
    fn props_gesture_never_merges_into_earlier_entries() {
        let mut doc = Document::new(64, 64, 72);
        let id = doc.active();
        let mut h = History::default();
        // A discrete blend change, then two separate drags on the same layer.
        let mut p = doc.layer(id).unwrap().props.clone();
        p.blend = BlendMode::Multiply;
        let before = doc.set_props(id, p).unwrap();
        h.push_props(id, before, false);
        for drag in [&[0.5, 0.4][..], &[0.2]] {
            h.end_props_gesture(); // drag starts
            for &op in drag {
                set_opacity(&mut doc, &mut h, op, true);
            }
            h.end_props_gesture(); // drag ends
        }
        assert_eq!(h.undo_len(), 3);

        h.undo(&mut doc);
        assert_eq!(doc.layer(id).unwrap().props.opacity, 0.4, "second drag undoes alone");
        h.undo(&mut doc);
        let props = &doc.layer(id).unwrap().props;
        assert_eq!((props.opacity, props.blend), (1.0, BlendMode::Multiply), "blend change is its own step");
        h.undo(&mut doc);
        assert_eq!(doc.layer(id).unwrap().props.blend, BlendMode::Normal);
    }

    #[test]
    fn props_gesture_closes_on_other_history_operations() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::default();
        set_opacity(&mut doc, &mut h, 0.5, true);
        h.undo(&mut doc);
        h.redo(&mut doc);
        // The redone entry is not this gesture's own, so it is not reused.
        set_opacity(&mut doc, &mut h, 0.3, true);
        assert_eq!(h.undo_len(), 2);
        h.push(Edit::Structure(Box::new(doc.snapshot_structure())));
        set_opacity(&mut doc, &mut h, 0.1, true);
        assert_eq!(h.undo_len(), 4);
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
