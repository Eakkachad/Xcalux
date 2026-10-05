//! Undo/redo built on copy-on-write tiles.
//!
//! A pixel edit stores the *previous* `Arc` of every tile it touched, so
//! recording costs a pointer clone per tile and memory grows only by the
//! tiles that actually changed. Undo swaps the stored tiles back in and
//! keeps the swapped-out ones as the redo entry.
//!
//! # Memory budget
//!
//! Besides the step limit, the undo stack is held to a byte budget
//! ([`undo_budget`], plans/lowend_ux_plan.md E1). A step costs what dropping
//! it frees: tiles still anywhere in the document cost nothing, and a tile
//! several steps hold is charged once. `Arc::strong_count` is never read: the
//! io TileCache and autosave clones pin document tiles from other threads, a
//! solid fill shares one tile over many coordinates and a duplicated layer
//! shares tiles inside the document, so counts say nothing about what only
//! history holds.
//!
//! Why the charges add up: every document change is a step and history is
//! linear (a push clears redo), and an `Arc` re-enters the document only
//! through [`Edit::apply`]. So a tile leaves the document at exactly one op,
//! and that op's step holds it and is costed after it. Older holders were
//! costed while the tile was in the document and charged 0 for it. Hence the
//! stored costs sum to the bytes only history holds when every step ran the
//! document scan. The scan runs only when a step could cause a trim; below
//! the budget a step skips it and may overcount (a tile still on a twin layer
//! or at another slot). So before a trim for the budget, any such step makes
//! the stack be re-costed in one pass, charging each history-only tile to its
//! newest holder: the op it left the document at, whose drop frees it. An
//! overcount therefore never costs an undo step.
//!
//! The re-cost walks only the snapshot maps that were not document maps when
//! their step was costed. A map that was one held only document tiles then;
//! any that left the document since did so at a newer op, whose step holds it
//! and is still on the stack (trims drop the oldest), so that step owns it.
//!
//! A [`TileGrid`] map that a structure snapshot shares with the document
//! costs 0 when its step is costed, and stays with the snapshot when the
//! document later copies it on write (a stroke on that layer). Its tiles are
//! covered as above, but its table is then history-only with no step that
//! removed it, even after that stroke is undone: up to (structure steps ×
//! raster layers) tables, 68 KiB each on A4 350 dpi and 272 KiB on B4 600
//! dpi. So each step records the maps it shared, and every push charges each
//! of those tables the document no longer holds to its newest holder.
//!
//! All of this needs the document on a step boundary at every push: no
//! stroke or transform preview in progress. Their old tiles are held outside
//! both the document and history, and a re-cost would charge them to any
//! snapshot that holds them. The exception is [`History::push_props`], which
//! the layer panel may call mid-stroke or mid-transform: it holds no tiles and
//! trims only for the step limit, leaving the budget to the next push.

use std::collections::VecDeque;
use std::sync::Arc;

use ahash::{AHashMap, AHashSet};

use crate::document::{DirtyRegion, Document, StructureSnapshot};
use crate::frame::Frame;
use crate::grid::TileGrid;
use crate::layer::{Layer, LayerContent, LayerId, LayerProps};
use crate::page::PageSetup;
use crate::selection::{MaskPixels, Selection, is_full};
use crate::tile::{TileCoord, TilePixels, TileRef};

pub const DEFAULT_LIMIT: usize = 200;
/// The strong and weak counts in front of every `Arc` allocation.
pub(crate) const ARC_COUNTS: usize = 2 * size_of::<usize>();
/// Heap bytes of one tile: pixels plus the Arc counts.
pub const TILE_BYTES: usize = size_of::<TilePixels>() + ARC_COUNTS;
const MASK_BYTES: usize = size_of::<MaskPixels>() + ARC_COUNTS;
pub const MIN_BUDGET: usize = 128 << 20;
pub const MAX_BUDGET: usize = 1 << 30;
/// The 4 GB tier: used when RAM is unknown and by [`History::new`] and
/// `default` (deterministic tests).
pub const DEFAULT_BUDGET: usize = 256 << 20;

/// Undo memory for a machine with `physical_ram` bytes (lowend_ux_plan E1):
/// RAM/16 clamped to 128 MiB..=1 GiB. RAM is rounded up to a whole GiB first,
/// because GlobalMemoryStatusEx reports usable memory (about 7.8 GiB on an
/// 8 GB machine).
pub fn undo_budget(physical_ram: Option<u64>) -> usize {
    physical_ram.filter(|&r| r > 0).map_or(DEFAULT_BUDGET, |r| {
        ((r.div_ceil(1 << 30) << 30) / 16).clamp(MIN_BUDGET as u64, MAX_BUDGET as u64) as usize
    })
}

/// Allocation of a std (hashbrown) table with `cap` capacity of
/// `entry`-byte entries: the buckets plus one control byte each and a group
/// of trailing control bytes.
pub(crate) fn table_bytes(cap: usize, entry: usize) -> usize {
    if cap == 0 {
        return 0;
    }
    let buckets = if cap < 8 { cap + 1 } else { cap / 7 * 8 };
    buckets * (entry + 1) + 16
}

// Dropped steps may be freed on another thread (`History::set_release`).
const _: fn() = || {
    fn send<T: Send>() {}
    send::<Edit>();
};

/// One undo step: the state to put back. Applying an edit returns the edit
/// that reverses it.
pub enum Edit {
    Pixels { layer: LayerId, tiles: Vec<(TileCoord, Option<TileRef>)> },
    Props { layer: LayerId, props: LayerProps },
    Structure(Box<StructureSnapshot>),
    /// The selection to restore.
    Selection(Box<Selection>),
    /// The page setup to restore.
    Page(Option<PageSetup>),
    /// The frame to restore on a folder.
    Frame { layer: LayerId, frame: Option<Arc<Frame>> },
    /// Several edits as one step, applied last to first.
    Batch(Vec<Edit>),
}

/// What part of the document a history step changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    Pixels(LayerId),
    Props(LayerId),
    Structure,
    Selection,
    Page,
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
            Edit::Selection(sel) => Edit::Selection(Box::new(doc.swap_selection(*sel))),
            Edit::Page(page) => Edit::Page(doc.set_page_setup(page).unwrap_or(page)),
            Edit::Frame { layer, frame } => {
                let old = doc.set_frame(layer, frame.clone()).unwrap_or(frame);
                Edit::Frame { layer, frame: old }
            }
            // The inverses come out last to first, which is the order that
            // undoes them (applied last to first again).
            Edit::Batch(edits) => Edit::Batch(edits.into_iter().rev().map(|e| e.apply(doc)).collect()),
        }
    }

    /// Report what applying this edit changes. A frame edit counts as a
    /// structure change (the composite of the folder changes).
    pub fn touched(&self, f: &mut impl FnMut(Touch)) {
        match self {
            Edit::Pixels { layer, .. } => f(Touch::Pixels(*layer)),
            Edit::Props { layer, .. } => f(Touch::Props(*layer)),
            Edit::Structure(_) | Edit::Frame { .. } => f(Touch::Structure),
            Edit::Selection(_) => f(Touch::Selection),
            Edit::Page(_) => f(Touch::Page),
            Edit::Batch(edits) => {
                for e in edits {
                    e.touched(f);
                }
            }
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
        // An exact-size copy: the step does not carry the largest stroke's
        // capacity (which the undo budget would charge), and the recorder
        // keeps its buffer.
        let tiles: Vec<_> = self.tiles.drain(..).collect();
        Some(Edit::Pixels { layer, tiles })
    }
}

struct Step {
    edit: Edit,
    /// What dropping this step frees (see the module docs).
    bytes: usize,
    /// Tiles charged, part of `bytes`.
    tiles: usize,
    /// Costed with the document scan (or had no candidate tiles).
    exact: bool,
    /// Snapshot maps that were not document maps when costed: the only ones
    /// a re-cost walks (see the module docs). Empty, and unallocated, for
    /// anything but a snapshot that deleted or rewrote layers.
    walked: Box<[usize]>,
    /// Snapshot maps that were document maps when costed, with their table
    /// bytes: charged while the document no longer holds them (see the
    /// module docs). Empty, and unallocated, for anything but a snapshot.
    shared: Box<[(usize, usize)]>,
    /// Bytes charged for `shared` tables, part of `bytes`.
    tables: usize,
}

