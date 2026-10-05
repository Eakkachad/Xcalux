//! The document: page size, layer tree, active layer and dirty tracking.

use std::sync::Arc;

use ahash::{AHashMap, AHashSet};

use crate::blend::{BlendMode, blend_tile, blend_tile_atop};
use crate::fix15::ONE_U16;
use crate::frame::Frame;
use crate::grid::TileGrid;
use crate::layer::{Layer, LayerContent, LayerId, LayerProps};
use crate::page::PageSetup;
use crate::selection::Selection;
use crate::tile::{TILE_SIZE, TileCoord};

/// Deepest allowed folder nesting (top level = 1). `move_layer` refuses
/// moves past it and file readers reject deeper trees.
pub const MAX_TREE_DEPTH: usize = 64;

/// Most layers (rasters and folders) a document may hold.
pub const MAX_LAYERS: usize = 65_535;

/// Largest `next_id` a document may hold. Fresh ids stay below it, so
/// allocating one never overflows; layer adds are refused once it is
/// reached.
pub const MAX_NEXT_ID: u32 = u32::MAX - MAX_LAYERS as u32;

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
    /// Bumped by every content change (pixels, props, paper, structure).
    revision: u64,
    /// Bumped by view-only changes (active layer, folder expansion).
    view_revision: u64,
    /// Pixel selection (document data, as in CSP); empty = none.
    selection: Selection,
    /// Bumped by every selection change, recorded or not (outline caches).
    selection_rev: u64,
    page: Option<PageSetup>,
}

/// Everything needed to rebuild a document, e.g. by a file reader.
/// [`Document::from_parts`] validates it.
pub struct DocParts {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub paper: Option<[u16; 4]>,
    /// Every layer, in any order. Folder children are ordered bottom → top.
    pub layers: Vec<Layer>,
    /// Top-level layers, bottom → top.
    pub root: Vec<LayerId>,
    pub active: LayerId,
    pub next_id: u32,
}

