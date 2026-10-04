//! The document: page size, layer tree, active layer and dirty tracking.

use ahash::{AHashMap, AHashSet};

use crate::blend::{BlendMode, blend_tile, blend_tile_atop};
use crate::fix15::ONE_U16;
use crate::grid::TileGrid;
use crate::layer::{Layer, LayerContent, LayerId, LayerProps};
use crate::tile::{TILE_SIZE, TileCoord};

/// Tiles whose composite is out of date.
#[derive(Default)]
pub struct DirtyRegion {
    all: bool,
    tiles: AHashSet<TileCoord>,
}

impl DirtyRegion {
    #[inline]
    pub fn mark(&mut self, c: TileCoord) {
        if !self.all {
            self.tiles.insert(c);
        }
    }

    pub fn mark_all(&mut self) {
        self.all = true;
        self.tiles.clear();
    }

    pub fn is_clean(&self) -> bool {
        !self.all && self.tiles.is_empty()
    }

    /// Moves the dirty set into `out` (reusing its capacity). Returns `true`
    /// when everything is dirty, in which case `out` is left empty.
    pub fn drain_into(&mut self, out: &mut Vec<TileCoord>) -> bool {
        out.clear();
        if std::mem::take(&mut self.all) {
            self.tiles.clear();
            return true;
        }
        out.extend(self.tiles.drain());
        false
    }
}

/// Captured layer tree, used for undoing structural edits. Cheap: pixel
/// tiles are `Arc`-shared, so only map entries are copied.
#[derive(Clone)]
pub struct StructureSnapshot {
    pub(crate) layers: AHashMap<LayerId, Layer>,
    pub(crate) root: Vec<LayerId>,
    pub(crate) active: LayerId,
    pub(crate) next_id: u32,
}

pub struct Document {
    width: u32,
    height: u32,
    dpi: u32,
    /// Opaque paper color under all layers (fix15 RGBA), or transparent.
    paper: Option<[u16; 4]>,
    pub(crate) layers: AHashMap<LayerId, Layer>,
    /// Top-level layers, bottom → top.
    pub(crate) root: Vec<LayerId>,
    pub(crate) next_id: u32,
    pub(crate) active: LayerId,
    pub(crate) dirty: DirtyRegion,
}

pub const PAPER_WHITE: [u16; 4] = [ONE_U16; 4];

impl Document {
    /// New document with white paper and one empty raster layer.
    pub fn new(width: u32, height: u32, dpi: u32) -> Self {
        let mut doc = Self {
            width: width.max(1),
            height: height.max(1),
            dpi: dpi.max(1),
            paper: Some(PAPER_WHITE),
            layers: AHashMap::default(),
            root: Vec::new(),
            next_id: 1,
            active: LayerId(0),
            dirty: DirtyRegion::default(),
        };
        let id = doc.alloc_id();
        doc.layers.insert(
            id,
            Layer { id, props: LayerProps::named("Layer 1"), content: LayerContent::Raster(TileGrid::new()) },
        );
        doc.root.push(id);
        doc.active = id;
        doc.dirty.mark_all();
        doc
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn dpi(&self) -> u32 {
        self.dpi
    }

    pub fn paper(&self) -> Option<[u16; 4]> {
        self.paper
    }

    pub fn set_paper(&mut self, paper: Option<[u16; 4]>) {
        self.paper = paper;
        self.dirty.mark_all();
    }

    /// Number of tile columns / rows covering the page.
    pub fn tiles_wide(&self) -> u32 {
        self.width.div_ceil(TILE_SIZE as u32)
    }

    pub fn tiles_high(&self) -> u32 {
        self.height.div_ceil(TILE_SIZE as u32)
    }

    #[inline]
    pub fn contains_tile(&self, c: TileCoord) -> bool {
        c.x >= 0 && c.y >= 0 && (c.x as u32) < self.tiles_wide() && (c.y as u32) < self.tiles_high()
    }

    // ----- access ---------------------------------------------------------

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(&id)
    }

