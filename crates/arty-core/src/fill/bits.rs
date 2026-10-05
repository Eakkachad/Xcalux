//! Bit tiles: one `u64` per tile row, bit `x` = pixel `x`, stored densely
//! per page tile so the flood never hashes.

use crate::tile::TILE_SIZE;

pub(super) type Bits = [u64; TILE_SIZE];
pub(super) const ZERO: Bits = [0; TILE_SIZE];
pub(super) const ONES: Bits = [!0; TILE_SIZE];

/// Page size in px and tiles. Tile slots are `ty·tw + tx`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Geom {
    pub w: i32,
    pub h: i32,
    pub tw: i32,
    pub th: i32,
}

impl Geom {
    pub fn new(w: u32, h: u32) -> Geom {
        let (w, h) = (w as i32, h as i32);
        Geom { w, h, tw: (w + 63) / 64, th: (h + 63) / 64 }
    }

    pub fn len(&self) -> usize {
        (self.tw * self.th) as usize
    }

    #[inline]
    pub fn slot(&self, tx: i32, ty: i32) -> Option<usize> {
        (tx >= 0 && ty >= 0 && tx < self.tw && ty < self.th).then(|| (ty * self.tw + tx) as usize)
    }

    #[inline]
    pub fn coord(&self, i: usize) -> (i32, i32) {
        (i as i32 % self.tw, i as i32 / self.tw)
    }

    /// Slot, row and column of a page pixel.
    #[inline]
    pub fn pixel(&self, x: i32, y: i32) -> Option<(usize, usize, usize)> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        Some((((y >> 6) * self.tw + (x >> 6)) as usize, (y & 63) as usize, (x & 63) as usize))
    }

    /// In-page columns of tile column `tx`.
    #[inline]
    pub fn cols(&self, tx: i32) -> u64 {
        let n = self.w - tx * 64;
        if n >= 64 { !0 } else { (1u64 << n) - 1 }
    }

    /// In-page rows of tile row `ty`.
    #[inline]
    pub fn rows(&self, ty: i32) -> usize {
        (self.h - ty * 64).min(64) as usize
    }

    /// The in-page pixels of a tile.
    pub fn mask(&self, tx: i32, ty: i32) -> Bits {
        let mut m = ZERO;
        let c = self.cols(tx);
        for r in &mut m[..self.rows(ty)] {
            *r = c;
        }
        m
    }

    /// The in-page tiles around `i` (itself included).
    pub fn around(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let (tx, ty) = self.coord(i);
        (-1..=1).flat_map(move |dy| (-1..=1).filter_map(move |dx| self.slot(tx + dx, ty + dy)))
    }
}

const K_ZERO: u32 = 0;
const K_ONES: u32 = 1;
const K_UNSET: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Zero,
    Ones,
    Mixed,
}

/// One bit tile per page tile: constant tiles share static storage, the
/// rest live in a pool reused across fills. A slot can also be "unset"
/// (not classified yet), which reads as zero.
#[derive(Default)]
pub(super) struct BitGrid {
    idx: Vec<u32>,
    pool: Vec<Bits>,
}

impl BitGrid {
    /// `n` slots, all zero or all unset. Keeps the pool's capacity.
    pub fn reset(&mut self, n: usize, unset: bool) {
        self.idx.clear();
        self.idx.resize(n, if unset { K_UNSET } else { K_ZERO });
        self.pool.clear();
    }

    #[inline]
    pub fn is_set(&self, i: usize) -> bool {
        self.idx[i] != K_UNSET
    }

    #[inline]
    pub fn get(&self, i: usize) -> &Bits {
        match self.idx[i] {
            K_ZERO | K_UNSET => &ZERO,
            K_ONES => &ONES,
            k => &self.pool[(k - 2) as usize],
        }
    }

    #[inline]
    pub fn kind(&self, i: usize) -> Kind {
        match self.idx[i] {
            K_ZERO | K_UNSET => Kind::Zero,
            K_ONES => Kind::Ones,
            _ => Kind::Mixed,
        }
    }

    pub fn any(&self, i: usize) -> bool {
        match self.kind(i) {
            Kind::Zero => false,
            Kind::Ones => true,
            Kind::Mixed => self.get(i).iter().any(|&r| r != 0),
        }
    }

    /// Writable bits, copied out of shared constant storage first.
    #[inline]
    pub fn get_mut(&mut self, i: usize) -> &mut Bits {
        let k = self.idx[i];
        if k < 2 || k == K_UNSET {
            self.pool.push(if k == K_ONES { ONES } else { ZERO });
            self.idx[i] = self.pool.len() as u32 + 1;
        }
        let k = self.idx[i];
        &mut self.pool[(k - 2) as usize]
    }

    /// Store `b`, sharing the constant tiles.
    pub fn set(&mut self, i: usize, b: &Bits) {
        if b.iter().all(|&r| r == 0) {
            self.idx[i] = K_ZERO;
        } else if b.iter().all(|&r| r == !0) {
            self.idx[i] = K_ONES;
        } else {
            *self.get_mut(i) = *b;
        }
    }