/// Sets reused by every costing; they keep their capacity.
#[derive(Default)]
struct Scratch {
    /// Candidate tiles of the step being costed (by address).
    tiles: AHashSet<usize>,
    /// Maps, masks and frames already charged to this step.
    blocks: AHashSet<usize>,
    /// Maps of the document's raster layers.
    maps: AHashSet<usize>,
    /// Re-costing: each candidate tile and the index of its newest holder.
    owner: AHashMap<usize, usize>,
    /// Snapshot maps the step being costed walked.
    walked: Vec<usize>,
    /// Snapshot maps (and table bytes) the step being costed shares with the
    /// document.
    shared: Vec<(usize, usize)>,
}

/// What history holds, for the UI (lowend_ux_plan D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryUsage {
    pub undo_steps: usize,
    pub redo_steps: usize,
    pub undo_bytes: usize,
    pub redo_bytes: usize,
    pub budget: usize,
    pub limit: usize,
    /// Steps dropped for the budget since the last clear.
    pub trimmed: usize,
}

/// Undo and redo stacks, held to a step limit and a byte budget on the
/// undo side (see the module docs).
///
/// The costs rely on every document change being a step. Any future change
/// that rewrites document tiles without one (planned E10/E18/L2 storage
/// work) must fold into a step, or the budget undercounts.
pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    limit: usize,
    budget: usize,
    undo_bytes: usize,
    redo_bytes: usize,
    /// Steps dropped for the budget (not the step limit) since the last clear.
    trimmed: usize,
    /// Layer whose props entry on top of `undo` belongs to the gesture that
    /// is still running, and may absorb further coalescing pushes.
    props_open: Option<LayerId>,
    scratch: Scratch,
    /// Where dropped steps go; `None` drops them here.
    release: Option<Box<dyn FnMut(Edit) + Send>>,
    #[cfg(test)]
    always_scan: bool,
    #[cfg(test)]
    pushes: usize,
    /// Document walks: scans and re-costs.
    #[cfg(test)]
    doc_walks: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_LIMIT)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self::with_budget(limit, DEFAULT_BUDGET)
    }

    pub fn with_budget(limit: usize, budget: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            budget,
            undo_bytes: 0,
            redo_bytes: 0,
            trimmed: 0,
            props_open: None,
            scratch: Scratch::default(),
            release: None,
            #[cfg(test)]
            always_scan: false,
            #[cfg(test)]
            pushes: 0,
            #[cfg(test)]
            doc_walks: 0,
        }
    }

    /// Change the byte budget. Takes effect at the next push; nothing is
    /// dropped now.
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Hand dropped steps to `f` instead of freeing them here. Freeing costs
    /// about 3 µs per 32 KiB tile, so an A4 350 dpi whole-layer step takes
    /// about 9 ms and a B4 600 dpi one about 45 ms; that is why the app frees
    /// on another thread.
    pub fn set_release(&mut self, f: Box<dyn FnMut(Edit) + Send>) {
        self.release = Some(f);
    }

    /// Record an edit that has already been applied to the document. `doc`
    /// is the document the edit was already applied to, on a step boundary
    /// (see the module docs).
    pub fn push(&mut self, edit: Edit, doc: &Document) {
        self.release_redo();
        // If the step could trim, an unscanned step makes push_step re-cost
        // the stack, which scans the document itself: scan once.
        let recosts = self.undo.iter().any(|s| !s.exact);
        let step = self.step(edit, doc, self.undo_bytes, recosts);
        self.push_step(step, Some(doc));
    }

    /// Record a property change. `coalesce` marks it as part of a continuous
    /// gesture (e.g. dragging an opacity slider): the first such push opens
    /// an entry, and later coalescing pushes for the same layer merge into
    /// it until [`History::end_props_gesture`] or any other history operation
    /// closes it. Entries recorded before the gesture are never merged into.
    /// May be called off a step boundary (see the module docs).
    pub fn push_props(&mut self, layer: LayerId, before: LayerProps, coalesce: bool) {
        if coalesce && self.props_open == Some(layer) {
            return; // keep the gesture's oldest "before" state
        }
        let bytes = size_of::<Step>() + before.name.capacity();
        let edit = Edit::Props { layer, props: before };
        let (walked, shared) = (Box::default(), Box::default());
        let step = Step { edit, bytes, tiles: 0, exact: true, walked, shared, tables: 0 };
        self.push_step(step, None);
        self.props_open = coalesce.then_some(layer);
    }

    /// Push `step` and trim. `doc` is `None` for a props push, which may be
    /// off a step boundary: it trims only for the step limit.
    fn push_step(&mut self, step: Step, doc: Option<&Document>) {
        self.props_open = None;
        self.release_redo();
        self.undo_bytes += step.bytes;
        self.undo.push_back(step);
        #[cfg(test)]
        {
            self.pushes += 1;
        }
        let budget = match doc {
            Some(doc) => {
                self.charge_tables(doc);
                self.budget
            }
            None => usize::MAX,
        };
        // Oldest first; the newest step always stays.
        while self.undo.len() > 1 && (self.undo.len() > self.limit || self.undo_bytes > budget) {
            if self.undo.len() <= self.limit {
                // A trim for the budget: an overcount must not cause it.
                if let Some(doc) = doc
                    && self.undo.iter().any(|s| !s.exact)
                {
                    self.recost(doc);
                    continue;
                }
                self.trimmed += 1;
            }
            let old = self.undo.pop_front().expect("len > 1");
            self.undo_bytes -= old.bytes;
            give(&mut self.release, old.edit);
        }
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

    /// Undo one step. Returns what it changed (empty when there was
    /// nothing to undo). Never drops a step.
    pub fn undo(&mut self, doc: &mut Document) -> Vec<Touch> {
        self.props_open = None;
        let Some(s) = self.undo.pop_back() else { return Vec::new() };
        self.undo_bytes -= s.bytes;
        let touched = touches(&s.edit);
        let inv = s.edit.apply(doc);
        let step = self.step(inv, doc, self.redo_bytes, false);
        self.redo_bytes += step.bytes;
        self.redo.push(step);
        touched
    }

    /// Redo one step. Never drops a step.
    pub fn redo(&mut self, doc: &mut Document) -> Vec<Touch> {
        self.props_open = None;
        let Some(s) = self.redo.pop() else { return Vec::new() };
        self.redo_bytes -= s.bytes;
        let touched = touches(&s.edit);
        let inv = s.edit.apply(doc);
        let step = self.step(inv, doc, self.undo_bytes, false);
        self.undo_bytes += step.bytes;
        self.undo.push_back(step);
        touched
    }

    pub fn clear(&mut self) {
        self.props_open = None;
        for s in self.undo.drain(..) {
            give(&mut self.release, s.edit);
        }
        self.release_redo();
        self.undo_bytes = 0;
        self.trimmed = 0;
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn usage(&self) -> HistoryUsage {
        HistoryUsage {
            undo_steps: self.undo.len(),
            redo_steps: self.redo.len(),
            undo_bytes: self.undo_bytes,
            redo_bytes: self.redo_bytes,
            budget: self.budget,
            limit: self.limit,
            trimmed: self.trimmed,
        }
    }

    /// How many steps of `step_bytes` each the budget holds, within the step
    /// limit (at least 1: the newest step always stays). For a whole-layer
    /// step pass `tiles_wide × tiles_high × TILE_BYTES`.
    pub fn steps_that_fit(&self, step_bytes: usize) -> usize {
        if step_bytes == 0 {
            return self.limit;
        }
        (self.budget / step_bytes).clamp(1, self.limit)
    }

    /// Cost every step from now on with the document scan.
    #[cfg(test)]
    fn scan_always(&mut self) {
        self.always_scan = true;
    }

    fn release_redo(&mut self) {
        for s in self.redo.drain(..) {
            give(&mut self.release, s.edit);
        }
        self.redo_bytes = 0;
    }

    /// Cost `edit`: what dropping it would free, with `doc` the document
    /// after it. Tiles not at their own slot in the document are candidates;
    /// the document scan, which takes out those still anywhere in it, runs
    /// only when the result could push `stack_bytes` over the budget, and
    /// not then when `recosts` (a re-cost will follow and scan instead).
    fn step(&mut self, edit: Edit, doc: &Document, stack_bytes: usize, recosts: bool) -> Step {
        let s = &mut self.scratch;
        s.tiles.clear();
        s.blocks.clear();
        s.walked.clear();
        s.shared.clear();
        let mut bytes = size_of::<Step>() + walk(&edit, doc, s);
        let walked: Box<[usize]> = s.walked.as_slice().into();
        let shared: Box<[(usize, usize)]> = s.shared.as_slice().into();
        bytes += walked.len() * size_of::<usize>() + shared.len() * size_of::<(usize, usize)>();
        let n = s.tiles.len();
        let over = stack_bytes + bytes + n * TILE_BYTES > self.budget;
        let exact = n == 0 || self.scans_always() || (over && !recosts);
        if n > 0 && exact {
            self.scratch.scan(doc);
            #[cfg(test)]
            {
                self.doc_walks += 1;
            }
        }
        let tiles = self.scratch.tiles.len();
        Step { edit, bytes: bytes + tiles * TILE_BYTES, tiles, exact, walked, shared, tables: 0 }
    }

    /// Charge each `shared` table the document no longer holds to its newest
    /// holder (see the module docs). A table a newer step walked is that
    /// step's already.
    fn charge_tables(&mut self, doc: &Document) {
        if self.undo.iter().all(|s| s.shared.is_empty()) {
            return;
        }
        let s = &mut self.scratch;
        s.maps.clear();
        s.maps.extend(doc.layers.values().filter_map(Layer::raster).map(TileGrid::map_ptr));
        s.blocks.clear();
        for step in self.undo.iter_mut().rev() {
            let mut tables = 0;
            for &(m, bytes) in &step.shared {
                if !s.maps.contains(&m) && s.blocks.insert(m) {
                    tables += bytes;
                }
            }
            s.blocks.extend(step.walked.iter().copied());
            step.bytes = step.bytes - step.tables + tables;
            self.undo_bytes = self.undo_bytes - step.tables + tables;
            step.tables = tables;
        }
    }

    /// Re-cost the tiles of every undo step against `doc` in one pass,
    /// charging each history-only tile to its newest holder (see the module
    /// docs). Runs only before a trim for the budget, while some step skipped
    /// the document scan.
    fn recost(&mut self, doc: &Document) {
        #[cfg(test)]
        {
            self.doc_walks += 1;
        }
        let s = &mut self.scratch;
        s.owner.clear();
        s.blocks.clear();
        s.maps.clear();
        s.maps.extend(doc.layers.values().filter_map(Layer::raster).map(TileGrid::map_ptr));
        for (i, step) in self.undo.iter().enumerate().rev() {
            s.claim(&step.edit, &step.walked, doc, i);
        }
        s.maps.clear();
        'grids: for g in doc.layers.values().filter_map(Layer::raster) {
            if s.owner.is_empty() {
                break;
            }
            if !s.maps.insert(g.map_ptr()) {
                continue;
            }
            for (_, t) in g.iter() {
                if s.owner.remove(&(Arc::as_ptr(t) as usize)).is_some() && s.owner.is_empty() {
                    break 'grids;
                }
            }
        }
        for step in &mut self.undo {
            step.bytes -= step.tiles * TILE_BYTES;
            step.tiles = 0;
            step.exact = true;
        }
        for &i in s.owner.values() {
            self.undo[i].tiles += 1;
        }
        self.undo_bytes = 0;
        for step in &mut self.undo {
            step.bytes += step.tiles * TILE_BYTES;
            self.undo_bytes += step.bytes;
        }
    }

    #[cfg(test)]
    fn scans_always(&self) -> bool {
        self.always_scan
    }

    #[cfg(not(test))]
    fn scans_always(&self) -> bool {
        false
    }
}

