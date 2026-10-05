//! The pixel selection: a sparse 8-bit mask over the page, one 64×64 tile
//! at a time, with copy-on-write sharing like [`crate::TileGrid`].
//!
//! Canonical form:
//! - an all-0 tile is never stored;
//! - every all-255 tile is the one shared [`full_mask`] (`Arc::ptr_eq`);
//! - "no selection" is [`Selection::is_empty`], so subtracting down to
//!   nothing deselects (as in CSP).
//!
//! Only page tiles are meaningful. In edge tiles, pixels past the page are
//! "don't care": every consumer treats `x ≥ w` or `y ≥ h` as 0.

use std::sync::{Arc, LazyLock};

use ahash::AHashMap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::geom::TileRect;
use crate::history::{ARC_COUNTS, Edit, table_bytes};
use crate::layer::LayerId;
use crate::tile::{TILE_SIZE, TileCoord, TileRef, is_tile_empty};

/// `[y][x]` coverage, 0 (unselected) ..= 255 (selected). 4 KiB.
pub type MaskPixels = [[u8; TILE_SIZE]; TILE_SIZE];

/// Shared, copy-on-write mask tile.
pub type MaskRef = Arc<MaskPixels>;

static FULL: LazyLock<MaskRef> = LazyLock::new(|| Arc::new([[255; TILE_SIZE]; TILE_SIZE]));

/// The one all-255 tile every fully selected tile shares.
pub fn full_mask() -> &'static MaskRef {
    &FULL
}

/// How one tile of a selection looks.
#[derive(Debug, Clone, Copy)]
pub enum MaskView<'a> {
    /// Nothing selected.
    Empty,
    /// Everything selected.
    Full,
    Partial(&'a MaskPixels),
}

/// How a new shape combines with the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SelectOp {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

