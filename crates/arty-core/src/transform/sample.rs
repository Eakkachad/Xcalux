//! Per-tile resampling: one destination tile from a source tile grid.
//!
//! Positions arrive in source *index* space: a destination pixel centre
//! `(x + .5, y + .5)` mapped through the inverse transform, divided by the
//! pyramid level's scale, then `− 0.5`, so source pixel `i` sits at `i`.
//! All filters work on premultiplied values and keep `c ≤ a ≤ ONE`.
//!
//! Each destination tile first gathers the source tiles its inverse image
//! can reach into a small [`Window`], so sampling indexes an array instead
//! of hashing (a footprint that straddles two tiles would otherwise thrash a
//! one-entry cache). Footprints inside one tile index it directly.

use crate::fix15::ONE;
use crate::geom::Affine64;
use crate::grid::TileGrid;
use crate::tile::{TILE_SIZE, TILE_SIZE_I32, TileCoord, TilePixels};

use super::Filter;

const ONE_F: f32 = ONE as f32;
/// Keeps `floor() as i32` and `+ 2` far from overflow for wild maps.
const COORD_LIMIT: f32 = 1.0e8;
/// Window side in tiles; larger reaches fall back to hashed lookups.
const WIN: usize = 8;
/// A filter reaches this many source px around the sample point (plus one
/// for f32 rounding).
const REACH: f64 = 3.0;

/// The source tiles one destination tile can read.
pub(super) struct Window<'a> {
    grid: &'a TileGrid,
    tx0: i32,
    ty0: i32,
    w: u32,
    h: u32,
    /// `tiles` holds the whole range; otherwise look tiles up (1-entry cache).
    small: bool,
    tiles: [Option<&'a TilePixels>; WIN * WIN],
    key: Option<TileCoord>,
    last: Option<&'a TilePixels>,
}

impl<'a> Window<'a> {
    /// The inclusive tile range `(tx0, ty0, tx1, ty1)` of `grid`; `None`
    /// when it holds no tile (past `probe_cap` tiles it is assumed to).
    pub(super) fn new(grid: &'a TileGrid, (tx0, ty0, tx1, ty1): (i32, i32, i32, i32), probe_cap: i64) -> Option<Self> {
        let (w, h) = ((tx1 - tx0 + 1).max(0) as i64, (ty1 - ty0 + 1).max(0) as i64);
        let mut win = Window { grid, tx0, ty0, w: w as u32, h: h as u32, small: false, tiles: [None; WIN * WIN], key: None, last: None };
        if w * h <= (WIN * WIN) as i64 {
            win.small = true;
            let mut any = false;
            for j in 0..h as i32 {
                for i in 0..w as i32 {
                    let t = grid.get(TileCoord::new(tx0 + i, ty0 + j));
                    any |= t.is_some();
                    win.tiles[(j * w as i32 + i) as usize] = t;
                }
            }
            return any.then_some(win);
        }
        let any = w * h > probe_cap || (ty0..=ty1).any(|y| (tx0..=tx1).any(|x| grid.get(TileCoord::new(x, y)).is_some()));
        any.then_some(win)
    }

    /// The window for destination tile `c` sampled through `a`.
    pub(super) fn for_dest(grid: &'a TileGrid, a: &Affine64, c: TileCoord, probe_cap: i64) -> Option<Self> {
        let (ox, oy) = ((c.x * TILE_SIZE_I32) as f64, (c.y * TILE_SIZE_I32) as f64);
        let t = TILE_SIZE as f64;
        let pts = [[ox, oy], [ox + t, oy], [ox + t, oy + t], [ox, oy + t]].map(|p| a.apply(p));
        let lo = |i: usize| pts.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min) - REACH;
        let hi = |i: usize| pts.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max) + REACH;
        let tile = |v: f64| (v / t).floor().clamp(-1.0e7, 1.0e7) as i32;
        Self::new(grid, (tile(lo(0)), tile(lo(1)), tile(hi(0)), tile(hi(1))), probe_cap)
    }

    #[inline(always)]
    fn tile(&mut self, tx: i32, ty: i32) -> Option<&'a TilePixels> {
        if self.small {
            let (i, j) = (tx.wrapping_sub(self.tx0) as u32, ty.wrapping_sub(self.ty0) as u32);
            return if i < self.w && j < self.h { self.tiles[(j * self.w + i) as usize] } else { None };
        }
        let c = TileCoord::new(tx, ty);
        if self.key != Some(c) {
            self.key = Some(c);
            self.last = self.grid.get(c);
        }
        self.last
    }

    #[inline(always)]
    fn px(&mut self, x: i32, y: i32) -> [u16; 4] {
        match self.tile(x >> 6, y >> 6) {
            Some(t) => t[(y & 63) as usize][(x & 63) as usize],
            None => [0; 4],
        }
    }
}

