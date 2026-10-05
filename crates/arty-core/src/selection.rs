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
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::geom::TileRect;
use crate::history::Edit;
use crate::layer::LayerId;
use crate::tile::{TILE_SIZE, TileCoord};

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

    /// Mask bytes held by this selection: partial tiles only, since every
    /// full tile shares [`full_mask`].
    pub fn byte_size(&self) -> usize {
        let partial = self.tiles.values().filter(|t| !Arc::ptr_eq(t, full_mask())).count();
        partial * std::mem::size_of::<MaskPixels>()
    }

    /// Every page tile of a `w`×`h` page selected.
    pub fn all(_w: u32, _h: u32) -> Selection {
        // SEL-CORE: insert a `full_mask()` clone per page tile.
        Selection::default()
    }

    /// The complement within a `w`×`h` page.
    pub fn inverted(&self, _w: u32, _h: u32) -> Selection {
        // SEL-CORE: absent → FULL, FULL → removed, partial → 255 − v.
        self.clone()
    }

    /// Combine `shape` into this selection.
    pub fn combine(&mut self, shape: &Selection, op: SelectOp) {
        // SEL-CORE: Add / Subtract / Intersect per tile.
        if op == SelectOp::Replace {
            *self = shape.clone();
        }
    }
}

/// Clear the selected pixels of a raster layer (`dst *= 1 − m/255`) as one
/// `Edit::Pixels`. `None` when nothing changed or the layer is locked.
pub fn erase_selected(_doc: &mut Document, _layer: LayerId) -> Option<Edit> {
    // SEL-CORE
    None
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