#[derive(Clone, Default)]
pub struct Selection {
    tiles: Arc<AHashMap<TileCoord, MaskRef>>,
    /// Conservative tile bbox: grows on insert, is not shrunk on remove.
    bounds: Option<TileRect>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn get(&self, c: TileCoord) -> MaskView<'_> {
        match self.tiles.get(&c) {
            None => MaskView::Empty,
            Some(t) if Arc::ptr_eq(t, full_mask()) => MaskView::Full,
            Some(t) => MaskView::Partial(t),
        }
    }

    /// Coverage of document pixel `(x, y)`.
    pub fn value(&self, x: i32, y: i32) -> u8 {
        let c = TileCoord::from_pixel(x, y);
        let (ox, oy) = c.origin();
        match self.get(c) {
            MaskView::Empty => 0,
            MaskView::Full => 255,
            MaskView::Partial(m) => m[(y - oy) as usize][(x - ox) as usize],
        }
    }

    /// Store a tile in canonical form: an all-0 mask removes the tile, an
    /// all-255 one is replaced by [`full_mask`]. Returns the previous tile.
    pub fn insert_tile(&mut self, c: TileCoord, m: MaskRef) -> Option<MaskRef> {
        let m = if Arc::ptr_eq(&m, full_mask()) {
            m
        } else {
            let flat = m.as_flattened();
            if flat.iter().all(|&v| v == 0) {
                return self.remove_tile(c);
            }
            if flat.iter().all(|&v| v == 255) { full_mask().clone() } else { m }
        };
        let one = TileRect { x0: c.x, y0: c.y, x1: c.x + 1, y1: c.y + 1 };
        self.bounds = Some(self.bounds.map_or(one, |b| b.union(one)));
        Arc::make_mut(&mut self.tiles).insert(c, m)
    }

    /// Remove a tile, returning it. Removing a missing tile keeps the map
    /// shared. The bounds are not shrunk.
    pub fn remove_tile(&mut self, c: TileCoord) -> Option<MaskRef> {
        if !self.tiles.contains_key(&c) {
            return None;
        }
        Arc::make_mut(&mut self.tiles).remove(&c)
    }

    /// Stored tiles, in no particular order.
    pub fn tiles(&self) -> impl Iterator<Item = (TileCoord, &MaskRef)> {
        self.tiles.iter().map(|(c, t)| (*c, t))
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Conservative bounding box of the stored tiles (`None` when nothing
    /// was ever inserted).
    pub fn bounds(&self) -> Option<TileRect> {
        self.bounds
    }

    /// True when both share one tile map (neither written since a clone).
    pub fn shares_storage(&self, other: &Selection) -> bool {
        Arc::ptr_eq(&self.tiles, &other.tiles)
    }

    /// The stored tile at `c` (the shared [`full_mask`] for a full one).
    pub(crate) fn tile_ref(&self, c: TileCoord) -> Option<&MaskRef> {
        self.tiles.get(&c)
    }

    /// Identity of the tile map: equal exactly when [`Self::shares_storage`].
    pub(crate) fn map_ptr(&self) -> usize {
        Arc::as_ptr(&self.tiles) as usize
    }

    /// Heap bytes of the tile map itself (not the masks).
    pub(crate) fn map_bytes(&self) -> usize {
        table_bytes(self.tiles.capacity(), size_of::<(TileCoord, MaskRef)>())
            + ARC_COUNTS
            + size_of::<AHashMap<TileCoord, MaskRef>>()
    }

    /// Mask bytes held by this selection: partial tiles only, since every
    /// full tile shares [`full_mask`].
    pub fn byte_size(&self) -> usize {
        let partial = self.tiles.values().filter(|t| !Arc::ptr_eq(t, full_mask())).count();
        partial * std::mem::size_of::<MaskPixels>()
    }

    /// Every page tile of a `w`×`h` page selected.
    pub fn all(w: u32, h: u32) -> Selection {
        let (tw, th) = page_tiles(w, h);
        if tw == 0 || th == 0 {
            return Selection::default();
        }
        let mut map = AHashMap::with_capacity((tw * th) as usize);
        for ty in 0..th {
            for tx in 0..tw {
                map.insert(TileCoord::new(tx, ty), full_mask().clone());
            }
        }
        Selection { tiles: Arc::new(map), bounds: Some(TileRect { x0: 0, y0: 0, x1: tw, y1: th }) }
    }

    /// The complement within a `w`×`h` page: absent tiles become full, full
    /// tiles are removed and partial ones become `255 − v`.
    pub fn inverted(&self, w: u32, h: u32) -> Selection {
        let (tw, th) = page_tiles(w, h);
        let mut map = AHashMap::with_capacity((tw * th) as usize);
        let mut partial = Vec::new();
        for ty in 0..th {
            for tx in 0..tw {
                let c = TileCoord::new(tx, ty);
                match self.tiles.get(&c) {
                    None => {
                        map.insert(c, full_mask().clone());
                    }
                    Some(m) if is_full(m) => {}
                    Some(m) => partial.push((c, m.clone())),
                }
            }
        }
        let mut out = Selection { tiles: Arc::new(map), bounds: None };
        for (c, m) in par_map(&partial, |(c, m)| (*c, map_tile(m, |v| 255 - v))) {
            out.set(c, m);
        }
        // `255 − 0` past the page: the inverse of a shape covering an edge
        // tile's page pixels must not keep it.
        out.clip_to_page(w, h);
        out.bounds = out.tight_bounds();
        out
    }

    /// Canonical form relative to a `w`×`h` page: in the edge tiles, decide
    /// on the page pixels only. A tile with none selected is removed, one
    /// with all of them fully selected becomes [`full_mask`] (as
    /// [`Selection::all`] stores it), and any other has its pixels past the
    /// page cleared. Visits only the edge tiles.
    ///
    /// Inverting and subtracting turn the 0 the rasterizer leaves past the
    /// page into 255; without this, a selection could hold such tiles and
    /// no page pixel at all (`is_empty()` false, nothing selected).
    pub fn clip_to_page(&mut self, w: u32, h: u32) {
        let (tw, th) = page_tiles(w, h);
        if tw == 0 || th == 0 {
            return;
        }
        let t = TILE_SIZE as i32;
        let (lw_last, lh_last) = (w as i32 - (tw - 1) * t, h as i32 - (th - 1) * t);
        let mut edge = Vec::new();
        if lw_last < t {
            edge.extend((0..th).map(|ty| TileCoord::new(tw - 1, ty)));
        }
        if lh_last < t {
            edge.extend((0..tw - i32::from(lw_last < t)).map(|tx| TileCoord::new(tx, th - 1)));
        }
        for c in edge {
            let Some(m) = self.tiles.get(&c).filter(|m| !is_full(m)).cloned() else { continue };
            let lw = if c.x == tw - 1 { lw_last as usize } else { TILE_SIZE };
            let lh = if c.y == th - 1 { lh_last as usize } else { TILE_SIZE };
            let page = || m.iter().take(lh).flat_map(|row| &row[..lw]);
            let off_page =
                m.iter().enumerate().any(|(y, row)| row.iter().enumerate().any(|(x, &v)| v != 0 && (x >= lw || y >= lh)));
            if page().all(|&v| v == 0) {
                self.remove_tile(c);
            } else if page().all(|&v| v == 255) {
                self.put(c, full_mask().clone());
            } else if off_page {
                let mut out = *m;
                for (y, row) in out.iter_mut().enumerate() {
                    let from = if y < lh { lw } else { 0 };
                    row[from..].fill(0);
                }
                self.put(c, Arc::new(out));
            }
        }
    }

    /// Combine `shape` into this selection, per pixel: Add is `max`,
    /// Subtract `min(m, 255 − s)` and Intersect `min(m, s)` (so tiles the
    /// shape lacks are dropped). Full and absent tiles take fast paths.
    pub fn combine(&mut self, shape: &Selection, op: SelectOp) {
        match op {
            SelectOp::Replace => *self = shape.clone(),
            SelectOp::Add => self.add(shape),
            SelectOp::Subtract => self.subtract(shape),
            SelectOp::Intersect => self.intersect(shape),
        }
    }

    fn add(&mut self, shape: &Selection) {
        if self.is_empty() {
            *self = shape.clone();
            return;
        }
        let mut direct = Vec::new();
        let mut jobs = Vec::new();
        for (c, s) in shape.tiles() {
            match self.tiles.get(&c) {
                None => direct.push((c, s.clone())),
                Some(m) if is_full(m) => {}
                Some(_) if is_full(s) => direct.push((c, s.clone())),
                Some(m) => jobs.push((c, m.clone(), s.clone())),
            }
        }
        for (c, m) in direct {
            self.put(c, m);
        }
        for (c, m) in par_map(&jobs, |(c, m, s)| (*c, zip_tiles(m, s, u8::max))) {
            self.set(c, m);
        }
    }

    fn subtract(&mut self, shape: &Selection) {
        let mut removed = Vec::new();
        let mut jobs = Vec::new();
        for (c, s) in shape.tiles() {
            match self.tiles.get(&c) {
                None => {}
                Some(_) if is_full(s) => removed.push(c),
                Some(m) => jobs.push((c, m.clone(), s.clone())),
            }
        }
        for c in removed {
            self.remove_tile(c);
        }
        for (c, m) in par_map(&jobs, |(c, m, s)| (*c, zip_tiles(m, s, |a, b| a.min(255 - b)))) {
            self.set(c, m);
        }
    }

    fn intersect(&mut self, shape: &Selection) {
        let mut out = Selection::default();
        let mut jobs = Vec::new();
        for (c, m) in self.tiles() {
            match shape.tiles.get(&c) {
                None => {}
                Some(s) if is_full(s) => out.put(c, m.clone()),
                Some(s) if is_full(m) => out.put(c, s.clone()),
                Some(s) => jobs.push((c, m.clone(), s.clone())),
            }
        }
        for (c, m) in par_map(&jobs, |(c, m, s)| (*c, zip_tiles(m, s, u8::min))) {
            out.set(c, m);
        }
        *self = out;
    }

    /// Store a tile already in canonical form (never all-0; all-255 only as
    /// the shared [`full_mask`]), without the scan of [`Self::insert_tile`].
    pub(crate) fn put(&mut self, c: TileCoord, m: MaskRef) {
        debug_assert!(classify(&m) != Class::Empty, "all-0 tile stored");
        debug_assert!(classify(&m) != Class::Full || is_full(&m), "all-255 tile not shared");
        let one = TileRect { x0: c.x, y0: c.y, x1: c.x + 1, y1: c.y + 1 };
        self.bounds = Some(self.bounds.map_or(one, |b| b.union(one)));
        Arc::make_mut(&mut self.tiles).insert(c, m);
    }

    /// [`Self::put`] a canonical tile, or remove the tile (`None`).
    pub(crate) fn set(&mut self, c: TileCoord, m: Option<MaskRef>) {
        match m {
            Some(m) => self.put(c, m),
            None => {
                self.remove_tile(c);
            }
        }
    }

    /// The exact tile bbox of the stored tiles.
    fn tight_bounds(&self) -> Option<TileRect> {
        self.tiles.keys().map(|c| TileRect { x0: c.x, y0: c.y, x1: c.x + 1, y1: c.y + 1 }).reduce(TileRect::union)
    }
}

/// Page size in tiles.
pub(crate) fn page_tiles(w: u32, h: u32) -> (i32, i32) {
    (w.div_ceil(TILE_SIZE as u32) as i32, h.div_ceil(TILE_SIZE as u32) as i32)
}

/// What a mask tile holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Empty,
    Full,
    Partial,
}