/// Resample destination tile `c` from `win` through `a` (destination pixel
/// centre → source index space) with `f`. Returns whether any pixel has
/// alpha.
pub(super) fn resample(win: &mut Window, a: &Affine64, c: TileCoord, f: Filter, out: &mut TilePixels) -> bool {
    match f {
        Filter::Nearest => rows(win, a, c, out, nearest),
        Filter::Bilinear => rows(win, a, c, out, bilinear),
        Filter::Bicubic => rows(win, a, c, out, bicubic),
    }
}

/// The row walk: the row start in f64, then f32 steps along the row.
#[inline(always)]
fn rows(
    win: &mut Window,
    a: &Affine64,
    c: TileCoord,
    out: &mut TilePixels,
    sample: impl Fn(&mut Window, f32, f32) -> [u16; 4],
) -> bool {
    let (ox, oy) = c.origin();
    let [m0, m1, m2, m3, m4, m5] = a.m;
    let (du, dv) = (m0 as f32, m3 as f32);
    let px = ox as f64 + 0.5;
    let mut any = 0u16;
    for (y, row) in out.iter_mut().enumerate() {
        let py = (oy + y as i32) as f64 + 0.5;
        let u0 = (m0 * px + m1 * py + m2) as f32;
        let v0 = (m3 * px + m4 * py + m5) as f32;
        for (x, p) in row.iter_mut().enumerate() {
            let i = x as f32;
            let u = (u0 + du * i).clamp(-COORD_LIMIT, COORD_LIMIT);
            let v = (v0 + dv * i).clamp(-COORD_LIMIT, COORD_LIMIT);
            *p = sample(win, u, v);
            any |= p[3];
        }
    }
    any != 0
}

/// `v.floor()` as an integer and as f32, without a libm call (`|v|` is
/// within [`COORD_LIMIT`]).
#[inline(always)]
fn floor(v: f32) -> (i32, f32) {
    let i = v as i32;
    let i = if (i as f32) > v { i - 1 } else { i };
    (i, i as f32)
}

#[inline(always)]
fn nearest(win: &mut Window, u: f32, v: f32) -> [u16; 4] {
    win.px(floor(u + 0.5).0, floor(v + 0.5).0)
}

/// One pixel's four channels as f32 lanes (premultiplied r, g, b, a):
/// SSE2 (the x86_64 baseline) there, plain arrays elsewhere.
#[cfg(target_arch = "x86_64")]
mod lanes {
    // SAFETY (every block here): SSE2 is part of the x86_64 baseline, so
    // its intrinsics are always available; the one load and one store
    // touch exactly the bytes of the array they are given.
    use std::arch::x86_64::*;

    #[derive(Clone, Copy)]
    pub(super) struct L(__m128);

    #[inline(always)]
    pub(super) fn zero() -> L {
        L(unsafe { _mm_setzero_ps() })
    }

    /// Two adjacent pixels.
    #[inline(always)]
    pub(super) fn load2(p: &[[u16; 4]; 2]) -> (L, L) {
        unsafe {
            let v = _mm_loadu_si128(p.as_ptr().cast());
            let z = _mm_setzero_si128();
            (L(_mm_cvtepi32_ps(_mm_unpacklo_epi16(v, z))), L(_mm_cvtepi32_ps(_mm_unpackhi_epi16(v, z))))
        }
    }

    /// `acc + x·w`.
    #[inline(always)]
    pub(super) fn madd(acc: L, x: L, w: f32) -> L {
        L(unsafe { _mm_add_ps(acc.0, _mm_mul_ps(x.0, _mm_set1_ps(w))) })
    }

    /// Round, clamping `a` to `[0, one]` and each colour to `[0, a]`.
    #[inline(always)]
    pub(super) fn finish(acc: L, one: f32) -> [u16; 4] {
        let mut o = [0i32; 4];
        unsafe {
            let a = _mm_shuffle_ps::<0xFF>(acc.0, acc.0);
            let a = _mm_min_ps(_mm_max_ps(a, _mm_setzero_ps()), _mm_set1_ps(one));
            let v = _mm_min_ps(_mm_max_ps(acc.0, _mm_setzero_ps()), a);
            _mm_storeu_si128(o.as_mut_ptr().cast(), _mm_cvttps_epi32(_mm_add_ps(v, _mm_set1_ps(0.5))));
        }
        [o[0] as u16, o[1] as u16, o[2] as u16, o[3] as u16]
    }
}

