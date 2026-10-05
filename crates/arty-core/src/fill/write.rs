//! Writing a fill region into a raster layer.

use std::sync::Arc;

use rayon::prelude::*;

use crate::fix15::{self, ONE};
use crate::selection::{MaskPixels, full_mask};
use crate::tile::{TILE_SIZE, TileCoord, TilePixels, TileRef, fill_tile, new_tile};

use super::FillBlend;

/// How a layer takes the fill.
#[derive(Clone, Copy)]
pub(super) struct Paint {
    /// fix15 premultiplied, `c ≤ a`.
    pub color: [u16; 4],
    /// fix15.
    pub opacity: u32,
    pub blend: FillBlend,
    pub lock_alpha: bool,
}

impl Paint {
    /// Write one pixel at coverage `k` (fix15). The result keeps `c ≤ a ≤ ONE`.
    #[inline]
    fn pixel(&self, d: [u16; 4], k: u32) -> [u16; 4] {
        let s = self.color;
        if self.lock_alpha {
            // Keep alpha; move the colour towards the fill colour at the
            // pixel's own alpha.
            let da = d[3] as u32;
            let sa = s[3] as u32;
            if da == 0 || sa == 0 {
                return d;
            }
            let t = fix15::mul(k, sa);
            let mut out = d;
            for c in 0..3 {
                let straight = ((s[c] as u32 * ONE + sa / 2) / sa).min(ONE);
                let target = fix15::mul(straight, da);
                out[c] = ((d[c] as u32 * (ONE - t) + target * t) >> 15).min(da) as u16;
            }
            return out;
        }
        let src = [0, 1, 2, 3].map(|c| fix15::mul(s[c] as u32, k));
        match self.blend {
            FillBlend::Normal => {
                let inv = ONE - src[3];
                [0, 1, 2, 3].map(|c| (src[c] + fix15::mul(d[c] as u32, inv)) as u16)
            }
            FillBlend::Behind => {
                let inv = ONE - d[3] as u32;
                [0, 1, 2, 3].map(|c| (d[c] as u32 + fix15::mul(src[c], inv)) as u16)
            }
        }
    }

    /// The tile this fill makes of `old` under `mask` (rows and columns
    /// past the page are left alone). `None` when nothing changes.
    fn tile(&self, old: Option<&TileRef>, mask: &MaskPixels, rows: usize, cols: usize) -> Option<TileRef> {
        if old.is_none() && self.lock_alpha {
            return None;
        }
        let mut out = new_tile();
        let px: &mut TilePixels = Arc::get_mut(&mut out).expect("fresh tile");
        if let Some(o) = old {
            *px = **o;
        }
        let mut changed = false;
        for (prow, mrow) in px.iter_mut().zip(mask.iter()).take(rows) {
            for (p, &m) in prow.iter_mut().zip(mrow.iter()).take(cols) {
                if m == 0 {
                    continue;
                }
                let k = (m as u32 * self.opacity + 127) / 255;
                let v = self.pixel(*p, k);
                changed |= v != *p;
                *p = v;
            }
        }
        changed.then_some(out)
    }
}

/// New tiles for every page tile of `region` that changes, in no
/// particular order. Absent tiles that a full mask covers share one solid
/// tile.
pub(super) fn paint_tiles<'a>(
    paint: &Paint,
    old: impl Fn(TileCoord) -> Option<&'a TileRef> + Sync,
    region: &[(TileCoord, &crate::selection::MaskRef)],
    w: i32,
    h: i32,
) -> Vec<(TileCoord, TileRef)> {
    let solid = (!paint.lock_alpha).then(|| {
        let mut t = new_tile();
        let v = paint.pixel([0; 4], paint.opacity);
        fill_tile(Arc::get_mut(&mut t).expect("fresh tile"), v);
        (v != [0; 4]).then_some(t)
    });
    let solid = solid.flatten();
    region
        .par_iter()
        .filter_map(|&(c, m)| {
            let (ox, oy) = c.origin();
            let rows = (h - oy).min(TILE_SIZE as i32) as usize;
            let cols = (w - ox).min(TILE_SIZE as i32) as usize;
            let prev = old(c);
            if prev.is_none() && rows == TILE_SIZE && cols == TILE_SIZE && Arc::ptr_eq(m, full_mask()) {
                return solid.clone().map(|t| (c, t));
            }
            paint.tile(prev, m, rows, cols).map(|t| (c, t))
        })
        .collect()
}