pub(crate) fn classify(m: &MaskPixels) -> Class {
    let flat = m.as_flattened();
    match flat[0] {
        0 if flat.iter().all(|&v| v == 0) => Class::Empty,
        255 if flat.iter().all(|&v| v == 255) => Class::Full,
        _ => Class::Partial,
    }
}

#[inline]
pub(crate) fn is_full(m: &MaskRef) -> bool {
    Arc::ptr_eq(m, full_mask())
}

/// `m` in canonical form: `None` when all-0, the shared tile when all-255.
pub(crate) fn canonical(m: &MaskPixels) -> Option<MaskRef> {
    match classify(m) {
        Class::Empty => None,
        Class::Full => Some(full_mask().clone()),
        Class::Partial => Some(Arc::new(*m)),
    }
}

/// `f(v)` per pixel, canonical.
fn map_tile(m: &MaskPixels, f: impl Fn(u8) -> u8) -> Option<MaskRef> {
    let mut out = *m;
    for v in out.as_flattened_mut() {
        *v = f(*v);
    }
    canonical(&out)
}

/// `f(a, b)` per pixel, canonical.
fn zip_tiles(a: &MaskPixels, b: &MaskPixels, f: impl Fn(u8, u8) -> u8) -> Option<MaskRef> {
    let mut out = *a;
    for (o, &s) in out.as_flattened_mut().iter_mut().zip(b.as_flattened()) {
        *o = f(*o, s);
    }
    canonical(&out)
}

