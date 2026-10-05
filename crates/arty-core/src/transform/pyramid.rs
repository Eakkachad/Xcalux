//! Lazily built 2× box mips of a session's source, for downscaling.
//!
//! Level `k` pixel `i` covers level-0 pixels `[i·2ᵏ, (i+1)·2ᵏ)`. Each level
//! is a tile grid of its own resolution, built in parallel from the level
//! below the first time a transform needs it.

use ahash::AHashSet;
use rayon::prelude::*;

use crate::grid::TileGrid;
use crate::tile::{TILE_SIZE, TileCoord, TilePixels, TileRef, new_tile};

/// Deepest level built (64×: a tile shrinks to one pixel).
pub(super) const MAX_LEVEL: u32 = 6;

#[derive(Default)]
pub(super) struct SrcPyramid {
    /// `levels[k]` is level `k + 1`.
    levels: Vec<TileGrid>,
}

impl SrcPyramid {
    /// Build the levels up to `level` (≤ [`MAX_LEVEL`]) if they are missing.
    pub(super) fn ensure(&mut self, src: &TileGrid, level: u32) {
        while (self.levels.len() as u32) < level.min(MAX_LEVEL) {
            let next = downsample(self.levels.last().unwrap_or(src));
            self.levels.push(next);
        }
    }

    /// Level `level` (0 = `src`). [`Self::ensure`] it first.
    pub(super) fn get<'a>(&'a self, src: &'a TileGrid, level: u32) -> &'a TileGrid {
        match level {
            0 => src,
            l => &self.levels[(l - 1) as usize],
        }
    }
}

/// The 2× box mip of `g`.
fn downsample(g: &TileGrid) -> TileGrid {
    let parents: AHashSet<TileCoord> =
        g.coords().map(|c| TileCoord::new(c.x.div_euclid(2), c.y.div_euclid(2))).collect();
    let mut parents: Vec<TileCoord> = parents.into_iter().collect();
    parents.sort_unstable();
    let tiles: Vec<(TileCoord, TileRef)> = parents
        .par_iter()
        .filter_map(|&p| {
            let mut t = new_tile();
            let any = mip_tile(g, p, std::sync::Arc::get_mut(&mut t).expect("fresh tile"));
            any.then_some((p, t))
        })
        .collect();
    let mut out = TileGrid::with_capacity(tiles.len());
    for (c, t) in tiles {
        out.insert(c, t);
    }
    out
}

/// Fill parent tile `p` from its four children; whether any alpha remains.
fn mip_tile(g: &TileGrid, p: TileCoord, out: &mut TilePixels) -> bool {
    const HALF: usize = TILE_SIZE / 2;
    let mut any = 0u16;
    for qy in 0..2 {
        for qx in 0..2 {
            let Some(child) = g.get(TileCoord::new(p.x * 2 + qx as i32, p.y * 2 + qy as i32)) else { continue };
            for y in 0..HALF {
                let (r0, r1) = (&child[2 * y], &child[2 * y + 1]);
                let row = &mut out[qy * HALF + y][qx * HALF..qx * HALF + HALF];
                for (x, d) in row.iter_mut().enumerate() {
                    for k in 0..4 {
                        let s = r0[2 * x][k] as u32 + r0[2 * x + 1][k] as u32 + r1[2 * x][k] as u32 + r1[2 * x + 1][k] as u32;
                        d[k] = ((s + 2) >> 2) as u16;
                    }
                    any |= d[3];
                }
            }
        }
    }
    any != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_average_and_keep_alpha_order() {
        let mut g = TileGrid::new();
        // A 2-pixel checker of opaque red and transparent, over tile (-1, 0).
        let px = g.get_mut_or_create(TileCoord::new(-1, 0));
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                px[y][x] = if (x + y) % 2 == 0 { [32768, 0, 0, 32768] } else { [0; 4] };
            }
        }
        let mut pyr = SrcPyramid::default();
        pyr.ensure(&g, 2);
        let l1 = pyr.get(&g, 1);
        assert_eq!(l1.len(), 1);
        let t = l1.get(TileCoord::new(-1, 0)).unwrap();
        assert_eq!(t[0][32], [16384, 0, 0, 16384], "the child lands in the right quadrant");
        assert_eq!(t[0][0], [0; 4]);
        let l2 = pyr.get(&g, 2).get(TileCoord::new(-1, 0)).unwrap();
        assert_eq!(l2[0][48], [16384, 0, 0, 16384]);
        assert!(l2.as_flattened().iter().all(|p| p[0] <= p[3]));
    }
}
