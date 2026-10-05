//! hokusai `TiledSurface` over an ARTY raster layer.

use arty_core::{DirtyRegion, MaskPixels, MaskView, PixelRecorder, Selection, TILE_SIZE, TileCoord, TileGrid, TilePixels};

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
    pub mask_cur: MaskCur<'a>,
}

/// The selection over the tile being painted.
#[derive(Default, Clone, Copy)]
pub enum MaskCur<'a> {
    /// Every pixel may be painted.
    #[default]
    All,
    /// No pixel may be painted (the tile is unselected; hokusai holds the
    /// discard buffer).
    None,
    /// Partly selected: the tile's mask and its document origin.
    Tile(&'a MaskPixels, i32, i32),
}

/// `v / 255` for every mask value.
static MASK_SCALE: [f32; 256] = {
    let mut t = [0.0f32; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as f32 / 255.0;
        i += 1;
    }
    t
};

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
            self.mask_cur = MaskCur::All;
            return self.discard;
        }
        if let Some(sel) = self.mask {
            self.mask_cur = match sel.get(c) {
                // Nothing selected: no undo record, no dirty mark, no tile.
                MaskView::Empty => {
                    self.mask_cur = MaskCur::None;
                    return self.discard;
                }
                MaskView::Full => MaskCur::All,
                MaskView::Partial(m) => {
                    let (ox, oy) = c.origin();
                    MaskCur::Tile(m, ox, oy)
                }
            };
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
        if let Some(sel) = self.mask {
            // The same bbox hokusai paints (radius + 1, floored corners).
            let r = dab.radius + 1.0;
            let t = |v: f32| (v.floor() as i32).div_euclid(TILE_SIZE as i32);
            let hit = sel.bounds().is_some_and(|b| {
                t(dab.x - r) < b.x1 && t(dab.x + r) >= b.x0 && t(dab.y - r) < b.y1 && t(dab.y + r) >= b.y0
            });
            if !hit {
                return false;
            }
        }
        self.stats.dabs += 1;
        let d = (2.0 * dab.radius + 3.0) as u64;
        self.stats.px += d * d;
        hokusai::brushmodes::draw_dab_default(self, dab)
    }

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&hokusai::TilePixels> {
        self.grid.get(TileCoord::new(tx, ty))
    }

    /// Selection coverage of pixel `(px, py)` of the tile hokusai holds.
    ///
    /// hokusai calls this through a raw pointer to the surface while it
    /// writes the lent tile, so it reads only `mask_cur` (never `grid`).
    #[inline]
    fn get_pixel_mask(&self, px: f32, py: f32, _dab: &hokusai::Dab) -> f32 {
        match self.mask_cur {
            MaskCur::All => 1.0,
            MaskCur::None => 0.0,
            MaskCur::Tile(m, ox, oy) => {
                let (x, y) = (px as i32, py as i32);
                debug_assert!((0..TILE_SIZE as i32).contains(&(x - ox)) && (0..TILE_SIZE as i32).contains(&(y - oy)));
                // Tile origins are multiples of 64: the low bits are the
                // in-tile position.
                MASK_SCALE[usize::from(m[y as usize & (TILE_SIZE - 1)][x as usize & (TILE_SIZE - 1)])]
            }
        }
    }
}