/// Below this many tiles, per-tile work stays on the calling thread.
const PAR_MIN_TILES: usize = 32;

/// `items.map(f)`, spread over the rayon pool when there are enough items.
pub(crate) fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync + Send) -> Vec<R> {
    if items.len() < PAR_MIN_TILES { items.iter().map(f).collect() } else { items.par_iter().map(f).collect() }
}

/// Clear the selected pixels of a raster layer (`dst *= 1 − m/255` on all
/// four channels, so `c ≤ a` still holds) as one `Edit::Pixels`. Tiles left
/// fully transparent are removed. `None` when nothing changed, there is no
/// selection, or the layer is a folder or locked.
pub fn erase_selected(doc: &mut Document, layer: LayerId) -> Option<Edit> {
    let l = doc.layer(layer)?;
    let grid = l.raster()?;
    let sel = doc.selection();
    if l.props.locked || sel.is_empty() || grid.is_empty() {
        return None;
    }
    // Walk whichever side has fewer tiles.
    let jobs: Vec<(TileCoord, &TileRef, MaskView<'_>)> = if grid.len() <= sel.tile_count() {
        grid.iter().map(|(c, t)| (c, t, sel.get(c))).filter(|j| !matches!(j.2, MaskView::Empty)).collect()
    } else {
        sel.tiles().filter_map(|(c, _)| grid.get_ref(c).map(|t| (c, t, sel.get(c)))).collect()
    };
    let changes: Vec<(TileCoord, Option<TileRef>)> =
        par_map(&jobs, |&(c, t, m)| erase_tile(t, m).map(|t| (c, t))).into_iter().flatten().collect();
    if changes.is_empty() {
        return None;
    }
    let (grid, dirty) = doc.paint_target(layer)?;
    let mut tiles = Vec::with_capacity(changes.len());
    for (c, t) in changes {
        tiles.push((c, grid.replace(c, t)));
        dirty.mark(c);
    }
    Some(Edit::Pixels { layer, tiles })
}