fn give(release: &mut Option<Box<dyn FnMut(Edit) + Send>>, e: Edit) {
    match release {
        Some(f) => f(e),
        None => drop(e),
    }
}

impl Scratch {
    /// Note `t` (held at `c`) unless `live` holds the same tile there.
    #[inline]
    fn candidate(&mut self, live: Option<&TileGrid>, c: TileCoord, t: &TileRef) {
        if !at_slot(live, c, t) {
            self.tiles.insert(Arc::as_ptr(t) as usize);
        }
    }

    /// Make step `i` the owner of each candidate tile of `edit` that no newer
    /// step owns. Only the snapshot maps in `walked` can hold any (see the
    /// module docs). `maps` holds the document's maps, and `blocks` the
    /// snapshot maps a newer step already claimed (it owns all their tiles).
    fn claim(&mut self, edit: &Edit, walked: &[usize], doc: &Document, i: usize) {
        match edit {
            Edit::Pixels { layer, tiles } => {
                let live = doc.layer(*layer).and_then(Layer::raster);
                for (c, t) in tiles {
                    if let Some(t) = t
                        && !at_slot(live, *c, t)
                    {
                        self.owner.entry(Arc::as_ptr(t) as usize).or_insert(i);
                    }
                }
            }
            Edit::Structure(snap) => {
                for l in snap.layers.values() {
                    let Some(g) = l.raster() else { continue };
                    let m = g.map_ptr();
                    if !walked.contains(&m) || self.maps.contains(&m) || !self.blocks.insert(m) {
                        continue;
                    }
                    let live = doc.layer(l.id).and_then(Layer::raster);
                    for (c, t) in g.iter() {
                        if !at_slot(live, c, t) {
                            self.owner.entry(Arc::as_ptr(t) as usize).or_insert(i);
                        }
                    }
                }
            }
            Edit::Batch(edits) => {
                for e in edits {
                    self.claim(e, walked, doc, i);
                }
            }
            Edit::Props { .. } | Edit::Selection(_) | Edit::Page(_) | Edit::Frame { .. } => {}
        }
    }

    /// Drop the candidates the document still holds anywhere. Grids that
    /// share a map (untouched duplicates) are walked once.
    fn scan(&mut self, doc: &Document) {
        self.maps.clear();
        'grids: for g in doc.layers.values().filter_map(Layer::raster) {
            if !self.maps.insert(g.map_ptr()) {
                continue;
            }
            for (_, t) in g.iter() {
                if self.tiles.remove(&(Arc::as_ptr(t) as usize)) && self.tiles.is_empty() {
                    break 'grids;
                }
            }
        }
    }
}

/// Whether `live` holds `t` at `c`.
#[inline]
fn at_slot(live: Option<&TileGrid>, c: TileCoord, t: &TileRef) -> bool {
    live.and_then(|g| g.get_ref(c)).is_some_and(|l| Arc::ptr_eq(l, t))
}

/// Bytes of `edit` besides its candidate tiles, which go to `s.tiles`.
fn walk(edit: &Edit, doc: &Document, s: &mut Scratch) -> usize {
    match edit {
        Edit::Pixels { layer, tiles } => {
            let live = doc.layer(*layer).and_then(Layer::raster);
            for (c, t) in tiles {
                if let Some(t) = t {
                    s.candidate(live, *c, t);
                }
            }
            tiles.capacity() * size_of::<(TileCoord, Option<TileRef>)>()
        }
        Edit::Props { props, .. } => props.name.capacity(),
        Edit::Page(_) => 0,
        Edit::Structure(snap) => {
            let mut bytes = size_of::<StructureSnapshot>()
                + table_bytes(snap.layers.capacity(), size_of::<(LayerId, Layer)>())
                + snap.root.capacity() * size_of::<LayerId>();
            s.maps.clear();
            s.maps.extend(doc.layers.values().filter_map(Layer::raster).map(TileGrid::map_ptr));
            for l in snap.layers.values() {
                bytes += l.props.name.capacity();
                match &l.content {
                    LayerContent::Raster(g) => {
                        // Unchanged, or a twin of a document layer not
                        // written since: every tile is in the document.
                        if s.maps.contains(&g.map_ptr()) {
                            if !s.shared.iter().any(|&(m, _)| m == g.map_ptr()) {
                                s.shared.push((g.map_ptr(), g.map_bytes()));
                            }
                            continue;
                        }
                        if s.blocks.insert(g.map_ptr()) {
                            bytes += g.map_bytes();
                            s.walked.push(g.map_ptr());
                        }
                        let live = doc.layer(l.id).and_then(Layer::raster);
                        for (c, t) in g.iter() {
                            s.candidate(live, c, t);
                        }
                    }
                    LayerContent::Folder { children, frame, .. } => {
                        bytes += children.capacity() * size_of::<LayerId>();
                        if let Some(f) = frame {
                            bytes += frame_cost(f, doc, s);
                        }
                    }
                }
            }
            bytes
        }
        Edit::Selection(sel) => {
            let live = doc.selection();
            if live.shares_storage(sel) {
                return 0;
            }
            let mut bytes = 0;
            if s.blocks.insert(sel.map_ptr()) {
                bytes += sel.map_bytes();
            }
            for (c, m) in sel.tiles() {
                if is_full(m) || live.tile_ref(c).is_some_and(|l| Arc::ptr_eq(l, m)) {
                    continue;
                }
                if s.blocks.insert(Arc::as_ptr(m) as usize) {
                    bytes += MASK_BYTES;
                }
            }
            bytes
        }
        Edit::Frame { frame: Some(f), .. } => frame_cost(f, doc, s),
        Edit::Frame { frame: None, .. } => 0,
        Edit::Batch(edits) => {
            let mut bytes = edits.capacity() * size_of::<Edit>();
            for e in edits {
                bytes += walk(e, doc, s);
            }
            bytes
        }
    }
}

