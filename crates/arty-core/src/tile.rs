//! 64×64 fix15 RGBA tiles with copy-on-write sharing.
//!
//! Pixel layout is identical to `hokusai::TilePixels` (`[y][x][rgba]`,
//! premultiplied, 0..=1<<15), so the brush engine paints straight into our
//! tiles with no conversion. Channel values are stored sRGB-encoded: like
//! SAI, blending happens in display space.
//!
//! Tiles are `Arc`-shared. A history snapshot or autosave is a pointer
//! clone; the first write afterwards copies the tile (`Arc::make_mut`).

use std::sync::Arc;

pub const TILE_SIZE: usize = 64;
pub const TILE_SIZE_I32: i32 = TILE_SIZE as i32;
pub const TILE_PIXELS: usize = TILE_SIZE * TILE_SIZE;

/// `[y][x][rgba]`, fix15 premultiplied.
pub type TilePixels = [[[u16; 4]; TILE_SIZE]; TILE_SIZE];

/// Shared, copy-on-write tile storage.
pub type TileRef = Arc<TilePixels>;

/// Integer tile coordinate (`tile = floor(pixel / 64)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TileCoord {
    pub x: i32,
    pub y: i32,
}

impl TileCoord {
    #[inline]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Tile containing world pixel `(px, py)`.
    #[inline]
    pub fn from_pixel(px: i32, py: i32) -> Self {
        Self::new(px.div_euclid(TILE_SIZE_I32), py.div_euclid(TILE_SIZE_I32))
    }

    /// World pixel of this tile's top-left corner.
    #[inline]
    pub fn origin(self) -> (i32, i32) {
        (self.x * TILE_SIZE_I32, self.y * TILE_SIZE_I32)
    }
}

/// A fully transparent tile, allocated zeroed (no 32 KiB stack temporary).
#[inline]
pub fn new_tile() -> TileRef {
    let uninit = Arc::<TilePixels>::new_zeroed();
    // SAFETY: TilePixels is a plain u16 array; all-zero is a valid value.
    unsafe { uninit.assume_init() }
}

/// A boxed transparent tile, used for scratch buffers.
#[inline]
pub fn new_tile_box() -> Box<TilePixels> {
    let uninit = Box::<TilePixels>::new_zeroed();
    // SAFETY: as above.
    unsafe { uninit.assume_init() }
}

/// True when every pixel has zero alpha (and therefore, being
/// premultiplied, zero color).
pub fn is_tile_empty(px: &TilePixels) -> bool {
    px.as_flattened().iter().all(|p| p[3] == 0)
}

#[inline]
pub fn clear_tile(px: &mut TilePixels) {
    px.as_flattened_mut().fill([0; 4]);
}

#[inline]
pub fn fill_tile(px: &mut TilePixels, value: [u16; 4]) {
    px.as_flattened_mut().fill(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coord_from_negative_pixels() {
        assert_eq!(TileCoord::from_pixel(-1, -64), TileCoord::new(-1, -1));
        assert_eq!(TileCoord::from_pixel(-65, 63), TileCoord::new(-2, 0));
        assert_eq!(TileCoord::from_pixel(64, 128), TileCoord::new(1, 2));
    }

    #[test]
    fn new_tile_is_empty() {
        assert!(is_tile_empty(&new_tile()));
    }

    #[test]
    fn copy_on_write_only_copies_when_shared() {
        let mut a = new_tile();
        let snapshot = a.clone();
        Arc::make_mut(&mut a)[0][0] = [1, 2, 3, 4];
        assert_eq!(snapshot[0][0], [0; 4]);
        assert_eq!(a[0][0], [1, 2, 3, 4]);
    }
}