#[cfg(not(target_arch = "x86_64"))]
mod lanes {
    #[derive(Clone, Copy)]
    pub(super) struct L([f32; 4]);

    #[inline(always)]
    pub(super) fn zero() -> L {
        L([0.0; 4])
    }

    #[inline(always)]
    pub(super) fn load2(p: &[[u16; 4]; 2]) -> (L, L) {
        (L(p[0].map(f32::from)), L(p[1].map(f32::from)))
    }

    #[inline(always)]
    pub(super) fn madd(acc: L, x: L, w: f32) -> L {
        L(std::array::from_fn(|k| acc.0[k] + x.0[k] * w))
    }

    #[inline(always)]
    pub(super) fn finish(acc: L, one: f32) -> [u16; 4] {
        let a = acc.0[3].clamp(0.0, one);
        let ch = |v: f32| (v.clamp(0.0, a) + 0.5) as u16;
        [ch(acc.0[0]), ch(acc.0[1]), ch(acc.0[2]), (a + 0.5) as u16]
    }
}

use lanes::{L, load2, madd};

/// Round an accumulated premultiplied pixel, keeping `c ≤ a ≤ ONE`.
#[inline(always)]
fn finish(acc: L) -> [u16; 4] {
    lanes::finish(acc, ONE_F)
}

/// Four taps of one row weighted by `w`.
#[inline(always)]
fn row4(row: &[[u16; 4]; 4], w: [f32; 4]) -> L {
    let (a, b) = load2(row[..2].try_into().expect("2 px"));
    let (c, d) = load2(row[2..].try_into().expect("2 px"));
    madd(madd(madd(madd(lanes::zero(), a, w[0]), b, w[1]), c, w[2]), d, w[3])
}

#[inline(always)]
fn bilinear(win: &mut Window, u: f32, v: f32) -> [u16; 4] {
    let ((x0, fx), (y0, fy)) = (floor(u), floor(v));
    let (tx, ty) = (u - fx, v - fy);
    let (lx, ly) = ((x0 & 63) as usize, (y0 & 63) as usize);
    let rows: [[[u16; 4]; 2]; 2] = if lx < TILE_SIZE - 1 && ly < TILE_SIZE - 1 {
        let Some(t) = win.tile(x0 >> 6, y0 >> 6) else { return [0; 4] };
        let r = |y: usize| -> [[u16; 4]; 2] { t[y][lx..lx + 2].try_into().expect("2 px") };
        [r(ly), r(ly + 1)]
    } else {
        [[win.px(x0, y0), win.px(x0 + 1, y0)], [win.px(x0, y0 + 1), win.px(x0 + 1, y0 + 1)]]
    };
    if rows.as_flattened().iter().all(|q| q[3] == 0) {
        return [0; 4];
    }
    let (p00, p10) = load2(&rows[0]);
    let (p01, p11) = load2(&rows[1]);
    let acc = madd(madd(lanes::zero(), p00, (1.0 - tx) * (1.0 - ty)), p10, tx * (1.0 - ty));
    finish(madd(madd(acc, p01, (1.0 - tx) * ty), p11, tx * ty))
}

/// Catmull-Rom weights for taps −1, 0, 1, 2 at fraction `t`.
#[inline(always)]
fn catmull_rom(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

#[inline(always)]
fn bicubic(win: &mut Window, u: f32, v: f32) -> [u16; 4] {
    let ((x0, fx), (y0, fy)) = (floor(u), floor(v));
    let (wx, wy) = (catmull_rom(u - fx), catmull_rom(v - fy));
    let (lx, ly) = ((x0 & 63) as usize, (y0 & 63) as usize);
    let mut acc = lanes::zero();
    if (1..TILE_SIZE - 2).contains(&lx) && (1..TILE_SIZE - 2).contains(&ly) {
        // The 4×4 footprint is inside one tile: index it directly.
        let Some(t) = win.tile(x0 >> 6, y0 >> 6) else { return [0; 4] };
        for (j, wy) in wy.into_iter().enumerate() {
            let row: &[[u16; 4]; 4] = t[ly + j - 1][lx - 1..lx + 3].try_into().expect("4 taps");
            acc = madd(acc, row4(row, wx), wy);
        }
    } else {
        for (j, wy) in wy.into_iter().enumerate() {
            let y = y0 + j as i32 - 1;
            let row: [[u16; 4]; 4] = std::array::from_fn(|i| win.px(x0 + i as i32 - 1, y));
            acc = madd(acc, row4(&row, wx), wy);
        }
    }
    finish(acc)
}

/// The source tiles a whole-pixel shift by `(dx, dy)` reads for tile `c`.
pub(super) fn shift_window(grid: &TileGrid, dx: i32, dy: i32, c: TileCoord) -> Option<Window<'_>> {
    let (ox, oy) = c.origin();
    let (sx, sy) = (ox - dx, oy - dy);
    Window::new(grid, (sx >> 6, sy >> 6, (sx + 63) >> 6, (sy + 63) >> 6), i64::MAX)
}