/// An old frame costs nothing while a document folder still holds it (a
/// duplicated frame folder shares it).
fn frame_cost(f: &Arc<Frame>, doc: &Document, s: &mut Scratch) -> usize {
    let held = doc
        .layers
        .values()
        .any(|l| matches!(&l.content, LayerContent::Folder { frame: Some(g), .. } if Arc::ptr_eq(f, g)));
    if held || !s.blocks.insert(Arc::as_ptr(f) as usize) { 0 } else { f.heap_bytes() + ARC_COUNTS }
}

fn touches(edit: &Edit) -> Vec<Touch> {
    let mut out = Vec::new();
    edit.touched(&mut |t| out.push(t));
    out
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
        h.push(stroke(&mut doc, &mut rec, 0, 100).unwrap(), &doc);
        h.push(stroke(&mut doc, &mut rec, 1, 200).unwrap(), &doc);
        assert_eq!((pixel(&doc, 0)[0], pixel(&doc, 1)[0]), (100, 200));

        let id = doc.active();
        assert_eq!(h.undo(&mut doc), [Touch::Pixels(id)]);
        assert_eq!((pixel(&doc, 0)[0], pixel(&doc, 1)[0]), (100, 0));
        h.undo(&mut doc);
        assert_eq!(pixel(&doc, 0)[0], 0);
        assert!(doc.active_layer().raster().unwrap().is_empty(), "first stroke created the tile");
        assert!(h.undo(&mut doc).is_empty(), "nothing left to undo");

        assert_eq!(h.redo(&mut doc), [Touch::Pixels(id)]);
        h.redo(&mut doc);
        assert_eq!((pixel(&doc, 0)[0], pixel(&doc, 1)[0]), (100, 200));
        assert!(h.redo(&mut doc).is_empty());
    }

    fn selection_at(x: i32) -> Selection {
        let mut s = Selection::new();
        s.insert_tile(TileCoord::new(x, 0), crate::selection::full_mask().clone());
        s
    }

    #[test]
    fn selection_edit_round_trips_the_same_storage() {
        let mut doc = Document::new(256, 64, 72);
        let mut h = History::default();
        let first = selection_at(0);
        doc.swap_selection(first.clone());
        let second = selection_at(1);
        let old = doc.swap_selection(second.clone());
        h.push(Edit::Selection(Box::new(old)), &doc);
        let rev = doc.revision();

        assert_eq!(h.undo(&mut doc), [Touch::Selection]);
        assert!(doc.selection().shares_storage(&first));
        assert_ne!(doc.revision(), rev);
        assert_eq!(h.redo(&mut doc), [Touch::Selection]);
        assert!(doc.selection().shares_storage(&second));
    }

    #[test]
    fn page_edit_round_trips() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::default();
        let trim = crate::geom::RectF { x: 1.0, y: 2.0, w: 30.0, h: 40.0 };
        let page = PageSetup { trim, bleed: 1.0, safe: 2.0, inner: crate::geom::RectF::default(), unit: 0 };
        let old = doc.set_page_setup(Some(page)).unwrap();
        h.push(Edit::Page(old), &doc);
        assert_eq!(h.undo(&mut doc), [Touch::Page]);
        assert_eq!(doc.page_setup(), None);
        assert_eq!(h.redo(&mut doc), [Touch::Page]);
        assert_eq!(doc.page_setup(), Some(&page));
    }

    #[test]
    fn frame_edit_restores_the_same_arc() {
        let mut doc = Document::new(128, 128, 72);
        let folder = doc.add_folder().unwrap();
        let mut h = History::default();
        let shape = crate::frame::FrameShape {
            panels: Vec::new(),
            border: crate::frame::BorderStyle { width: 1.0, color: [0; 4] },
        };
        let a = Frame::with_full_tiles(shape.clone(), 128, 128, &[TileCoord::new(0, 0)]);
        let b = Frame::with_full_tiles(shape, 128, 128, &[TileCoord::new(1, 1)]);
        doc.set_frame(folder, Some(a.clone()));
        let old = doc.set_frame(folder, Some(b.clone())).unwrap();
        h.push(Edit::Frame { layer: folder, frame: old }, &doc);

        assert_eq!(h.undo(&mut doc), [Touch::Structure]);
        assert!(Arc::ptr_eq(doc.frame(folder).unwrap(), &a));
        assert_eq!(h.redo(&mut doc), [Touch::Structure]);
        assert!(Arc::ptr_eq(doc.frame(folder).unwrap(), &b));
    }

    #[test]
    fn batch_undoes_last_to_first_and_redoes_first_to_last() {
        let mut doc = Document::new(256, 64, 72);
        let id = doc.active();
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        let (s0, s1, s2) = (doc.selection().clone(), selection_at(1), selection_at(2));
        // One step of three operations: select s1, paint, select s2. Each
        // recorded edit is the inverse of its operation, in operation order.
        let undo_1 = doc.swap_selection(s1.clone());
        let paint = stroke(&mut doc, &mut rec, 0, 77).unwrap();
        let undo_2 = doc.swap_selection(s2.clone());
        h.push(Edit::Batch(vec![Edit::Selection(Box::new(undo_1)), paint, Edit::Selection(Box::new(undo_2))]), &doc);

        assert_eq!(h.undo(&mut doc), [Touch::Selection, Touch::Pixels(id), Touch::Selection]);
        assert!(doc.selection().shares_storage(&s0), "the first operation is undone last");
        assert_eq!(pixel(&doc, 0)[0], 0);
        h.redo(&mut doc);
        assert!(doc.selection().shares_storage(&s2), "the last operation is redone last");
        assert_eq!(pixel(&doc, 0)[0], 77);
        h.undo(&mut doc);
        assert!(doc.selection().shares_storage(&s0));
        assert!(!s1.is_empty());
    }

    #[test]
    fn restore_puts_back_pre_stroke_tiles_and_keeps_one_edit() {
        let mut doc = Document::new(256, 64, 72);
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        // An existing tile (0,0) from an earlier stroke.
        h.push(stroke(&mut doc, &mut rec, 0, 100).unwrap(), &doc);
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
        h.push(edit, &doc);
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
        h.push(stroke(&mut doc, &mut rec, 0, 1).unwrap(), &doc);
        h.undo(&mut doc);
        assert!(h.can_redo());
        h.push(stroke(&mut doc, &mut rec, 0, 2).unwrap(), &doc);
        assert!(!h.can_redo());
    }

    #[test]
    fn structure_undo_restores_deleted_layer() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::default();
        let snap = doc.snapshot_structure();
        let added = doc.add_raster_layer().unwrap();
        h.push(Edit::Structure(Box::new(snap)), &doc);
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
        h.push(Edit::Structure(Box::new(doc.snapshot_structure())), &doc);
        set_opacity(&mut doc, &mut h, 0.1, true);
        assert_eq!(h.undo_len(), 4);
    }

    #[test]
    fn limit_drops_oldest() {
        let mut doc = Document::new(64, 64, 72);
        let mut h = History::new(2);
        let mut rec = PixelRecorder::default();
        for v in 1..=3 {
            h.push(stroke(&mut doc, &mut rec, 0, v).unwrap(), &doc);
        }
        assert_eq!(h.undo_len(), 2);
        assert_eq!(h.usage().trimmed, 0, "a step-limit drop is not a budget trim");
    }

    // ----- memory budget ---------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::fill::{FillBlend, apply_fill};
    use crate::frame::{BorderStyle, FrameShape};
    use crate::selection::{erase_selected, full_mask};
    use crate::transform::{Filter, FloatSession};

    const MIB: usize = 1 << 20;
    const OVERHEAD: usize = size_of::<Step>();

    /// Write a distinct pixel into each tile of `coords` without recording.
    fn paint(doc: &mut Document, id: LayerId, coords: impl IntoIterator<Item = TileCoord>, v: u16) {
        let (grid, _) = doc.paint_target(id).unwrap();
        for c in coords {
            grid.get_mut_or_create(c)[1][2] = [v; 4];
        }
    }

    fn row(n: i32) -> impl Iterator<Item = TileCoord> {
        (0..n).map(|i| TileCoord::new(i % 10, i / 10))
    }

    /// Clear a layer as one step, the way the app does.
    fn clear(doc: &mut Document, h: &mut History, id: LayerId) {
        let grid = doc.layer(id).unwrap().raster().unwrap();
        let tiles: Vec<_> = grid.iter().map(|(c, t)| (c, Some(t.clone()))).collect();
        if tiles.is_empty() {
            return;
        }
        doc.clear_layer(id);
        h.push(Edit::Pixels { layer: id, tiles }, doc);
    }

    /// A structural edit as one step; false when it changed nothing.
    fn structure(doc: &mut Document, h: &mut History, f: impl FnOnce(&mut Document) -> bool) -> bool {
        let snap = doc.snapshot_structure();
        let changed = f(doc);
        if changed {
            h.push(Edit::Structure(Box::new(snap)), doc);
        }
        changed
    }

    /// Tiles charged to the newest step.
    fn last_tiles(h: &History) -> usize {
        h.undo.back().unwrap().tiles
    }

    fn counter(h: &mut History) -> Arc<AtomicUsize> {
        let n = Arc::new(AtomicUsize::new(0));
        let seen = n.clone();
        h.set_release(Box::new(move |_| {
            seen.fetch_add(1, Ordering::Relaxed);
        }));
        n
    }

    /// Every page tile partial, so a fill makes a distinct tile per coordinate.
    fn partial_region(w: u32, h: u32) -> Selection {
        let mut m = [[255u8; 64]; 64];
        m[0][0] = 128;
        let m = Arc::new(m);
        let mut s = Selection::new();
        for y in 0..h.div_ceil(64) as i32 {
            for x in 0..w.div_ceil(64) as i32 {
                s.insert_tile(TileCoord::new(x, y), m.clone());
            }
        }
        s
    }

    const INK: [u16; 4] = [1000, 2000, 3000, 1 << 15];

    #[test]
    fn undo_budget_tiers() {
        let gib = |g: f64| Some((g * (1u64 << 30) as f64) as u64);
        assert_eq!(undo_budget(None), 256 * MIB);
        assert_eq!(undo_budget(Some(0)), 256 * MIB);
        assert_eq!(undo_budget(gib(1.0)), 128 * MIB);
        assert_eq!(undo_budget(gib(3.8)), 256 * MIB);
        assert_eq!(undo_budget(gib(4.0)), 256 * MIB);
        assert_eq!(undo_budget(gib(6.0)), 384 * MIB);
        assert_eq!(undo_budget(gib(7.8)), 512 * MIB);
        assert_eq!(undo_budget(gib(8.0)), 512 * MIB);
        assert_eq!(undo_budget(gib(15.9)), 1024 * MIB);
        assert_eq!(undo_budget(gib(64.0)), 1024 * MIB);
    }

    #[test]
    fn stroke_costs_its_old_tiles() {
        let mut doc = Document::new(320, 64, 72);
        let id = doc.active();
        paint(&mut doc, id, row(3), 9);
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        // Bigger stroke first, so the recorder's buffer outgrows the next one.
        rec.begin(id);
        let (grid, _) = doc.paint_target(id).unwrap();
        for c in (0..8).map(|y| TileCoord::new(7, y)) {
            rec.before_write(grid, c);
            grid.get_mut_or_create(c)[0][0] = [5; 4];
        }
        h.push(rec.finish().unwrap(), &doc);

        // Three existing tiles and two new ones.
        rec.begin(id);
        let (grid, _) = doc.paint_target(id).unwrap();
        for c in row(5) {
            rec.before_write(grid, c);
            grid.get_mut_or_create(c)[0][0] = [7; 4];
        }
        let before = h.usage().undo_bytes;
        h.push(rec.finish().unwrap(), &doc);
        assert_eq!(size_of::<(TileCoord, Option<TileRef>)>(), 16);
        assert_eq!(h.undo.back().unwrap().bytes, 3 * TILE_BYTES + OVERHEAD + 5 * 16);
        assert_eq!(h.usage().undo_bytes - before, 3 * TILE_BYTES + OVERHEAD + 5 * 16);
    }

    #[test]
    fn budget_drops_oldest_keeps_newest() {
        let mut doc = Document::new(128, 128, 72);
        let id = doc.active();
        let layer = 4 * TILE_BYTES;
        let mut h = History::with_budget(200, layer * 5 / 2);
        let all = || (0..4).map(|i| TileCoord::new(i % 2, i / 2));
        for v in 1..=3 {
            paint(&mut doc, id, all(), v);
            clear(&mut doc, &mut h, id);
            assert!(h.usage().undo_bytes <= h.budget());
        }
        assert_eq!((h.undo_len(), h.usage().trimmed), (2, 1));
        for v in [3, 2] {
            h.undo(&mut doc);
            let grid = doc.layer(id).unwrap().raster().unwrap();
            assert_eq!(grid.len(), 4);
            assert!(all().all(|c| grid.get(c).unwrap()[1][2] == [v; 4]), "undo restores round {v}");
        }
        assert!(!h.can_undo());
    }

    #[test]
    fn oversized_step_is_kept_alone() {
        let mut doc = Document::new(64, 64, 72);
        let id = doc.active();
        let mut h = History::with_budget(200, 1);
        let mut rec = PixelRecorder::default();
        h.push(stroke(&mut doc, &mut rec, 0, 100).unwrap(), &doc);
        assert_eq!(h.undo_len(), 1, "the newest step stays even over budget");
        h.undo(&mut doc);
        assert_eq!(pixel(&doc, 0)[0], 0);
        h.redo(&mut doc);
        assert_eq!(pixel(&doc, 0)[0], 100);

        let mut p = doc.layer(id).unwrap().props.clone();
        p.opacity = 0.5;
        let before = doc.set_props(id, p).unwrap();
        h.push_props(id, before, false);
        assert_eq!((h.undo_len(), h.usage().trimmed), (2, 0), "a props push leaves the budget to the next push");
        h.push(stroke(&mut doc, &mut rec, 1, 50).unwrap(), &doc);
        assert_eq!((h.undo_len(), h.usage().trimmed), (1, 2), "the stroke and the props step are dropped");
        h.undo(&mut doc);
        assert_eq!(pixel(&doc, 1)[0], 0);
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_redo_never_trim() {
        let mut doc = Document::new(256, 256, 72);
        let id = doc.active();
        let mut h = History::with_budget(200, 4 * TILE_BYTES);
        let freed = counter(&mut h);
        let mut rec = PixelRecorder::default();
        h.push(stroke(&mut doc, &mut rec, 0, 100).unwrap(), &doc);
        clear(&mut doc, &mut h, id);
        let edit = apply_fill(&mut doc, id, &partial_region(256, 256), INK, 1.0, FillBlend::Normal).unwrap();
        h.push(edit, &doc);
        assert!(h.usage().undo_bytes < 4 * TILE_BYTES, "filling empty tiles costs only overhead");
        assert_eq!(h.undo_len(), 3);

        h.undo(&mut doc);
        h.undo(&mut doc);
        let u = h.usage();
        assert!(u.redo_bytes >= 16 * TILE_BYTES, "the redo of the fill holds its 16 tiles");
        assert!(u.redo_bytes > u.budget, "redo is never trimmed");
        assert_eq!((u.undo_steps + u.redo_steps, u.trimmed), (3, 0));
        h.redo(&mut doc);
        h.redo(&mut doc);
        assert_eq!((h.undo_len(), h.redo_len(), h.usage().trimmed), (3, 0, 0));
        h.undo(&mut doc);
        assert_eq!(freed.load(Ordering::Relaxed), 0);

        h.push(stroke(&mut doc, &mut rec, 1, 50).unwrap(), &doc);
        assert_eq!((h.usage().redo_bytes, h.redo_len()), (0, 0));
        assert_eq!(freed.load(Ordering::Relaxed), 1, "the cleared redo step reaches the hook");

        // A redo that takes the undo stack over budget drops nothing either.
        let mut doc = Document::new(256, 256, 72);
        let id = doc.active();
        let mut h = History::with_budget(200, 64 * TILE_BYTES);
        let freed = counter(&mut h);
        paint(&mut doc, id, (0..16).map(|i| TileCoord::new(i % 4, i / 4)), 7);
        h.push(stroke(&mut doc, &mut rec, 0, 100).unwrap(), &doc);
        let edit = apply_fill(&mut doc, id, &partial_region(256, 256), INK, 1.0, FillBlend::Normal).unwrap();
        h.push(edit, &doc);
        h.set_budget(4 * TILE_BYTES);
        h.undo(&mut doc);
        h.redo(&mut doc);
        let u = h.usage();
        assert!(u.undo_bytes >= 16 * TILE_BYTES, "the redone fill holds the 16 painted tiles");
        assert_eq!((u.undo_steps, u.trimmed, freed.load(Ordering::Relaxed)), (2, 0, 0), "redo is never trimmed");
    }

    #[test]
    fn shared_tiles_cost_nothing() {
        for scan in [false, true] {
            let mut doc = Document::new(640, 640, 72);
            let base = doc.active();
            paint(&mut doc, base, row(50), 1);
            let mut h = History::default();
            if scan {
                h.scan_always();
            }
            let mut top = base;
            assert!(structure(&mut doc, &mut h, |d| {
                top = d.add_raster_layer().unwrap();
                true
            }));
            assert_eq!(last_tiles(&h), 0, "an add keeps every tile in the document");
            paint(&mut doc, top, row(50), 2);
            assert!(structure(&mut doc, &mut h, |d| d.delete_layer(top)));
            assert_eq!(last_tiles(&h), 50, "a deleted layer's tiles");

            // Upper on 0..20, lower on 10..30.
            let lower = doc.add_raster_layer().unwrap();
            paint(&mut doc, lower, row(30).skip(10), 3);
            let upper = doc.add_raster_layer().unwrap();
            paint(&mut doc, upper, row(20), 4);
            assert!(structure(&mut doc, &mut h, |d| d.merge_down(upper)));
            assert_eq!(last_tiles(&h), 30, "the upper layer plus the 10 lower tiles it covered (scan {scan})");
        }
    }

    #[test]
    fn twin_layers_share() {
        let mut doc = Document::new(640, 64, 72);
        let a = doc.active();
        paint(&mut doc, a, row(10), 1);
        let mut h = History::default();
        let mut b = a;
        structure(&mut doc, &mut h, |d| {
            b = d.duplicate_layer(a).unwrap();
            true
        });
        assert!(structure(&mut doc, &mut h, |d| d.delete_layer(b)));
        assert_eq!(last_tiles(&h), 0, "an unwritten copy shares the original's map");

        for scan in [true, false] {
            let mut doc = Document::new(640, 64, 72);
            let a = doc.active();
            paint(&mut doc, a, row(10), 1);
            let mut h = History::default();
            if scan {
                h.scan_always();
            }
            let mut b = a;
            structure(&mut doc, &mut h, |d| {
                b = d.duplicate_layer(a).unwrap();
                true
            });
            clear(&mut doc, &mut h, a);
            if scan {
                assert_eq!(last_tiles(&h), 0, "the copy still holds the cleared tiles");
            } else {
                assert_eq!(last_tiles(&h), 10, "under budget the scan is skipped: overcount");
            }
            assert!(structure(&mut doc, &mut h, |d| d.delete_layer(b)));
            if scan {
                assert_eq!(last_tiles(&h), 10, "deleting the copy frees them");
            }
        }
    }

    #[test]
    fn overcount_never_trims() {
        let mut doc = Document::new(256, 256, 72);
        let a = doc.active();
        let all = || (0..16).map(|i| TileCoord::new(i % 4, i / 4));
        let mut h = History::with_budget(200, 16 * TILE_BYTES * 5 / 2);
        paint(&mut doc, a, all(), 1);
        // Back the layer up, then clear the original: the copy holds its tiles.
        structure(&mut doc, &mut h, |d| d.duplicate_layer(a).is_some());
        clear(&mut doc, &mut h, a);
        assert_eq!(last_tiles(&h), 16, "under budget the scan is skipped: overcount");
        for v in 2..=3 {
            paint(&mut doc, a, all(), v);
            clear(&mut doc, &mut h, a);
        }
        // Two layers only history holds, under 2.5: the overcount trims nothing.
        assert_eq!((h.undo_len(), h.usage().trimmed), (4, 0));
        assert_eq!(h.undo.iter().map(|s| s.tiles).collect::<Vec<_>>(), [0, 0, 16, 16]);
        assert!(h.usage().undo_bytes <= h.budget());
        // A third does trim, oldest first.
        paint(&mut doc, a, all(), 4);
        clear(&mut doc, &mut h, a);
        assert_eq!((h.undo_len(), h.usage().trimmed), (2, 3));
        assert_eq!(h.usage().undo_bytes, h.undo.iter().map(|s| s.bytes).sum::<usize>());
    }

    #[test]
    fn recost_walks_only_maps_outside_the_document() {
        let mut doc = Document::new(256, 256, 72);
        let a = doc.active();
        paint(&mut doc, a, row(16), 1);
        let b = doc.add_raster_layer().unwrap();
        paint(&mut doc, b, row(16), 2);
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        doc.set_active(a);
        // A move keeps every map in the document; the stroke then copies
        // a's on write, so the move's snapshot holds a drifted map of a.
        assert!(structure(&mut doc, &mut h, |d| d.shift_layer(b, -1)));
        h.push(stroke(&mut doc, &mut rec, 0, 9).unwrap(), &doc);
        assert!(structure(&mut doc, &mut h, |d| d.delete_layer(b)));
        clear(&mut doc, &mut h, a);
        let walked: Vec<usize> = h.undo.iter().map(|s| s.walked.len()).collect();
        assert_eq!(walked, [0, 0, 1, 0], "only the deleted layer's map was outside the document");

        h.recost(&doc);
        check_stacks(&h, &doc, true, "re-cost");
        assert_eq!(h.undo.iter().map(|s| s.tiles).collect::<Vec<_>>(), [0, 1, 16, 16]);
    }

    #[test]
    fn shared_tables_the_document_left_are_charged() {
        let mut doc = Document::new(256, 256, 72);
        let a = doc.active();
        paint(&mut doc, a, row(16), 1);
        let b = doc.add_raster_layer().unwrap();
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        doc.set_active(a);
        // Each move's snapshot shares a's map; the stroke copies it on write
        // and its undo keeps the copy, so only the snapshot holds the old one.
        for delta in [-1, 1, -1] {
            assert!(structure(&mut doc, &mut h, |d| d.shift_layer(b, delta)));
            h.push(stroke(&mut doc, &mut rec, 0, 9).unwrap(), &doc);
            h.undo(&mut doc);
        }
        let old_table = |s: &Step| match &s.edit {
            Edit::Structure(snap) => snap.layers[&a].raster().unwrap().map_bytes(),
            _ => 0,
        };
        let tables = |h: &History| h.undo.iter().map(|s| s.tables).collect::<Vec<_>>();
        let want: Vec<usize> = h.undo.iter().map(old_table).collect();
        assert!(want.len() == 3 && want.iter().all(|&t| t > 0));
        assert_eq!(tables(&h), want, "every old table of a, once");
        let bytes = |h: &History| h.undo.iter().map(|s| s.bytes).sum::<usize>();
        assert_eq!(h.usage().undo_bytes, bytes(&h));

        // Undoing the last move puts its map back in the document.
        h.undo(&mut doc);
        h.push(stroke(&mut doc, &mut rec, 1, 9).unwrap(), &doc);
        assert_eq!(tables(&h), [want[0], want[1], 0]);
        check_stacks(&h, &doc, false, "after undo");
    }

    #[test]
    fn push_that_recosts_walks_the_document_once() {
        let mut doc = Document::new(640, 64, 72);
        let mut h = History::default();
        let mut rec = PixelRecorder::default();
        for v in 1..=3 {
            h.push(stroke(&mut doc, &mut rec, 0, v).unwrap(), &doc);
        }
        assert_eq!(h.doc_walks, 0, "under budget the scan is skipped");
        h.set_budget(h.usage().undo_bytes);
        h.push(stroke(&mut doc, &mut rec, 0, 4).unwrap(), &doc);
        let trimmed = h.usage().trimmed;
        assert_eq!(h.doc_walks, 1, "the re-cost scans; the step does not");
        assert!(trimmed > 0);
        check_stacks(&h, &doc, true, "after the trim");
        // Every step exact now: the next push at budget scans by itself.
        h.set_budget(h.usage().undo_bytes);
        h.push(stroke(&mut doc, &mut rec, 0, 5).unwrap(), &doc);
        assert!(h.undo.iter().all(|s| s.exact));
        assert_eq!(h.doc_walks, 2);
        assert!(h.usage().trimmed > trimmed);
    }

    #[test]
    fn solid_shared_tile_counts_once() {
        let mut doc = Document::new(512, 512, 72);
        let id = doc.active();
        let mut h = History::default();
        let edit = apply_fill(&mut doc, id, &Selection::all(512, 512), INK, 1.0, FillBlend::Normal).unwrap();
        h.push(edit, &doc);
        let grid = doc.layer(id).unwrap().raster().unwrap();
        let first = grid.get_ref(TileCoord::new(0, 0)).unwrap();
        assert_eq!(grid.len(), 64);
        assert!(grid.iter().all(|(_, t)| Arc::ptr_eq(t, first)), "one solid tile shared by every coordinate");
        clear(&mut doc, &mut h, id);
        assert_eq!(last_tiles(&h), 1);
        assert_eq!(h.undo.back().unwrap().bytes, OVERHEAD + 64 * 16 + TILE_BYTES);
    }

    #[test]
    fn shift_tiles_transform_charges_only_departed_tiles() {
        let mut doc = Document::new(256, 256, 72);
        let id = doc.active();
        let all: Vec<TileCoord> = (0..16).map(|i| TileCoord::new(i % 4, i / 4)).collect();
        paint(&mut doc, id, all.iter().copied(), 9);
        let mut h = History::default();
        h.scan_always();
        let shift = |doc: &mut Document, h: &mut History, dx: f64| {
            let mut s = FloatSession::begin(doc, id).unwrap();
            let mut p = s.params();
            p.t = [dx, 0.0];
            s.set_params(p);
            let edit = s.commit(doc, Filter::Bicubic).unwrap();
            let Edit::Pixels { ref tiles, .. } = edit else { panic!("a layer transform is a pixel edit") };
            assert_eq!(tiles.iter().filter(|(_, t)| t.is_some()).count(), 16, "every old slot changed");
            h.push(edit, doc);
        };
        // Columns 0..4 move to 1..5: every tile is still in the layer.
        shift(&mut doc, &mut h, 64.0);
        assert_eq!(last_tiles(&h), 0, "moved tiles stay in the document");
        // Columns 1..5 move to 5..9, and the layer keeps at most one page of
        // margin (columns -4..8): column 4's tiles leave it.
        shift(&mut doc, &mut h, 256.0);
        assert_eq!(last_tiles(&h), 4, "only the tiles pushed past the margin left the layer");
        assert_eq!(doc.layer(id).unwrap().raster().unwrap().len(), 12);
    }

    #[test]
    fn selection_and_frame_costs() {
        let mut doc = Document::new(512, 128, 72);
        let mut h = History::default();
        let partial = || {
            let mut m = [[0u8; 64]; 64];
            m[3][3] = 200;
            Arc::new(m)
        };
        let mut s = Selection::new();
        s.insert_tile(TileCoord::new(0, 0), partial());
        s.insert_tile(TileCoord::new(1, 0), partial());
        for x in 2..5 {
            s.insert_tile(TileCoord::new(x, 0), full_mask().clone());
        }
        doc.swap_selection(s.clone());

        // Add keeps every old mask Arc at its coordinate: only the map is new.
        let mut added = s.clone();
        let mut shape = Selection::new();
        shape.insert_tile(TileCoord::new(6, 1), full_mask().clone());
        added.combine(&shape, crate::selection::SelectOp::Add);
        let old = doc.swap_selection(added);
        let map = old.map_bytes();
        h.push(Edit::Selection(Box::new(old)), &doc);
        assert_eq!(h.undo.back().unwrap().bytes, OVERHEAD + map);

        // Deselect: the two partial masks are charged, the full ones never.
        let old = doc.swap_selection(Selection::new());
        let map = old.map_bytes();
        h.push(Edit::Selection(Box::new(old)), &doc);
        assert_eq!(h.undo.back().unwrap().bytes, OVERHEAD + map + 2 * MASK_BYTES);

        let shape = FrameShape { panels: Vec::new(), border: BorderStyle { width: 1.0, color: [0; 4] } };
        let frame = |c| Frame::with_full_tiles(shape.clone(), 512, 128, &[TileCoord::new(c, 0)]);
        let folder = doc.add_folder().unwrap();
        let (a, b, c) = (frame(0), frame(1), frame(2));
        doc.set_frame(folder, Some(a.clone()));
        let old = doc.set_frame(folder, Some(b.clone())).unwrap();
        h.push(Edit::Frame { layer: folder, frame: old }, &doc);
        assert_eq!(h.undo.back().unwrap().bytes, OVERHEAD + a.heap_bytes() + ARC_COUNTS);

        doc.duplicate_layer(folder).unwrap();
        let old = doc.set_frame(folder, Some(c)).unwrap();
        h.push(Edit::Frame { layer: folder, frame: old }, &doc);
        assert_eq!(h.undo.back().unwrap().bytes, OVERHEAD, "the duplicated folder still holds the old frame");
    }

    #[test]
    fn steps_that_fit() {
        let h = History::default();
        assert_eq!(h.steps_that_fit(92 * MIB), 2);
        assert_eq!(h.steps_that_fit(401 * MIB), 1);
        assert_eq!(h.steps_that_fit(0), DEFAULT_LIMIT);
        assert_eq!(h.steps_that_fit(1), DEFAULT_LIMIT);
    }

    #[test]
    fn release_hook_receives_dropped_steps() {
        let mut doc = Document::new(128, 128, 72);
        let id = doc.active();
        let mut h = History::with_budget(3, 4 * TILE_BYTES * 5 / 2);
        let freed = counter(&mut h);
        let freed = || freed.load(Ordering::Relaxed);
        let all = || (0..4).map(|i| TileCoord::new(i % 2, i / 2));
        for v in 1..=3 {
            paint(&mut doc, id, all(), v);
            clear(&mut doc, &mut h, id);
        }
        assert_eq!((freed(), h.usage().trimmed), (1, 1), "budget trim");

        let mut rec = PixelRecorder::default();
        h.push(stroke(&mut doc, &mut rec, 0, 1).unwrap(), &doc);
        h.push(stroke(&mut doc, &mut rec, 0, 2).unwrap(), &doc);
        assert_eq!((h.undo_len(), freed(), h.usage().trimmed), (3, 2, 1), "step-limit drop");

        h.undo(&mut doc);
        h.undo(&mut doc);
        h.push(stroke(&mut doc, &mut rec, 1, 3).unwrap(), &doc);
        assert_eq!(freed(), 4, "cleared redo");

        let left = h.undo_len();
        h.clear();
        assert_eq!(freed(), 4 + left, "clear");
        assert_eq!(h.usage(), HistoryUsage { budget: h.budget(), limit: 3, ..zero_usage() });
    }

    fn zero_usage() -> HistoryUsage {
        HistoryUsage { undo_steps: 0, redo_steps: 0, undo_bytes: 0, redo_bytes: 0, budget: 0, limit: 0, trimmed: 0 }
    }

    // ----- oracle ----------------------------------------------------------

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }

        fn coord(&mut self) -> TileCoord {
            TileCoord::new(self.below(4) as i32, self.below(4) as i32)
        }
    }

    fn ptr(t: &TileRef) -> usize {
        Arc::as_ptr(t) as usize
    }

    fn held(e: &Edit, out: &mut AHashSet<usize>) {
        match e {
            Edit::Pixels { tiles, .. } => out.extend(tiles.iter().filter_map(|(_, t)| t.as_ref()).map(ptr)),
            Edit::Structure(s) => {
                for g in s.layers.values().filter_map(Layer::raster) {
                    out.extend(g.iter().map(|(_, t)| ptr(t)));
                }
            }
            Edit::Batch(v) => v.iter().for_each(|e| held(e, out)),
            _ => {}
        }
    }

    /// Distinct tiles the steps hold that no document raster holds.
    fn oracle<'a>(steps: impl Iterator<Item = &'a Step>, doc: &Document) -> usize {
        let mut set = AHashSet::new();
        for s in steps {
            held(&s.edit, &mut set);
        }
        for g in doc.layers.values().filter_map(Layer::raster) {
            for (_, t) in g.iter() {
                set.remove(&ptr(t));
            }
        }
        set.len()
    }

    fn sorted(doc: &Document, folders: bool) -> Vec<LayerId> {
        let mut v: Vec<LayerId> = doc.layers.values().filter(|l| l.is_folder() == folders).map(|l| l.id).collect();
        v.sort_unstable();
        v
    }

    fn random_selection(rng: &mut Rng) -> Selection {
        let mut s = Selection::new();
        match rng.below(4) {
            0 => {}
            1 => s = Selection::all(256, 256),
            _ => {
                for _ in 0..1 + rng.below(5) {
                    if rng.below(2) == 0 {
                        s.insert_tile(rng.coord(), full_mask().clone());
                    } else {
                        let mut m = [[0u8; 64]; 64];
                        for _ in 0..8 {
                            m[rng.below(64)][rng.below(64)] = 1 + rng.below(254) as u8;
                        }
                        s.insert_tile(rng.coord(), Arc::new(m));
                    }
                }
            }
        }
        s
    }

    /// One random op on `doc`.
    fn random_op(
        rng: &mut Rng,
        doc: &mut Document,
        h: &mut History,
        rec: &mut PixelRecorder,
        snaps: &mut Vec<Document>,
    ) {
        let rasters = sorted(doc, false);
        let folders = sorted(doc, true);
        let all: Vec<LayerId> = rasters.iter().chain(&folders).copied().collect();
        let raster = rasters[rng.below(rasters.len())];
        let any = all[rng.below(all.len())];
        let roomy = doc.layer_count() < 8;
        match rng.below(22) {
            0 | 1 => {
                rec.begin(raster);
                let (grid, _) = doc.paint_target(raster).unwrap();
                for _ in 0..1 + rng.below(4) {
                    let c = rng.coord();
                    rec.before_write(grid, c);
                    grid.get_mut_or_create(c)[rng.below(64)][rng.below(64)] = [1 + rng.below(30000) as u16; 4];
                }
                let e = rec.finish().unwrap();
                h.push(e, doc);
            }
            2 | 3 => {
                let region = if rng.below(2) == 0 { Selection::all(256, 256) } else { random_selection(rng) };
                let color = [rng.below(1000) as u16, 0, 0, 1000 + rng.below(30000) as u16];
                if let Some(e) = apply_fill(doc, raster, &region, color, 1.0, FillBlend::Normal) {
                    h.push(e, doc);
                }
            }
            4 => {
                if let Some(e) = erase_selected(doc, raster) {
                    h.push(e, doc);
                }
            }
            5 => clear(doc, h, raster),
            6 if roomy => {
                structure(doc, h, |d| d.add_raster_layer().is_some());
            }
            7 if roomy => {
                structure(doc, h, |d| d.duplicate_layer(any).is_some());
            }
            8 => {
                structure(doc, h, |d| d.delete_layer(any));
            }
            9 => {
                let parent = (!folders.is_empty() && rng.below(2) == 0).then(|| folders[rng.below(folders.len())]);
                let index = rng.below(4);
                structure(doc, h, |d| d.move_layer(any, parent, index));
            }
            10 => {
                structure(doc, h, |d| d.merge_down(raster));
            }
            11 if roomy => {
                structure(doc, h, |d| d.add_folder().is_some());
            }
            12 => {
                let s = random_selection(rng);
                if !doc.selection().shares_storage(&s) {
                    let old = doc.swap_selection(s);
                    h.push(Edit::Selection(Box::new(old)), doc);
                }
            }
            13 if !folders.is_empty() => {
                let folder = folders[rng.below(folders.len())];
                let shape = FrameShape { panels: Vec::new(), border: BorderStyle { width: 1.0, color: [0; 4] } };
                let at = [rng.coord(), rng.coord()];
                let f = (rng.below(3) != 0).then(|| Frame::with_full_tiles(shape, 256, 256, &at));
                if let Some(old) = doc.set_frame(folder, f) {
                    h.push(Edit::Frame { layer: folder, frame: old }, doc);
                }
            }
            14 | 15 => {
                if let Ok(mut s) = FloatSession::begin(doc, raster) {
                    let mut p = s.params();
                    p.t = [64.0 * (rng.below(3) as f64 - 1.0), 64.0 * (rng.below(3) as f64 - 1.0)];
                    if rng.below(3) == 0 {
                        p.t[0] += 10.5;
                    }
                    if rng.below(4) == 0 {
                        p.theta = 0.3;
                    }
                    if rng.below(2) == 0 {
                        s.preview(doc, p, Filter::Nearest);
                    }
                    s.set_params(p);
                    if rng.below(3) == 0 {
                        s.cancel(doc);
                    } else if let Some(e) = s.commit(doc, Filter::Nearest) {
                        h.push(e, doc);
                    }
                }
            }
            16..=18 => {
                h.undo(doc);
            }
            19 | 20 => {
                h.redo(doc);
            }
            _ => {
                if rng.below(2) == 0 {
                    snaps.push(doc.snapshot());
                } else {
                    snaps.clear();
                }
            }
        }
    }

    fn check_stacks(h: &History, doc: &Document, exact: bool, what: &str) {
        let charged = |s: &mut dyn Iterator<Item = &Step>| s.map(|s| s.tiles).sum::<usize>();
        let (cu, cr) = (charged(&mut h.undo.iter()), charged(&mut h.redo.iter()));
        let (ou, or) = (oracle(h.undo.iter(), doc), oracle(h.redo.iter(), doc));
        if exact {
            assert_eq!((cu, cr), (ou, or), "{what}: charged == history-only tiles");
        } else {
            assert!(cu >= ou && cr >= or, "{what}: charged ({cu}, {cr}) < history-only ({ou}, {or})");
        }
        assert_eq!(h.undo_bytes, h.undo.iter().map(|s| s.bytes).sum::<usize>(), "{what}: undo total");
        assert_eq!(h.redo_bytes, h.redo.iter().map(|s| s.bytes).sum::<usize>(), "{what}: redo total");
    }

    #[test]
    fn oracle_random_ops() {
        // (scan always, limit, budget)
        let configs = [(false, 200, 6 * TILE_BYTES), (true, 200, 6 * TILE_BYTES), (false, 40, 256 * MIB)];
        for seed in [0x9E37_79B9_7F4A_7C15u64, 1, 0xDEAD_BEEF, 42] {
            for (scan, limit, budget) in configs {
                let mut rng = Rng(seed);
                let mut doc = Document::new(256, 256, 72);
                let mut h = History::with_budget(limit, budget);
                if scan {
                    h.scan_always();
                }
                let mut rec = PixelRecorder::default();
                let mut snaps = Vec::new();
                let (mut pushes, mut trimmed) = (0, 0);
                for i in 0..1500 {
                    let steps = h.undo_len() + h.redo_len();
                    random_op(&mut rng, &mut doc, &mut h, &mut rec, &mut snaps);
                    let what = format!("seed {seed:#x} scan {scan} op {i}");
                    if h.pushes == pushes {
                        // Undo and redo never trim.
                        assert_eq!((h.undo_len() + h.redo_len(), h.usage().trimmed), (steps, trimmed), "{what}");
                    }
                    // After a trim the costs are exact, so the trim was
                    // decided on what only history holds.
                    let trim = h.usage().trimmed != trimmed;
                    trimmed = h.usage().trimmed;
                    check_stacks(&h, &doc, scan || trim, &what);
                    if h.pushes != pushes {
                        pushes = h.pushes;
                        assert!(h.undo_bytes <= h.budget || h.undo_len() == 1, "{what}: over budget after a push");
                    }
                }
                let trims = h.usage().trimmed > 0 || budget > MIB;
                assert!(pushes > 500 && trims, "seed {seed:#x}: the ops push and trim");
            }
        }
    }

    /// The usage after every op of a fixed script; `others` adds holders
    /// that must not change any cost.
    fn scripted(others: bool) -> Vec<HistoryUsage> {
        let mut doc = Document::new(256, 256, 72);
        let mut h = History::with_budget(200, 20 * TILE_BYTES);
        let mut rec = PixelRecorder::default();
        let mut rng = Rng(7);
        let mut out = Vec::new();
        let mut pins: Vec<Document> = Vec::new();
        let mut threads = Vec::new();
        for _ in 0..300 {
            if others {
                pins.push(doc.snapshot());
                let rasters = doc.layers.values().filter_map(Layer::raster);
                let tiles: Vec<TileRef> = rasters.flat_map(|g| g.iter().map(|(_, t)| t.clone())).collect();
                let snap = doc.snapshot();
                threads.push(std::thread::spawn(move || {
                    let more = tiles.clone();
                    drop(snap);
                    drop((tiles, more));
                }));
                if pins.len() > 5 {
                    pins.remove(0);
                }
            }
            // The same ops in the same order: the rng drives only the op.
            random_op(&mut rng, &mut doc, &mut h, &mut rec, &mut Vec::new());
            out.push(h.usage());
        }
        threads.into_iter().for_each(|t| t.join().unwrap());
        out
    }

    #[test]
    fn independent_of_other_holders() {
        let alone = scripted(false);
        assert!(alone.last().unwrap().trimmed > 0, "the script trims");
        assert_eq!(alone, scripted(true));
    }
}
