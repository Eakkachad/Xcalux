//! hokusai `TiledSurface` over an ARTY raster layer.

use arty_core::{DirtyRegion, PixelRecorder, TileCoord, TileGrid, TilePixels};

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
        if !self.in_page(c) {
            return self.discard;
        }
        self.recorder.before_write(self.grid, c);
        self.dirty.mark(c);
        self.grid.get_mut_or_create(c)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&hokusai::TilePixels> {
        self.grid.get(TileCoord::new(tx, ty))
    }
}