/// Tile `c` of the source shifted by whole pixels: `out(x, y) = src(x − dx,
/// y − dy)`, from [`shift_window`]. Returns whether any pixel has alpha.
pub(super) fn shift_copy(win: &mut Window, dx: i32, dy: i32, c: TileCoord, out: &mut TilePixels) -> bool {
    let (ox, oy) = c.origin();
    let (sx, sy) = (ox - dx, oy - dy);
    let (tx0, lx) = (sx >> 6, (sx & 63) as usize);
    let n0 = TILE_SIZE - lx;
    let mut any = false;
    for (y, row) in out.iter_mut().enumerate() {
        let syy = sy + y as i32;
        let (ty, ly) = (syy >> 6, (syy & 63) as usize);
        let (left, right) = row.split_at_mut(n0);
        match win.tile(tx0, ty) {
            Some(t) => left.copy_from_slice(&t[ly][lx..]),
            None => left.fill([0; 4]),
        }
        if lx > 0 {
            match win.tile(tx0 + 1, ty) {
                Some(t) => right.copy_from_slice(&t[ly][..lx]),
                None => right.fill([0; 4]),
            }
        }
        any |= row.iter().any(|p| p[3] != 0);
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::new_tile;

    #[test]
    fn catmull_rom_weights_sum_to_one() {
        for i in 0..=16 {
            let w = catmull_rom(i as f32 / 16.0);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        }
        assert_eq!(catmull_rom(0.0), [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn floor_matches_std() {
        for v in [-2.5f32, -2.0, -1.0e-7, 0.0, 0.3, 1.0, 7.99, -63.5, 1.0e7] {
            assert_eq!(floor(v), (v.floor() as i32, v.floor()), "{v}");
        }
    }

    #[test]
    fn shift_copy_moves_pixels_across_tile_seams() {
        let mut grid = TileGrid::new();
        let mut t = new_tile();
        std::sync::Arc::make_mut(&mut t)[63][63] = [7, 7, 7, 9];
        grid.insert(TileCoord::new(0, 0), t);
        let mut out = crate::tile::new_tile_box();
        let copy = |c: TileCoord, out: &mut TilePixels| {
            shift_window(&grid, 3, 2, c).map(|mut w| shift_copy(&mut w, 3, 2, c, out))
        };
        // (63, 63) moved by (+3, +2) lands at (66, 65), inside tile (1, 1).
        assert_eq!(copy(TileCoord::new(1, 1), &mut out), Some(true));
        assert_eq!(out[1][2], [7, 7, 7, 9]);
        assert_eq!(out.as_flattened().iter().filter(|p| p[3] != 0).count(), 1);
        assert_eq!(copy(TileCoord::new(0, 0), &mut out), Some(false));
        assert_eq!(copy(TileCoord::new(5, 5), &mut out), None);
    }

    #[test]
    fn windows_cover_their_range_and_fall_back_past_it() {
        let mut grid = TileGrid::new();
        grid.insert(TileCoord::new(2, -1), new_tile());
        grid.insert(TileCoord::new(40, 3), new_tile());
        let mut w = Window::new(&grid, (0, -2, 3, 1), 4096).unwrap();
        assert!(w.small && w.tile(2, -1).is_some() && w.tile(1, -1).is_none() && w.tile(9, -1).is_none());
        assert!(Window::new(&grid, (0, 0, 3, 1), 4096).is_none());
        let mut big = Window::new(&grid, (0, 0, 50, 5), 4096).unwrap();
        assert!(!big.small && big.tile(40, 3).is_some() && big.tile(2, -1).is_some());
    }
}