    pub fn active(&self) -> LayerId {
        self.active
    }

    pub fn active_layer(&self) -> &Layer {
        &self.layers[&self.active]
    }

    pub fn set_active(&mut self, id: LayerId) {
        if self.layers.contains_key(&id) {
            self.active = id;
        }
    }

    pub fn root(&self) -> &[LayerId] {
        &self.root
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Approximate pixel memory of all raster layers (shared tiles counted
    /// once per layer).
    pub fn pixel_bytes(&self) -> usize {
        self.layers.values().filter_map(|l| l.raster()).map(TileGrid::pixel_bytes).sum()
    }

    /// Mutable pixel grid of a raster layer together with the dirty set, so
    /// a brush can paint and invalidate in one borrow.
    pub fn paint_target(&mut self, id: LayerId) -> Option<(&mut TileGrid, &mut DirtyRegion)> {
        let layer = self.layers.get_mut(&id)?;
        let grid = layer.raster_mut()?;
        Some((grid, &mut self.dirty))
    }

    pub fn dirty_mut(&mut self) -> &mut DirtyRegion {
        &mut self.dirty
    }

    /// Replace a layer's settings. Returns the old settings when changed.
    pub fn set_props(&mut self, id: LayerId, props: LayerProps) -> Option<LayerProps> {
        let layer = self.layers.get_mut(&id)?;
        if layer.props == props {
            return None;
        }
        let clip_changed = layer.props.clip != props.clip;
        let affects_pixels = layer.props.visible != props.visible
            || layer.props.opacity != props.opacity
            || layer.props.blend != props.blend;
        let old = std::mem::replace(&mut layer.props, props);
        if clip_changed {
            self.mark_clip_change_dirty(id);
        } else if affects_pixels {
            self.mark_layer_dirty(id);
        }
        Some(old)
    }

    /// Invalidate every tile a layer (or folder subtree) has pixels in.
    /// Clipped layers only show inside their base's pixels, so this also
    /// covers changes to a clip group's base.
    pub fn mark_layer_dirty(&mut self, id: LayerId) {
        let Some(layer) = self.layers.get(&id) else { return };
        match &layer.content {
            LayerContent::Raster(grid) => {
                for c in grid.coords() {
                    self.dirty.mark(c);
                }
            }
            LayerContent::Folder { children, .. } => {
                for c in children.clone() {
                    self.mark_layer_dirty(c);
                }
            }
        }
    }

    /// Toggling `id`'s clip flag regroups the clip run around it: the clip
    /// layers directly above it switch base, and the bases on either side
    /// gain or lose a clip group (which changes how a pass-through folder
    /// base renders). Invalidate all of them.
    fn mark_clip_change_dirty(&mut self, id: LayerId) {
        let Some((parent, index)) = self.location(id) else { return };
        let siblings = match parent {
            None => &self.root,
            Some(p) => self.layers[&p].children().expect("folder"),
        };
        let is_clip = |s: &LayerId| self.layers[s].props.clip;
        // The bottom sibling is always a base, even when flagged as clip.
        let base = siblings[..index].iter().rposition(|s| !is_clip(s)).or((index > 0).then_some(0));
        let mut affected: Vec<LayerId> = base.map(|b| siblings[b]).into_iter().collect();
        affected.push(id);
        affected.extend(siblings[index + 1..].iter().take_while(|s| is_clip(s)));
        for l in affected {
            self.mark_layer_dirty(l);
        }
    }

    pub fn set_folder_expanded(&mut self, id: LayerId, open: bool) {
        if let Some(Layer { content: LayerContent::Folder { expanded, .. }, .. }) = self.layers.get_mut(&id) {
            *expanded = open;
        }
    }

    /// Layers in panel order (top → bottom) with nesting depth, skipping the
    /// children of collapsed folders.
    pub fn panel_rows(&self, out: &mut Vec<(LayerId, usize)>) {
        out.clear();
        self.push_rows(&self.root, 0, out);
    }

    fn push_rows(&self, ids: &[LayerId], depth: usize, out: &mut Vec<(LayerId, usize)>) {
        for &id in ids.iter().rev() {
            out.push((id, depth));
            if let Some(Layer { content: LayerContent::Folder { children, expanded: true }, .. }) =
                self.layers.get(&id)
            {
                self.push_rows(children, depth + 1, out);
            }
        }
    }

    /// Maximum folder nesting depth (top level = 1).
    pub fn tree_depth(&self) -> usize {
        fn depth(doc: &Document, ids: &[LayerId]) -> usize {
            ids.iter()
                .map(|id| match &doc.layers[id].content {
                    LayerContent::Folder { children, .. } => 1 + depth(doc, children),
                    LayerContent::Raster(_) => 1,
                })
                .max()
                .unwrap_or(0)
        }
        depth(self, &self.root)
    }

    // ----- tree structure -------------------------------------------------

    fn alloc_id(&mut self) -> LayerId {
        let id = LayerId(self.next_id);
        self.next_id += 1;
        id
    }

    /// `(parent, index)` of a layer; `parent == None` means top level.
    pub fn location(&self, id: LayerId) -> Option<(Option<LayerId>, usize)> {
        if let Some(i) = self.root.iter().position(|&x| x == id) {
            return Some((None, i));
        }
        self.layers.values().find_map(|l| {
            let i = l.children()?.iter().position(|&x| x == id)?;
            Some((Some(l.id), i))
        })
    }

    fn siblings_mut(&mut self, parent: Option<LayerId>) -> &mut Vec<LayerId> {
        match parent {
            None => &mut self.root,
            Some(p) => match &mut self.layers.get_mut(&p).expect("parent exists").content {
                LayerContent::Folder { children, .. } => children,
                LayerContent::Raster(_) => unreachable!("parent is a folder"),
            },
        }
    }

    pub fn snapshot_structure(&self) -> StructureSnapshot {
        StructureSnapshot {
            layers: self.layers.clone(),
            root: self.root.clone(),
            active: self.active,
            next_id: self.next_id,
        }
    }

    /// Swap the layer tree with `snap`, returning the previous tree.
    pub(crate) fn swap_structure(&mut self, snap: StructureSnapshot) -> StructureSnapshot {
        let old = StructureSnapshot {
            layers: std::mem::replace(&mut self.layers, snap.layers),
            root: std::mem::replace(&mut self.root, snap.root),
            active: std::mem::replace(&mut self.active, snap.active),
            next_id: std::mem::replace(&mut self.next_id, snap.next_id),
        };
        self.dirty.mark_all();
        old
    }

    fn insert_above_active(&mut self, layer: Layer) -> LayerId {
        let id = layer.id;
        let (parent, index) = self.location(self.active).unwrap_or((None, self.root.len().saturating_sub(1)));
        self.layers.insert(id, layer);
        let siblings = self.siblings_mut(parent);
        let at = (index + 1).min(siblings.len());
        siblings.insert(at, id);
        self.active = id;
        self.dirty.mark_all();
        id
    }

    pub fn add_raster_layer(&mut self) -> LayerId {
        let id = self.alloc_id();
        let name = format!("Layer {}", id.0);
        self.insert_above_active(Layer { id, props: LayerProps::named(name), content: LayerContent::Raster(TileGrid::new()) })
    }

    pub fn add_folder(&mut self) -> LayerId {
        let id = self.alloc_id();
        let mut props = LayerProps::named(format!("Folder {}", id.0));
        props.blend = BlendMode::PassThrough;
        self.insert_above_active(Layer {
            id,
            props,
            content: LayerContent::Folder { children: Vec::new(), expanded: true },
        })
    }

    fn count_rasters(&self) -> usize {
        self.layers.values().filter(|l| !l.is_folder()).count()
    }

    fn collect_subtree(&self, id: LayerId, out: &mut Vec<LayerId>) {
        out.push(id);
        if let Some(children) = self.layers.get(&id).and_then(|l| l.children()) {
            for &c in children {
                self.collect_subtree(c, out);
            }
        }
    }

    /// Delete a layer (and a folder's contents). Refuses to remove the last
    /// raster layer so there is always something to paint on.
    pub fn delete_layer(&mut self, id: LayerId) -> bool {
        let Some((parent, index)) = self.location(id) else { return false };
        let mut doomed = Vec::new();
        self.collect_subtree(id, &mut doomed);
        let rasters_removed = doomed.iter().filter(|d| !self.layers[*d].is_folder()).count();
        if self.count_rasters() == rasters_removed {
            return false;
        }
        self.siblings_mut(parent).remove(index);
        for d in &doomed {
            self.layers.remove(d);
        }
        if doomed.contains(&self.active) {
            let siblings = match parent {
                None => &self.root,
                Some(p) => self.layers[&p].children().unwrap_or(&[]),
            };
            self.active = if siblings.is_empty() {
                parent.unwrap_or_else(|| self.root[0])
            } else {
                siblings[index.saturating_sub(1).min(siblings.len() - 1)]
            };
        }
        self.dirty.mark_all();
        true
    }

    /// Move a layer one step up (`delta = 1`) or down (`-1`) among its siblings.
    pub fn shift_layer(&mut self, id: LayerId, delta: i32) -> bool {
        let Some((parent, index)) = self.location(id) else { return false };
        let siblings = self.siblings_mut(parent);
        let target = index as i64 + delta as i64;
        if target < 0 || target >= siblings.len() as i64 {
            return false;
        }
        siblings.swap(index, target as usize);
        self.dirty.mark_all();
        true
    }

    fn is_descendant(&self, folder: LayerId, maybe_child: LayerId) -> bool {
        let mut stack = vec![folder];
        while let Some(f) = stack.pop() {
            if let Some(children) = self.layers.get(&f).and_then(|l| l.children()) {
                for &c in children {
                    if c == maybe_child {
                        return true;
                    }
                    stack.push(c);
                }
            }
        }
        false
    }

    /// Move `id` to `index` within `parent` (`None` = top level).
    pub fn move_layer(&mut self, id: LayerId, parent: Option<LayerId>, index: usize) -> bool {
        if let Some(p) = parent
            && (p == id || self.is_descendant(id, p) || !self.layers.get(&p).is_some_and(|l| l.is_folder())) {
                return false;
            }
        let Some((old_parent, old_index)) = self.location(id) else { return false };
        self.siblings_mut(old_parent).remove(old_index);
        let siblings = self.siblings_mut(parent);
        let mut at = index;
        if old_parent == parent && old_index < index {
            at -= 1;
        }
        siblings.insert(at.min(siblings.len()), id);
        self.dirty.mark_all();
        true
    }

    /// Duplicate a layer (deep for folders). Pixels are shared until edited.
    pub fn duplicate_layer(&mut self, id: LayerId) -> Option<LayerId> {
        let (parent, index) = self.location(id)?;
        let copy = self.clone_subtree(id);
        if let Some(l) = self.layers.get_mut(&copy) {
            l.props.name.push_str(" copy");
        }
        let siblings = self.siblings_mut(parent);
        siblings.insert(index + 1, copy);
        self.active = copy;
        self.dirty.mark_all();
        Some(copy)
    }

    fn clone_subtree(&mut self, id: LayerId) -> LayerId {
        let src = self.layers[&id].clone();
        let new_id = self.alloc_id();
        let content = match src.content {
            LayerContent::Raster(g) => LayerContent::Raster(g),
            LayerContent::Folder { children, expanded } => LayerContent::Folder {
                children: children.into_iter().map(|c| self.clone_subtree(c)).collect(),
                expanded,
            },
        };
        self.layers.insert(new_id, Layer { id: new_id, props: src.props, content });
        new_id
    }

    /// Merge a raster layer into the raster layer directly below it. The
    /// merged layer keeps the lower layer's settings. A clipping layer merged
    /// into its base stays clipped to the base's pixels; a normal layer is
    /// not merged into a clipping layer, which would clip its content.
    pub fn merge_down(&mut self, id: LayerId) -> bool {
        let Some((parent, index)) = self.location(id) else { return false };
        if index == 0 {
            return false;
        }
        let below = match parent {
            None => self.root[index - 1],
            Some(p) => self.layers[&p].children().expect("folder")[index - 1],
        };
        let (Some(upper), Some(lower)) = (self.layers.get(&id), self.layers.get(&below)) else {
            return false;
        };
        // The bottom sibling is a base even when flagged as clip.
        let lower_is_base = !lower.props.clip || index == 1;
        if upper.is_folder() || lower.is_folder() || lower.props.locked || (!lower_is_base && !upper.props.clip) {
            return false;
        }
        // Upper clips to lower: apply it the way the clip group does.
        // (Two clip layers on the same base merge with plain Over.)
        let clip_to_lower = upper.props.clip && lower_is_base;
        let upper = upper.clone();
        let src = upper.raster().expect("raster");
        let opacity = if upper.props.visible { upper.props.opacity } else { 0.0 };
        let dst = self.layers.get_mut(&below).and_then(|l| l.raster_mut()).expect("raster");
        for (c, tile) in src.iter() {
            if clip_to_lower {
                // Nothing shows where the base has no pixels.
                if dst.get(c).is_some() {
                    blend_tile_atop(dst.get_mut_or_create(c), tile, opacity, upper.props.blend);
                }
            } else {
                blend_tile(dst.get_mut_or_create(c), tile, opacity, upper.props.blend);
            }
        }
        self.siblings_mut(parent).remove(index);
        self.layers.remove(&id);
        self.active = below;
        self.dirty.mark_all();
        true
    }

    /// Erase all pixels of a raster layer.
    pub fn clear_layer(&mut self, id: LayerId) -> bool {
        let Some((grid, dirty)) = self.paint_target(id) else { return false };
        for c in grid.coords() {
            dirty.mark(c);
        }
        grid.clear();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_document_has_one_active_layer() {
        let doc = Document::new(1000, 600, 350);
        assert_eq!(doc.layer_count(), 1);
        assert_eq!(doc.tiles_wide(), 16);
        assert_eq!(doc.tiles_high(), 10);
        assert!(doc.contains_tile(TileCoord::new(15, 9)));
        assert!(!doc.contains_tile(TileCoord::new(16, 0)));
    }

    #[test]
    fn add_inserts_above_active_and_panel_rows_are_top_down() {
        let mut doc = Document::new(64, 64, 72);
        let first = doc.active();
        let second = doc.add_raster_layer();
        let folder = doc.add_folder();
        let inner = {
            doc.set_active(folder);
            doc.add_raster_layer()
        };
        // `inner` went above the folder (siblings), not inside it.
        assert_eq!(doc.root(), &[first, second, folder, inner]);

        doc.move_layer(inner, Some(folder), 0);
        let mut rows = Vec::new();
        doc.panel_rows(&mut rows);
        assert_eq!(rows, vec![(folder, 0), (inner, 1), (second, 0), (first, 0)]);
        assert_eq!(doc.tree_depth(), 2);
    }

    #[test]
    fn folder_cannot_move_into_itself() {
        let mut doc = Document::new(64, 64, 72);
        let outer = doc.add_folder();
        let inner = doc.add_folder();
        assert!(doc.move_layer(inner, Some(outer), 0));
        assert!(!doc.move_layer(outer, Some(inner), 0));
        assert!(!doc.move_layer(outer, Some(outer), 0));
    }

    #[test]
    fn cannot_delete_last_raster() {
        let mut doc = Document::new(64, 64, 72);
        let only = doc.active();
        assert!(!doc.delete_layer(only));
        let other = doc.add_raster_layer();
        assert!(doc.delete_layer(other));
        assert_eq!(doc.active(), only);
    }

    #[test]
    fn duplicate_shares_pixels_until_written() {
        let mut doc = Document::new(64, 64, 72);
        let a = doc.active();
        doc.paint_target(a).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [9, 9, 9, 9];
        let b = doc.duplicate_layer(a).unwrap();
        let ga = doc.layer(a).unwrap().raster().unwrap();
        let gb = doc.layer(b).unwrap().raster().unwrap();
        assert!(std::sync::Arc::ptr_eq(
            ga.get_ref(TileCoord::new(0, 0)).unwrap(),
            gb.get_ref(TileCoord::new(0, 0)).unwrap()
        ));
    }

    fn set_clip(doc: &mut Document, id: LayerId) {
        let mut p = doc.layer(id).unwrap().props.clone();
        p.clip = true;
        doc.set_props(id, p);
    }

    fn put(doc: &mut Document, id: LayerId, c: TileCoord, x: usize, v: [u16; 4]) {
        doc.paint_target(id).unwrap().0.get_mut_or_create(c)[0][x] = v;
    }

    fn flatten(doc: &Document, c: TileCoord) -> Box<crate::tile::TilePixels> {
        let mut out = crate::tile::new_tile_box();
        doc.composite_tile(c, &mut out, &mut crate::composite::CompositeScratch::new());
        out
    }

    #[test]
    fn merge_down_keeps_clipping() {
        const O: u16 = ONE_U16;
        let (t0, t1) = (TileCoord::new(0, 0), TileCoord::new(1, 0));
        let mut doc = Document::new(128, 64, 72);
        let base = doc.active();
        put(&mut doc, base, t0, 0, [O, 0, 0, O]);
        // Two clip layers spilling past the base, also into a tile the base
        // does not have.
        let first = doc.add_raster_layer();
        let second = doc.add_raster_layer();
        for (id, v) in [(first, [0, 0, O, O]), (second, [0, O / 2, 0, O / 2])] {
            set_clip(&mut doc, id);
            for x in 0..2 {
                put(&mut doc, id, t0, x, v);
            }
            put(&mut doc, id, t1, 0, v);
        }
        let before = (flatten(&doc, t0), flatten(&doc, t1));

        assert!(doc.merge_down(second), "clip into clip on the same base");
        assert!(doc.merge_down(first), "clip into its base");
        assert_eq!(doc.root(), &[base]);
        let after = (flatten(&doc, t0), flatten(&doc, t1));
        assert_eq!(after.1, before.1);
        for (a, b) in after.0.as_flattened().iter().zip(before.0.as_flattened()) {
            for ch in 0..4 {
                assert!((a[ch] as i32 - b[ch] as i32).abs() <= 2, "{a:?} vs {b:?}");
            }
        }
        assert!(doc.layer(base).unwrap().raster().unwrap().get(t1).is_none());
    }

    #[test]
    fn merge_down_refuses_normal_layer_into_clip_layer() {
        let mut doc = Document::new(64, 64, 72);
        let _base = doc.active();
        let clip = doc.add_raster_layer();
        set_clip(&mut doc, clip);
        let top = doc.add_raster_layer();
        assert!(!doc.merge_down(top));
        assert!(doc.layer(top).is_some());
    }

    #[test]
    fn dirty_region_drains() {
        let mut d = DirtyRegion::default();
        d.mark(TileCoord::new(1, 1));
        let mut out = Vec::new();
        assert!(!d.drain_into(&mut out));
        assert_eq!(out, vec![TileCoord::new(1, 1)]);
        assert!(d.is_clean());
        d.mark_all();
        assert!(d.drain_into(&mut out));
        assert!(out.is_empty());
    }
}