    /// The bits of the 8 neighbours that touch tile `(tx, ty)`. Off-page
    /// and unset tiles read as zero.
    pub fn nb(&self, g: &Geom, tx: i32, ty: i32) -> Nb {
        let get = |x: i32, y: i32| g.slot(x, y).map(|i| (self.kind(i), self.get(i)));
        let col = |t: Option<(Kind, &Bits)>, bit: u32| -> u128 {
            match t {
                None | Some((Kind::Zero, _)) => 0,
                Some((Kind::Ones, _)) => ((1u128 << 64) - 1) << 1,
                Some((Kind::Mixed, b)) => {
                    let mut c = 0u128;
                    for (y, r) in b.iter().enumerate() {
                        c |= (((r >> bit) & 1) as u128) << (y + 1);
                    }
                    c
                }
            }
        };
        let px = |t: Option<(Kind, &Bits)>, row: usize, bit: u32| t.map_or(0, |(_, b)| ((b[row] >> bit) & 1) as u128);
        Nb {
            above: get(tx, ty - 1).map_or(0, |(_, b)| b[63]),
            below: get(tx, ty + 1).map_or(0, |(_, b)| b[0]),
            left: col(get(tx - 1, ty), 63) | px(get(tx - 1, ty - 1), 63, 63) | px(get(tx - 1, ty + 1), 0, 63) << 65,
            right: col(get(tx + 1, ty), 0) | px(get(tx + 1, ty - 1), 63, 0) | px(get(tx + 1, ty + 1), 0, 0) << 65,
        }
    }
}

/// The pixels just outside a tile: the row above and below, and the
/// column left and right (bit `y + 1` = row `y`, for `y` in `-1..=64`).
pub(super) struct Nb {
    pub above: u64,
    pub below: u64,
    pub left: u128,
    pub right: u128,
}

#[inline]
fn hd(row: u64, l: u64, r: u64) -> u64 {
    row | (row << 1) | l | (row >> 1) | (r << 63)
}

/// One step of 4- or 8-connected dilation of `t`.
pub(super) fn dilate(t: &Bits, nb: &Nb, conn8: bool) -> Bits {
    let bit = |c: u128, k: usize| ((c >> k) & 1) as u64;
    let mut out = ZERO;
    for y in 0..TILE_SIZE {
        let up = if y == 0 { nb.above } else { t[y - 1] };
        let dn = if y == TILE_SIZE - 1 { nb.below } else { t[y + 1] };
        let mid = hd(t[y], bit(nb.left, y + 1), bit(nb.right, y + 1));
        out[y] = if conn8 {
            mid | hd(up, bit(nb.left, y), bit(nb.right, y)) | hd(dn, bit(nb.left, y + 2), bit(nb.right, y + 2))
        } else {
            mid | up | dn
        };
    }
    out
}

/// The runs of `m` that contain a bit of `s`.
#[inline]
pub(super) fn hfill(m: u64, s: u64) -> u64 {
    let s = s & m;
    if s == 0 {
        return 0;
    }
    if m == !0 {
        return !0;
    }
    // Towards bit 63: the carry of `m + s` runs from each seed to the top
    // of its run.
    let up = ((m.wrapping_add(s) ^ m ^ s) | s) & m;
    // Towards bit 0: Kogge–Stone prefix propagation.
    let (mut g, mut p) = (s, m);
    g |= p & (g >> 1);
    p &= p >> 1;
    g |= p & (g >> 2);
    p &= p >> 2;
    g |= p & (g >> 4);
    p &= p >> 4;
    g |= p & (g >> 8);
    p &= p >> 8;
    g |= p & (g >> 16);
    p &= p >> 16;
    g |= p & (g >> 32);
    up | g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hfill_ref(m: u64, s: u64) -> u64 {
        let mut out = 0;
        let mut x = 0;
        while x < 64 {
            if m >> x & 1 == 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < 64 && m >> x & 1 == 1 {
                x += 1;
            }
            let run = if x - start == 64 { !0 } else { ((1u64 << (x - start)) - 1) << start };
            if run & s != 0 {
                out |= run;
            }
        }
        out
    }

    #[test]
    fn hfill_matches_scalar_runs() {
        let mut v = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            v ^= v << 13;
            v ^= v >> 7;
            v ^= v << 17;
            v
        };
        for _ in 0..20_000 {
            let m = next() | next();
            let s = next() & next() & next();
            assert_eq!(hfill(m, s), hfill_ref(m, s), "m {m:064b} s {s:064b}");
        }
        assert_eq!(hfill(!0, 1 << 40), !0);
        assert_eq!(hfill(!0 >> 1, 1 << 62), !0 >> 1);
        assert_eq!(hfill(0b1110_0111, 0b1000_0000), 0b1110_0000);
    }

    #[test]
    fn dilate_reads_neighbour_pixels() {
        let mut g = BitGrid::default();
        let geom = Geom::new(192, 192);
        g.reset(geom.len(), false);
        // Pixel (63, 63) of tile (0, 0): diagonal to tile (1, 1)'s (0, 0).
        g.get_mut(0)[63] = 1 << 63;
        let nb = g.nb(&geom, 1, 1);
        let t = *g.get(4);
        assert_eq!(dilate(&t, &nb, false)[0], 0, "4-connected misses the corner");
        assert_eq!(dilate(&t, &nb, true)[0], 1, "8-connected reaches it");
        let nb = g.nb(&geom, 1, 0);
        assert_eq!(dilate(g.get(1), &nb, false)[63], 1);
    }
}