/// Why [`Document::from_parts`] refused a layer tree.
#[derive(Debug, Clone, PartialEq)]
pub enum TreeError {
    ZeroId,
    DuplicateId(LayerId),
    /// The root or a folder lists an id with no layer.
    MissingLayer(LayerId),
    MultipleParents(LayerId),
    /// A raster layer named as a parent. `DocParts` cannot express this
    /// (rasters have no children); readers that store parent ids report it.
    ChildOfRaster(LayerId),
    /// A layer not reachable from the root (including folder cycles).
    Orphan(LayerId),
    TooDeep,
    TooManyLayers,
    NoRasterLayer,
    BadActive,
    /// `next_id` not above every layer id, or above [`MAX_NEXT_ID`].
    BadNextId,
    /// Zero width, height or dpi.
    BadPage,
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
            next_id: 2,
            active: LayerId(0),
            dirty: DirtyRegion::default(),
            revision: 0,
            view_revision: 0,
            selection: Selection::default(),
            selection_rev: 0,
            page: None,
        };
        let id = LayerId(1);
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
        if self.paper != paper {
            self.paper = paper;
            self.dirty.mark_all();
            self.bump();
        }
    }

    /// Content revision: changes whenever the document's saved content may
    /// have changed. Compare against a remembered value to detect edits.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// View revision: changes with the active layer or folder expansion.
    pub fn view_revision(&self) -> u64 {
        self.view_revision
    }

    pub(crate) fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn bump_view(&mut self) {
        self.view_revision = self.view_revision.wrapping_add(1);
    }

    /// The id the next new layer will get.
    pub fn next_layer_id(&self) -> u32 {
        self.next_id
    }

    /// A copy for saving off the UI thread. O(layers): pixel maps and tiles
    /// are `Arc`-shared and copied on the next write. The copy has a clean
    /// dirty set and the same revisions.
    pub fn snapshot(&self) -> Document {
        Document {
            width: self.width,
            height: self.height,
            dpi: self.dpi,
            paper: self.paper,
            layers: self.layers.clone(),
            root: self.root.clone(),
            next_id: self.next_id,
            active: self.active,
            dirty: DirtyRegion::default(),
            revision: self.revision,
            view_revision: self.view_revision,
            selection: self.selection.clone(),
            selection_rev: self.selection_rev,
            page: self.page,
        }
    }

    /// Build a document from untrusted parts, checking every invariant the
    /// compositor and tree code rely on (they index layers by id).
    pub fn from_parts(p: DocParts) -> Result<Document, TreeError> {
        if p.width == 0 || p.height == 0 || p.dpi == 0 {
            return Err(TreeError::BadPage);
        }
        if p.layers.len() > MAX_LAYERS {
            return Err(TreeError::TooManyLayers);
        }
        let mut index = AHashMap::with_capacity(p.layers.len());
        for (i, l) in p.layers.iter().enumerate() {
            if l.id.0 == 0 {
                return Err(TreeError::ZeroId);
            }
            if index.insert(l.id, i).is_some() {
                return Err(TreeError::DuplicateId(l.id));
            }
        }
        // Each referenced id exists and is referenced once.
        let mut parented = AHashSet::with_capacity(p.layers.len());
        let children = p.layers.iter().filter_map(Layer::children).flatten();
        for &c in p.root.iter().chain(children) {
            if !index.contains_key(&c) {
                return Err(TreeError::MissingLayer(c));
            }
            if !parented.insert(c) {
                return Err(TreeError::MultipleParents(c));
            }
        }
        // With single parents every layer is visited at most once, and any
        // layer the walk misses is an orphan (or part of a folder cycle).
        let mut reached = vec![false; p.layers.len()];
        let mut stack: Vec<(LayerId, usize)> = p.root.iter().map(|&id| (id, 1)).collect();
        while let Some((id, depth)) = stack.pop() {
            if depth > MAX_TREE_DEPTH {
                return Err(TreeError::TooDeep);
            }
            let i = index[&id];
            reached[i] = true;
            if let Some(children) = p.layers[i].children() {
                stack.extend(children.iter().map(|&c| (c, depth + 1)));
            }
        }
        if let Some(i) = reached.iter().position(|r| !r) {
            return Err(TreeError::Orphan(p.layers[i].id));
        }
        if p.layers.iter().all(Layer::is_folder) {
            return Err(TreeError::NoRasterLayer);
        }
        if !index.contains_key(&p.active) {
            return Err(TreeError::BadActive);
        }
        if p.next_id > MAX_NEXT_ID || p.layers.iter().any(|l| l.id.0 >= p.next_id) {
            return Err(TreeError::BadNextId);
        }
        let mut doc = Document {
            width: p.width,
            height: p.height,
            dpi: p.dpi,
            paper: p.paper,
            layers: p.layers.into_iter().map(|l| (l.id, l)).collect(),
            root: p.root,
            next_id: p.next_id,
            active: p.active,
            dirty: DirtyRegion::default(),
            revision: 0,
            view_revision: 0,
            selection: Selection::default(),
            selection_rev: 0,
            page: None,
        };
        doc.dirty.mark_all();
        Ok(doc)
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
        if self.active != id && self.layers.contains_key(&id) {
            self.active = id;
            self.bump_view();
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
        // `self.bump()` would conflict with the layer borrow.
        self.revision = self.revision.wrapping_add(1);
        Some((grid, &mut self.dirty))
    }

    /// [`Self::paint_target`] plus the selection painting is limited to:
    /// `Some` exactly when there is one.
    pub fn paint_target_masked(&mut self, id: LayerId) -> Option<(&mut TileGrid, &mut DirtyRegion, Option<&Selection>)> {
        let layer = self.layers.get_mut(&id)?;
        let grid = layer.raster_mut()?;
        self.revision = self.revision.wrapping_add(1);
        let mask = (!self.selection.is_empty()).then_some(&self.selection);
        Some((grid, &mut self.dirty, mask))
    }

    pub fn dirty_mut(&mut self) -> &mut DirtyRegion {
        &mut self.dirty
    }

    // ----- selection and page setup ------------------------------------------

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn selection_rev(&self) -> u64 {
        self.selection_rev
    }

    pub fn has_selection(&self) -> bool {
        !self.selection.is_empty()
    }

    /// Replace the selection, returning the old one. A content change
    /// (the selection is saved); no tiles are dirtied.
    pub fn swap_selection(&mut self, s: Selection) -> Selection {
        let old = std::mem::replace(&mut self.selection, s);
        self.selection_rev = self.selection_rev.wrapping_add(1);
        self.bump();
        old
    }

    /// Set the selection without counting it as an edit (file readers).
    pub fn set_selection_unrecorded(&mut self, s: Selection) {
        self.selection = s;
        self.selection_rev = self.selection_rev.wrapping_add(1);
    }

    pub fn page_setup(&self) -> Option<&PageSetup> {
        self.page.as_ref()
    }

    /// Replace the page setup. Returns the old value when it changed.
    pub fn set_page_setup(&mut self, s: Option<PageSetup>) -> Option<Option<PageSetup>> {
        if self.page == s {
            return None;
        }
        let old = std::mem::replace(&mut self.page, s);
        self.bump();
        Some(old)
    }

    /// Set the page setup without counting it as an edit (file readers,
    /// the New dialog).
    pub fn set_page_unrecorded(&mut self, s: Option<PageSetup>) {
        self.page = s;
    }

    // ----- frames --------------------------------------------------------------

    /// The frame of a frame border folder.
    pub fn frame(&self, id: LayerId) -> Option<&Arc<Frame>> {
        match &self.layers.get(&id)?.content {
            LayerContent::Folder { frame, .. } => frame.as_ref(),
            LayerContent::Raster(_) => None,
        }
    }

    /// Replace a folder's frame. Returns the old one when it changed;
    /// `None` for a missing layer, a raster layer or the same frame.
    pub fn set_frame(&mut self, id: LayerId, f: Option<Arc<Frame>>) -> Option<Option<Arc<Frame>>> {
        let page_tiles = (self.tiles_wide(), self.tiles_high());
        let Some(Layer { content: LayerContent::Folder { frame, .. }, .. }) = self.layers.get_mut(&id) else {
            return None;
        };
        let same = match (&*frame, &f) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if same {
            return None;
        }
        if let Some(new) = &f {
            debug_assert_eq!(new.tiles(), page_tiles, "frame built for another page size");
        }
        let toggled = frame.is_some() != f.is_some();
        let new = f.clone();
        let old = std::mem::replace(frame, f);
        for c in old.iter().chain(new.iter()).flat_map(|f| f.touched_tiles()) {
            self.dirty.mark(c);
        }
        if toggled {
            // Masking starts or stops: every tile of the children changes.
            self.mark_layer_dirty(id);
        }
        self.bump();
        Some(old)
    }

    /// The nearest ancestor-or-self of `id` that has a frame.
    pub fn frame_folder_of(&self, id: LayerId) -> Option<LayerId> {
        let mut at = id;
        loop {
            if self.frame(at).is_some() {
                return Some(at);
            }
            at = self.location(at)?.0?;
        }
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
        self.bump();
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
            LayerContent::Folder { children, frame, .. } => {
                // A frame's border shows even where no child has pixels.
                for c in frame.iter().flat_map(|f| f.touched_tiles()) {
                    self.dirty.mark(c);
                }
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
        if let Some(Layer { content: LayerContent::Folder { expanded, .. }, .. }) = self.layers.get_mut(&id)
            && *expanded != open
        {
            *expanded = open;
            self.bump_view();
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
            if let Some(Layer { content: LayerContent::Folder { children, expanded: true, .. }, .. }) =
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

    /// A fresh id, or `None` once ids reach [`MAX_NEXT_ID`].
    fn alloc_id(&mut self) -> Option<LayerId> {
        if self.next_id >= MAX_NEXT_ID {
            return None;
        }
        let id = LayerId(self.next_id);
        self.next_id += 1;
        Some(id)
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
        self.bump();
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
        self.bump();
        id
    }

    /// `None` when layer ids are used up.
    pub fn add_raster_layer(&mut self) -> Option<LayerId> {
        let id = self.alloc_id()?;
        let name = format!("Layer {}", id.0);
        Some(self.insert_above_active(Layer {
            id,
            props: LayerProps::named(name),
            content: LayerContent::Raster(TileGrid::new()),
        }))
    }

    /// `None` when layer ids are used up.
    pub fn add_folder(&mut self) -> Option<LayerId> {
        let id = self.alloc_id()?;
        let mut props = LayerProps::named(format!("Folder {}", id.0));
        props.blend = BlendMode::PassThrough;
        Some(self.insert_above_active(Layer {
            id,
            props,
            content: LayerContent::Folder { children: Vec::new(), expanded: true, frame: None },
        }))
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
        self.bump();
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
        self.bump();
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

    /// Nesting depth of a layer (top level = 1).
    fn depth_of(&self, id: LayerId) -> usize {
        let mut depth = 1;
        let mut at = id;
        while let Some((Some(parent), _)) = self.location(at) {
            depth += 1;
            at = parent;
        }
        depth
    }

    /// Levels a subtree occupies (a raster or empty folder = 1).
    fn subtree_height(&self, id: LayerId) -> usize {
        match self.layers.get(&id).and_then(|l| l.children()) {
            Some(children) => 1 + children.iter().map(|&c| self.subtree_height(c)).max().unwrap_or(0),
            None => 1,
        }
    }

    /// Whether `id` may move into `parent` (`None` = top level): not into
    /// itself or its own subfolders, only into folders, and no deeper than
    /// [`MAX_TREE_DEPTH`].
    pub fn can_move(&self, id: LayerId, parent: Option<LayerId>) -> bool {
        parent.is_none_or(|p| {
            p != id
                && !self.is_descendant(id, p)
                && self.layers.get(&p).is_some_and(|l| l.is_folder())
                && self.depth_of(p) + self.subtree_height(id) <= MAX_TREE_DEPTH
        })
    }

    /// Move `id` to `index` within `parent` (`None` = top level). Refuses
    /// what [`Self::can_move`] refuses.
    pub fn move_layer(&mut self, id: LayerId, parent: Option<LayerId>, index: usize) -> bool {
        if !self.can_move(id, parent) {
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
        self.bump();
        true
    }

    /// Duplicate a layer (deep for folders). Pixels are shared until edited.
    /// Refuses when the copy would exceed [`MAX_LAYERS`] or use up the ids.
    pub fn duplicate_layer(&mut self, id: LayerId) -> Option<LayerId> {
        let (parent, index) = self.location(id)?;
        let mut subtree = Vec::new();
        self.collect_subtree(id, &mut subtree);
        if self.layers.len() + subtree.len() > MAX_LAYERS
            || self.next_id as usize + subtree.len() > MAX_NEXT_ID as usize
        {
            return None;
        }
        let copy = self.clone_subtree(id);
        if let Some(l) = self.layers.get_mut(&copy) {
            l.props.name.push_str(" copy");
        }
        let siblings = self.siblings_mut(parent);
        siblings.insert(index + 1, copy);
        self.active = copy;
        self.dirty.mark_all();
        self.bump();
        Some(copy)
    }

    fn clone_subtree(&mut self, id: LayerId) -> LayerId {
        let src = self.layers[&id].clone();
        let new_id = self.alloc_id().expect("room checked by duplicate_layer");
        let content = match src.content {
            LayerContent::Raster(g) => LayerContent::Raster(g),
            LayerContent::Folder { children, expanded, frame } => LayerContent::Folder {
                children: children.into_iter().map(|c| self.clone_subtree(c)).collect(),
                expanded,
                frame,
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
        self.bump();
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
        let second = doc.add_raster_layer().unwrap();
        let folder = doc.add_folder().unwrap();
        let inner = {
            doc.set_active(folder);
            doc.add_raster_layer().unwrap()
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
        let outer = doc.add_folder().unwrap();
        let inner = doc.add_folder().unwrap();
        assert!(doc.move_layer(inner, Some(outer), 0));
        assert!(!doc.move_layer(outer, Some(inner), 0));
        assert!(!doc.move_layer(outer, Some(outer), 0));
    }

    #[test]
    fn cannot_delete_last_raster() {
        let mut doc = Document::new(64, 64, 72);
        let only = doc.active();
        assert!(!doc.delete_layer(only));
        let other = doc.add_raster_layer().unwrap();
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
        let first = doc.add_raster_layer().unwrap();
        let second = doc.add_raster_layer().unwrap();
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
        let clip = doc.add_raster_layer().unwrap();
        set_clip(&mut doc, clip);
        let top = doc.add_raster_layer().unwrap();
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

    fn raster(id: u32) -> Layer {
        Layer { id: LayerId(id), props: LayerProps::named("r"), content: LayerContent::Raster(TileGrid::new()) }
    }

    fn folder(id: u32, children: &[u32]) -> Layer {
        Layer {
            id: LayerId(id),
            props: LayerProps::named("f"),
            content: LayerContent::Folder {
                children: children.iter().map(|&c| LayerId(c)).collect(),
                expanded: true,
                frame: None,
            },
        }
    }

    /// Raster 1 and folder 2 { raster 3 } at the top level.
    fn parts() -> DocParts {
        DocParts {
            width: 100,
            height: 50,
            dpi: 350,
            paper: None,
            layers: vec![raster(1), folder(2, &[3]), raster(3)],
            root: vec![LayerId(1), LayerId(2)],
            active: LayerId(3),
            next_id: 7,
        }
    }

    #[test]
    fn from_parts_accepts_a_valid_tree() {
        let doc = Document::from_parts(parts()).unwrap();
        assert_eq!(doc.root(), &[LayerId(1), LayerId(2)]);
        assert_eq!(doc.layer(LayerId(2)).unwrap().children(), Some(&[LayerId(3)][..]));
        assert_eq!((doc.active(), doc.next_layer_id(), doc.paper()), (LayerId(3), 7, None));
        assert_eq!(doc.tree_depth(), 2);
        let mut dirty = doc.dirty;
        assert!(dirty.drain_into(&mut Vec::new()), "a loaded document is all dirty");
    }

    #[test]
    fn from_parts_rejects_every_tree_error() {
        type Case = (TreeError, fn(&mut DocParts));
        let cases: [Case; 16] = [
            (TreeError::BadPage, |p| p.width = 0),
            (TreeError::BadPage, |p| p.dpi = 0),
            (TreeError::ZeroId, |p| p.layers[0].id = LayerId(0)),
            (TreeError::DuplicateId(LayerId(3)), |p| p.layers.push(raster(3))),
            (TreeError::MissingLayer(LayerId(9)), |p| p.root.push(LayerId(9))),
            (TreeError::MultipleParents(LayerId(3)), |p| p.root.push(LayerId(3))),
            (TreeError::MultipleParents(LayerId(3)), |p| p.layers[1] = folder(2, &[3, 3])),
            (TreeError::Orphan(LayerId(4)), |p| p.layers.push(raster(4))),
            // A folder cycle detached from the root: 4 → 5 → 4.
            (TreeError::Orphan(LayerId(4)), |p| p.layers.extend([folder(4, &[5]), folder(5, &[4])])),
            (TreeError::MultipleParents(LayerId(2)), |p| p.layers[1] = folder(2, &[2, 3])),
            // A folder holding itself, detached from the root.
            (TreeError::Orphan(LayerId(2)), |p| {
                p.layers[1] = folder(2, &[2, 3]);
                p.root.pop();
            }),
            (TreeError::NoRasterLayer, |p| {
                p.layers = vec![folder(1, &[])];
                p.root = vec![LayerId(1)];
                p.active = LayerId(1);
            }),
            (TreeError::BadActive, |p| p.active = LayerId(6)),
            (TreeError::BadNextId, |p| p.next_id = 3),
            (TreeError::BadNextId, |p| p.next_id = u32::MAX),
            (TreeError::BadNextId, |p| p.next_id = MAX_NEXT_ID + 1),
        ];
        for (want, edit) in cases {
            let mut p = parts();
            edit(&mut p);
            assert_eq!(Document::from_parts(p).err(), Some(want.clone()), "{want:?}");
        }

        // Depth: a chain of folders 1 → 2 → … ending in a raster.
        let chain = |levels: u32| {
            let mut layers: Vec<Layer> = (1..levels).map(|i| folder(i, &[i + 1])).collect();
            layers.push(raster(levels));
            DocParts { layers, root: vec![LayerId(1)], active: LayerId(levels), next_id: levels + 1, ..parts() }
        };
        assert!(Document::from_parts(chain(MAX_TREE_DEPTH as u32)).is_ok());
        assert_eq!(Document::from_parts(chain(MAX_TREE_DEPTH as u32 + 1)).err(), Some(TreeError::TooDeep));

        let many = DocParts {
            layers: (1..=MAX_LAYERS as u32 + 1).map(raster).collect(),
            root: (1..=MAX_LAYERS as u32 + 1).map(LayerId).collect(),
            active: LayerId(1),
            next_id: MAX_LAYERS as u32 + 2,
            ..parts()
        };
        assert_eq!(Document::from_parts(many).err(), Some(TreeError::TooManyLayers));
    }

    #[test]
    fn move_layer_refuses_nesting_past_max_depth() {
        let mut doc = Document::new(64, 64, 72);
        let first = doc.active();
        let mut parent = None;
        let mut deepest = None;
        for _ in 0..MAX_TREE_DEPTH - 1 {
            let f = doc.add_folder().unwrap();
            assert!(doc.move_layer(f, parent, 0));
            parent = Some(f);
            deepest = Some(f);
        }
        assert_eq!(doc.tree_depth(), MAX_TREE_DEPTH - 1);
        // A raster fits at depth 64; a folder holding a raster does not.
        let r = doc.add_raster_layer().unwrap();
        assert!(doc.move_layer(r, deepest, 0));
        assert_eq!(doc.tree_depth(), MAX_TREE_DEPTH);
        doc.set_active(first);
        let f = doc.add_folder().unwrap();
        let inner = doc.add_raster_layer().unwrap();
        assert!(doc.move_layer(inner, Some(f), 0));
        assert!(!doc.move_layer(f, deepest, 0));
        assert_eq!(doc.tree_depth(), MAX_TREE_DEPTH);
    }

    #[test]
    fn revision_bumps_on_content_and_view_changes() {
        use crate::history::{Edit, History};

        let mut doc = Document::new(64, 64, 72);
        let base = doc.active();
        let mut h = History::default();
        let mut last = (doc.revision(), doc.view_revision());
        // Each step asserts which counter moved: (content, view).
        let mut step = |doc: &mut Document, what: &str, want: (bool, bool), f: &mut dyn FnMut(&mut Document)| {
            f(doc);
            let now = (doc.revision(), doc.view_revision());
            assert_eq!((now.0 != last.0, now.1 != last.1), want, "{what}");
            last = now;
        };
        let content = (true, false);
        let view = (false, true);
        let none = (false, false);

        step(&mut doc, "paint_target", content, &mut |d| {
            d.paint_target(base).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][0] = [1; 4];
        });
        step(&mut doc, "dirty_mut", none, &mut |d| d.dirty_mut().mark_all());
        step(&mut doc, "mark_layer_dirty", none, &mut |d| d.mark_layer_dirty(base));
        step(&mut doc, "set_paper same", none, &mut |d| d.set_paper(Some(PAPER_WHITE)));
        step(&mut doc, "set_paper", content, &mut |d| d.set_paper(None));
        let mut p = doc.layer(base).unwrap().props.clone();
        step(&mut doc, "set_props same", none, &mut |d| assert!(d.set_props(base, p.clone()).is_none()));
        p.opacity = 0.5;
        step(&mut doc, "set_props", content, &mut |d| assert!(d.set_props(base, p.clone()).is_some()));
        let mut top = base;
        step(&mut doc, "add_raster_layer", content, &mut |d| top = d.add_raster_layer().unwrap());
        let mut f = base;
        step(&mut doc, "add_folder", content, &mut |d| f = d.add_folder().unwrap());
        step(&mut doc, "set_active", view, &mut |d| d.set_active(base));
        step(&mut doc, "set_active same", none, &mut |d| d.set_active(base));
        step(&mut doc, "set_active missing", none, &mut |d| d.set_active(LayerId(999)));
        step(&mut doc, "collapse", view, &mut |d| d.set_folder_expanded(f, false));
        step(&mut doc, "collapse same", none, &mut |d| d.set_folder_expanded(f, false));
        step(&mut doc, "move_layer", content, &mut |d| assert!(d.move_layer(top, Some(f), 0)));
        step(&mut doc, "move_layer refused", none, &mut |d| assert!(!d.move_layer(f, Some(f), 0)));
        step(&mut doc, "shift_layer", content, &mut |d| assert!(d.shift_layer(f, -1)));
        step(&mut doc, "shift_layer refused", none, &mut |d| assert!(!d.shift_layer(base, 5)));
        let mut copy = base;
        step(&mut doc, "duplicate_layer", content, &mut |d| copy = d.duplicate_layer(base).unwrap());
        step(&mut doc, "merge_down", content, &mut |d| assert!(d.merge_down(copy)));
        step(&mut doc, "merge_down refused", none, &mut |d| assert!(!d.merge_down(f)));
        step(&mut doc, "clear_layer", content, &mut |d| assert!(d.clear_layer(base)));
        step(&mut doc, "delete_layer", content, &mut |d| assert!(d.delete_layer(top)));
        step(&mut doc, "delete_layer refused", none, &mut |d| assert!(!d.delete_layer(LayerId(999))));
        step(&mut doc, "snapshot", none, &mut |d| {
            let s = d.snapshot();
            assert_eq!((s.revision(), s.view_revision()), (d.revision(), d.view_revision()));
        });

        // History apply paths: pixels, props and structure, both directions.
        h.push(Edit::Pixels { layer: base, tiles: vec![(TileCoord::new(3, 3), None)] }, &doc);
        let before = doc.layer(base).unwrap().props.clone();
        let mut changed = before.clone();
        changed.visible = false;
        doc.set_props(base, changed);
        h.push(Edit::Props { layer: base, props: before }, &doc);
        let snap = doc.snapshot_structure();
        doc.add_raster_layer().unwrap();
        h.push(Edit::Structure(Box::new(snap)), &doc);
        step(&mut doc, "setup", content, &mut |_| ());
        for what in ["undo structure", "undo props", "undo pixels"] {
            step(&mut doc, what, content, &mut |d| {
                h.undo(d);
            });
        }
        for what in ["redo pixels", "redo props", "redo structure"] {
            step(&mut doc, what, content, &mut |d| {
                h.redo(d);
            });
        }
    }

    #[test]
    fn snapshot_shares_maps_until_written() {
        let mut doc = Document::new(64, 64, 72);
        let id = doc.active();
        let c = TileCoord::new(0, 0);
        put(&mut doc, id, c, 0, [5; 4]);
        let snap = doc.snapshot();
        assert!(snap.dirty.is_clean());
        let grid = |d: &Document| d.layer(id).unwrap().raster().unwrap().clone();
        assert!(grid(&doc).shares_storage(&grid(&snap)));

        put(&mut doc, id, c, 0, [7; 4]);
        put(&mut doc, id, TileCoord::new(1, 0), 0, [7; 4]);
        assert!(!grid(&doc).shares_storage(&grid(&snap)), "first write copies the map");
        let tile = |d: &Document, c| d.layer(id).unwrap().raster().unwrap().get(c).map(|t| t[0][0]);
        assert_eq!(tile(&snap, c), Some([5; 4]));
        assert_eq!(tile(&snap, TileCoord::new(1, 0)), None);
        assert_eq!(tile(&doc, c), Some([7; 4]));
    }

    #[test]
    fn duplicate_refuses_past_max_layers() {
        let mut doc = Document::new(64, 64, 72);
        let id = doc.active();
        let layers = (2..=MAX_LAYERS as u32).map(raster);
        doc.layers.extend(layers.map(|l| (l.id, l)));
        doc.root.extend((2..=MAX_LAYERS as u32).map(LayerId));
        doc.next_id = MAX_LAYERS as u32 + 1;
        assert_eq!(doc.layer_count(), MAX_LAYERS);
        assert!(doc.duplicate_layer(id).is_none());
    }

    #[test]
    fn adds_refuse_once_ids_run_out() {
        // The highest next_id a file may carry: every fresh id still fits.
        let p = DocParts { next_id: MAX_NEXT_ID - 2, ..parts() };
        let mut doc = Document::from_parts(p).unwrap();
        let folder = LayerId(2);
        assert!(doc.add_raster_layer().is_some());
        assert!(doc.duplicate_layer(folder).is_none(), "a folder copy needs two ids, one is left");
        assert_eq!(doc.add_folder(), Some(LayerId(MAX_NEXT_ID - 1)));
        assert_eq!(doc.next_layer_id(), MAX_NEXT_ID);
        let count = doc.layer_count();
        assert_eq!(doc.add_raster_layer(), None);
        assert_eq!(doc.add_folder(), None);
        assert_eq!(doc.duplicate_layer(LayerId(1)), None);
        assert_eq!(doc.layer_count(), count, "no id was reused");
    }

    fn frame_shape() -> crate::frame::FrameShape {
        crate::frame::FrameShape {
            panels: Vec::new(),
            border: crate::frame::BorderStyle { width: 2.0, color: [0, 0, 0, ONE_U16] },
        }
    }

    fn some_selection() -> Selection {
        let mut s = Selection::new();
        s.insert_tile(TileCoord::new(0, 0), crate::selection::full_mask().clone());
        s
    }

    fn some_page() -> PageSetup {
        let trim = crate::geom::RectF { x: 4.0, y: 4.0, w: 50.0, h: 50.0 };
        PageSetup { trim, bleed: 2.0, safe: 3.0, inner: crate::geom::RectF::default(), unit: 2 }
    }

    #[test]
    fn swap_selection_bumps_both_revisions() {
        let mut doc = Document::new(128, 64, 72);
        let id = doc.active();
        assert!(!doc.has_selection());
        assert!(doc.paint_target_masked(id).unwrap().2.is_none(), "no selection, no mask");
        doc.dirty.drain_into(&mut Vec::new());
        let (rev, sel_rev) = (doc.revision(), doc.selection_rev());
        let old = doc.swap_selection(some_selection());
        assert!(old.is_empty() && doc.has_selection());
        assert_ne!(doc.revision(), rev);
        assert_ne!(doc.selection_rev(), sel_rev);
        assert!(doc.dirty.is_clean(), "a selection change dirties no tiles");
        let (_, _, mask) = doc.paint_target_masked(id).unwrap();
        assert!(mask.is_some_and(|m| m.tile_count() == 1));
        assert!(doc.paint_target_masked(LayerId(99)).is_none());
    }

    #[test]
    fn unrecorded_setters_do_not_bump_revision() {
        let mut doc = Document::new(64, 64, 72);
        let (rev, sel_rev) = (doc.revision(), doc.selection_rev());
        doc.set_selection_unrecorded(some_selection());
        assert!(doc.has_selection());
        assert_ne!(doc.selection_rev(), sel_rev, "outline caches still see the change");
        doc.set_page_unrecorded(Some(some_page()));
        assert_eq!(doc.page_setup(), Some(&some_page()));
        assert_eq!(doc.revision(), rev);

        // The recorded setter returns the old value and bumps, only on change.
        assert_eq!(doc.set_page_setup(Some(some_page())), None);
        assert_eq!(doc.revision(), rev);
        assert_eq!(doc.set_page_setup(None), Some(Some(some_page())));
        assert_ne!(doc.revision(), rev);
        assert_eq!(doc.page_setup(), None);
    }

    #[test]
    fn snapshot_shares_the_selection() {
        let mut doc = Document::new(64, 64, 72);
        doc.swap_selection(some_selection());
        doc.set_page_setup(Some(some_page()));
        let snap = doc.snapshot();
        assert!(snap.selection().shares_storage(doc.selection()));
        assert_eq!(snap.selection_rev(), doc.selection_rev());
        assert_eq!(snap.page_setup(), doc.page_setup());
        doc.swap_selection(Selection::default());
        assert!(snap.has_selection(), "the snapshot keeps its selection");
    }

    #[test]
    fn folder_dirty_marking_covers_its_frame() {
        let mut doc = Document::new(256, 256, 72);
        let raster = doc.active();
        let folder = doc.add_folder().unwrap();
        let t = TileCoord::new(3, 2);
        let f = Frame::with_full_tiles(frame_shape(), 256, 256, &[t]);
        let mut out = Vec::new();
        doc.dirty.drain_into(&mut out);
        let rev = doc.revision();

        assert!(matches!(doc.set_frame(folder, Some(f.clone())), Some(None)));
        assert_ne!(doc.revision(), rev);
        assert!(!doc.dirty.drain_into(&mut out));
        assert!(out.contains(&t), "set_frame marks the frame's tiles: {out:?}");
        assert!(Arc::ptr_eq(doc.frame(folder).unwrap(), &f));

        let rev = doc.revision();
        assert!(doc.set_frame(folder, Some(f.clone())).is_none(), "same frame");
        assert!(doc.set_frame(raster, Some(f.clone())).is_none(), "rasters have no frame");
        assert!(doc.set_frame(LayerId(99), None).is_none());
        assert_eq!(doc.revision(), rev);

        // The folder has no children: only its frame's tiles are dirtied.
        doc.mark_layer_dirty(folder);
        assert!(!doc.dirty.drain_into(&mut out));
        assert_eq!(out, vec![t]);

        let copy = doc.duplicate_layer(folder).unwrap();
        assert!(Arc::ptr_eq(doc.frame(copy).unwrap(), &f), "duplicate copies the frame");
        assert!(matches!(doc.set_frame(folder, None), Some(Some(old)) if Arc::ptr_eq(&old, &f)));
        assert!(doc.frame(folder).is_none());
    }

    #[test]
    fn frame_folder_of_walks_ancestors() {
        let mut doc = Document::new(64, 64, 72);
        let base = doc.active();
        let outer = doc.add_folder().unwrap();
        let inner = doc.add_folder().unwrap();
        let r = doc.add_raster_layer().unwrap();
        assert!(doc.move_layer(inner, Some(outer), 0));
        assert!(doc.move_layer(r, Some(inner), 0));
        assert_eq!(doc.frame_folder_of(r), None);

        doc.set_frame(outer, Some(Frame::build(frame_shape(), 64, 64)));
        assert_eq!(doc.frame_folder_of(r), Some(outer));
        assert_eq!(doc.frame_folder_of(inner), Some(outer));
        assert_eq!(doc.frame_folder_of(outer), Some(outer));
        assert_eq!(doc.frame_folder_of(base), None);
        assert_eq!(doc.frame_folder_of(LayerId(99)), None);

        doc.set_frame(inner, Some(Frame::build(frame_shape(), 64, 64)));
        assert_eq!(doc.frame_folder_of(r), Some(inner), "the nearest one");
    }

    #[test]
    fn can_move_matches_move_layer() {
        let mut doc = Document::new(64, 64, 72);
        let mut deepest = None;
        for _ in 0..MAX_TREE_DEPTH - 2 {
            let f = doc.add_folder().unwrap();
            assert!(doc.move_layer(f, deepest, 0));
            deepest = Some(f);
        }
        doc.set_active(doc.root()[0]);
        let outer = doc.add_folder().unwrap();
        let inner = doc.add_folder().unwrap();
        assert!(doc.move_layer(inner, Some(outer), 0));
        let r = doc.add_raster_layer().unwrap();
        assert!(doc.move_layer(r, Some(inner), 0));
        // outer is three levels high: one too many below depth 62.
        for (id, parent, ok) in [
            (outer, deepest, false),
            (inner, deepest, true),
            (outer, Some(inner), false),
            (outer, Some(outer), false),
            (outer, Some(r), false),
            (outer, None, true),
        ] {
            assert_eq!(doc.can_move(id, parent), ok, "{id:?} into {parent:?}");
            let snap = doc.snapshot_structure();
            assert_eq!(doc.move_layer(id, parent, 0), ok);
            doc.swap_structure(snap);
        }
    }
}