/// The tile after erasing under `m`: `None` when it does not change,
/// `Some(None)` when it becomes fully transparent.
fn erase_tile(t: &TileRef, m: MaskView<'_>) -> Option<Option<TileRef>> {
    let m = match m {
        MaskView::Empty => return None,
        MaskView::Full => return Some(None),
        MaskView::Partial(m) => m,
    };
    let mut out: Option<TileRef> = None;
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let k = u32::from(255 - m[y][x]);
            if k == 255 || t[y][x][3] == 0 {
                continue;
            }
            let px = t[y][x].map(|v| ((u32::from(v) * k + 127) / 255) as u16);
            Arc::make_mut(out.get_or_insert_with(|| t.clone()))[y][x] = px;
        }
    }
    let out = out?;
    Some((!is_tile_empty(&out)).then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(v: u8) -> MaskRef {
        Arc::new([[v; TILE_SIZE]; TILE_SIZE])
    }

    #[test]
    fn insert_keeps_canonical_form() {
        let mut s = Selection::new();
        assert!(s.is_empty() && s.bounds().is_none());
        let (a, b, c) = (TileCoord::new(0, 0), TileCoord::new(3, 1), TileCoord::new(-1, 2));
        s.insert_tile(a, mask(255));
        assert!(matches!(s.get(a), MaskView::Full));
        assert!(Arc::ptr_eq(s.tiles().next().unwrap().1, full_mask()));
        let mut half = mask(0);
        Arc::make_mut(&mut half)[5][7] = 128;
        s.insert_tile(b, half);
        assert!(matches!(s.get(b), MaskView::Partial(_)));
        assert_eq!(s.value(3 * 64 + 7, 64 + 5), 128);
        assert_eq!(s.value(3, 3), 255);
        assert_eq!(s.value(-3, 3), 0);
        assert_eq!(s.byte_size(), 4096);
        assert_eq!(s.tile_count(), 2);
        assert_eq!(s.bounds(), Some(TileRect { x0: 0, y0: 0, x1: 4, y1: 2 }));

        // An all-0 tile is never stored, and replaces what was there.
        s.insert_tile(c, mask(0));
        assert!(matches!(s.get(c), MaskView::Empty));
        assert_eq!(s.tile_count(), 2);
        s.insert_tile(a, mask(0));
        assert!(matches!(s.get(a), MaskView::Empty));
        assert_eq!(s.bounds(), Some(TileRect { x0: 0, y0: 0, x1: 4, y1: 2 }), "bounds do not shrink");
    }

    #[test]
    fn clones_share_until_written() {
        let mut s = Selection::new();
        s.insert_tile(TileCoord::new(0, 0), full_mask().clone());
        let snap = s.clone();
        assert!(s.shares_storage(&snap));
        assert!(s.remove_tile(TileCoord::new(5, 5)).is_none());
        assert!(s.shares_storage(&snap), "removing a missing tile keeps sharing");
        s.remove_tile(TileCoord::new(0, 0));
        assert!(!s.shares_storage(&snap));
        assert!(s.is_empty() && !snap.is_empty());
    }

    #[test]
    fn replace_combine_takes_the_shape() {
        let mut shape = Selection::new();
        shape.insert_tile(TileCoord::new(1, 1), full_mask().clone());
        let mut s = Selection::new();
        s.combine(&shape, SelectOp::Replace);
        assert!(s.shares_storage(&shape));
    }
}
