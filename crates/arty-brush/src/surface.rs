//! hokusai `TiledSurface` over an ARTY raster layer.

use arty_core::{DirtyRegion, PixelRecorder, Selection, TileCoord, TileGrid, TilePixels};

use crate::shape::{DabStats, TileClip};

/// Lends layer tiles to hokusai while recording undo state and marking the
/// composite dirty. Tiles outside the page go to a throwaway buffer.
pub struct LayerSurface<'a> {
    pub grid: &'a mut TileGrid,
    pub dirty: &'a mut DirtyRegion,
    pub recorder: &'a mut PixelRecorder,
    pub discard: &'a mut TilePixels,
    /// Page size in tiles.
    pub tiles_wide: i32,
    pub tiles_high: i32,
    /// During a clipped (Tail) replay: only these tiles are repainted, and
    /// dabs that miss them are skipped.
    pub clip: Option<&'a TileClip>,
    /// Running cost of the dabs drawn.
    pub stats: &'a mut DabStats,
    /// The selection painting is limited to (`None`: no selection).
    pub mask: Option<&'a Selection>,
    /// Mask of the tile hokusai currently holds.
    pub mask_cur: MaskCur,
}

/// The selection over the tile being painted. SEL-CORE adds the variants
/// for partially selected tiles.
#[derive(Default, Clone, Copy)]
pub enum MaskCur {
    /// Every pixel may be painted.
    #[default]
    All,
}

impl LayerSurface<'_> {
    #[inline]
    fn in_page(&self, c: TileCoord) -> bool {
        c.x >= 0 && c.y >= 0 && c.x < self.tiles_wide && c.y < self.tiles_high
    }
}

impl hokusai::TiledSurface for LayerSurface<'_> {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut hokusai::TilePixels {
        let c = TileCoord::new(tx, ty);
        if !self.in_page(c) || self.clip.is_some_and(|k| !k.contains(c)) {
            return self.discard;
        }
        self.recorder.before_write(self.grid, c);
        self.dirty.mark(c);
        self.grid.get_mut_or_create(c)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn draw_dab(&mut self, dab: &hokusai::Dab) -> bool {
        if self.clip.is_some_and(|k| !k.touches(dab.x, dab.y, dab.radius + 1.0)) {
            return false;
        }
        self.stats.dabs += 1;
        let d = (2.0 * dab.radius + 3.0) as u64;
        self.stats.px += d * d;
        hokusai::brushmodes::draw_dab_default(self, dab)
    }

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&hokusai::TilePixels> {
        self.grid.get(TileCoord::new(tx, ty))
    }
}
