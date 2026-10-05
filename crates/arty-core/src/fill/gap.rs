//! Gap closing: walls dilated by a disk, then competitive regrowth of the
//! clicked core against every other core inside the dilation ring.

use rayon::prelude::*;

use super::bits::{BitGrid, Bits, Geom, Kind, ZERO};

/// `⌊√(r² − d²)⌋` for `d` in `0..=r`: the half-width of a disk row.
fn disk_rows(r: u32) -> Vec<u32> {
    (0..=r).map(|d| (((r * r - d * d) as f64).sqrt() + 1e-9).floor() as u32).collect()
}

/// The walls (on-page pixels that do not pass) of the 3×3 tiles around
/// `(tx, ty)`, `[dy][dx]`. `None` when there are none.
fn walls3(g: &Geom, pass: &BitGrid, tx: i32, ty: i32) -> Option<[[Bits; 3]; 3]> {
    let mut out = [[ZERO; 3]; 3];
    let mut any = false;
    for (dy, row) in out.iter_mut().enumerate() {
        for (dx, w) in row.iter_mut().enumerate() {
            let (x, y) = (tx + dx as i32 - 1, ty + dy as i32 - 1);
            let Some(j) = g.slot(x, y) else { continue };
            // Every tile next to the region is classified before gap
            // closing; an unset one would read as passing.
            if !pass.is_set(j) || (pass.kind(j) == Kind::Ones) {
                continue;
            }
            let p = pass.get(j);
            let (rows, cols) = (g.rows(y), g.cols(x));
            for (o, r) in w.iter_mut().zip(p).take(rows) {
                *o = !r & cols;
                any |= *o != 0;
            }
        }
    }
    any.then_some(out)
}

/// `W′ = dilate(walls, disk r)` over tile `(tx, ty)`, `r ≤ 64`.
///
/// Each source row is dilated horizontally one px at a time (`H_k`, with
/// the left and right tiles' words carried along, exact for `k ≤ 64`);
/// a target row is the union of `H_{⌊√(r²−dy²)⌋}` of the rows `dy` away.
fn disk_dilate(walls: &[[Bits; 3]; 3], r: u32, kd: &[u32], h: &mut Vec<u64>) -> Bits {
    let r = r as usize;
    let n = 64 + 2 * r;
    let stride = r + 1;
    h.clear();
    h.resize(n * stride, 0);
    for i in 0..n {
        let gy = i as i32 - r as i32;
        let (ty, y) = (gy.div_euclid(64) + 1, gy.rem_euclid(64) as usize);
        let (mut l, mut m, mut rr) = (walls[ty as usize][0][y], walls[ty as usize][1][y], walls[ty as usize][2][y]);
        if l | m | rr == 0 {
            continue;
        }
        let row = &mut h[i * stride..(i + 1) * stride];
        row[0] = m;
        for k in 1..=r {
            if l & m & rr == !0 {
                row[k..].fill(!0);
                break;
            }
            (l, m, rr) = (
                l | (l << 1) | (l >> 1) | (m << 63),
                m | (m << 1) | (m >> 1) | (l >> 63) | (rr << 63),
                rr | (rr << 1) | (rr >> 1) | (m >> 63),
            );
            row[k] = m;
        }
    }
    let mut out = ZERO;
    for (y, o) in out.iter_mut().enumerate() {
        let mut acc = 0u64;
        for d in 0..=2 * r {
            let dy = d.abs_diff(r);
            acc |= h[(y + d) * stride + kd[dy] as usize];
            if acc == !0 {
                break;
            }
        }
        *o = acc;
    }
    out
}

/// `W′` for each of `tiles` (only tiles with walls nearby are returned).
pub(super) fn wall_dilation(g: &Geom, pass: &BitGrid, tiles: &[u32], r: u32) -> Vec<(u32, Bits)> {
    let kd = disk_rows(r);
    tiles
        .par_iter()
        .map_init(Vec::new, |h, &i| {
            let (tx, ty) = g.coord(i as usize);
            let walls = walls3(g, pass, tx, ty)?;
            Some((i, disk_dilate(&walls, r, &kd, h)))
        })
        .flatten()
        .collect()
}

/// Synchronous steps per regrowth round. A round runs them on a tile plus
/// a `HALO`-px border of its neighbours as they were at the round's start:
/// a frozen border is wrong by at most one px per step, so after `HALO`
/// steps the tile itself is still exact.
const HALO: usize = 16;
/// Window rows, and bits per row (`HALO` | 64 | `HALO`).
const WIN: usize = 64 + 2 * HALO;

type Win = [u128; WIN];

/// Tile `(tx, ty)` and its `HALO`-px border from `grid`; off-page and
/// unset tiles read as zero.
fn window(g: &Geom, grid: &BitGrid, tx: i32, ty: i32) -> Win {
    let mut w = [0u128; WIN];
    let word = |x: i32, y: i32, row: usize| g.slot(x, y).map_or(0, |i| grid.get(i)[row]);
    for (r, out) in w.iter_mut().enumerate() {
        let gy = r as i32 - HALO as i32;
        let (y, row) = (ty + gy.div_euclid(64), gy.rem_euclid(64) as usize);
        let left = (word(tx - 1, y, row) >> (64 - HALO)) as u128;
        let mid = (word(tx, y, row) as u128) << HALO;
        let right = ((word(tx + 1, y, row) & ((1 << HALO) - 1)) as u128) << (64 + HALO);
        *out = left | mid | right;
    }
    w
}

/// Up to `HALO` synchronous steps of tile `i`: its new `(f, o)` bits, or
/// `None` when nothing grows.
fn grow_round(g: &Geom, t: &BitGrid, f: &BitGrid, o: &BitGrid, i: usize) -> Option<(Bits, Bits)> {
    let (ti, fi, oi) = (t.get(i), f.get(i), o.get(i));
    if (0..64).all(|y| ti[y] & !fi[y] & !oi[y] == 0) {
        return None;
    }
    let (tx, ty) = g.coord(i);
    let tw = window(g, t, tx, ty);
    let (mut fw, mut ow) = (window(g, f, tx, ty), window(g, o, tx, ty));
    let mut grew = false;
    for _ in 0..HALO {
        let (pf, po) = (fw, ow);
        let mut changed = false;
        for r in 0..WIN {
            let free = tw[r] & !pf[r] & !po[r];
            if free == 0 {
                continue;
            }
            let near = |w: &Win| {
                let up = if r > 0 { w[r - 1] } else { 0 };
                let dn = if r + 1 < WIN { w[r + 1] } else { 0 };
                w[r] | (w[r] << 1) | (w[r] >> 1) | up | dn
            };
            let (a, b) = (near(&pf) & free, near(&po) & free);
            fw[r] |= a & !b;
            ow[r] |= b;
            changed |= a | b != 0;
        }
        if !changed {
            break;
        }
        grew = true;
    }
    if !grew {
        return None;
    }
    let (mut nf, mut no) = (ZERO, ZERO);
    for y in 0..64 {
        nf[y] = (fw[y + HALO] >> HALO) as u64;
        no[y] = (ow[y + HALO] >> HALO) as u64;
    }
    (nf != *fi || no != *oi).then_some((nf, no))
}

/// Grow `f` and `o` (disjoint) one 4-connected step at a time into the
/// free pixels of `t` in the `ring` tiles; a pixel both reach in the same
/// step goes to `o`. `active` is scratch flags, all clear, one per slot.
///
/// This is the geodesic Voronoi split of the ring between the two seeds:
/// a pixel joins `f` exactly when it is strictly nearer to it.
pub(super) fn regrow(g: &Geom, t: &BitGrid, f: &mut BitGrid, o: &mut BitGrid, ring: &[u32], active: &mut [u8]) {
    const RING: u8 = 1;
    const NEXT: u8 = 2;
    /// Below this many tiles a round runs on the calling thread.
    const PAR_MIN: usize = 8;
    for &i in ring {
        active[i as usize] |= RING;
    }
    let mut todo: Vec<u32> = ring.to_vec();
    let mut round: Vec<(u32, Bits, Bits)> = Vec::new();
    while !todo.is_empty() {
        round.clear();
        let one = |&i: &u32| grow_round(g, t, f, o, i as usize).map(|(nf, no)| (i, nf, no));
        if todo.len() < PAR_MIN {
            round.extend(todo.iter().filter_map(one));
        } else {
            round.par_extend(todo.par_iter().filter_map(one));
        }
        todo.clear();
        for (i, nf, no) in &round {
            f.set(*i as usize, nf);
            o.set(*i as usize, no);
            // Growth reaches up to HALO px into any neighbour, corners too.
            for j in g.around(*i as usize) {
                if active[j] & (RING | NEXT) == RING {
                    active[j] |= NEXT;
                    todo.push(j as u32);
                }
            }
        }
        for &j in &todo {
            active[j as usize] &= !NEXT;
        }
    }
    for &i in ring {
        active[i as usize] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill::bits::dilate;

    #[test]
    fn disk_dilation_matches_brute_force() {
        // A few wall pixels spread over the 3×3 neighbourhood.
        let mut walls = [[ZERO; 3]; 3];
        let pts: [(i32, i32); 8] = [(3, 10), (70, 2), (130, 140), (64, 64), (0, 127), (191, 191), (100, 34), (20, 100)];
        for (x, y) in pts {
            let (tx, ty) = (x / 64, y / 64);
            walls[ty as usize][tx as usize][(y - ty * 64) as usize] |= 1 << (x - tx * 64);
        }
        for r in [1u32, 3, 8, 16, 40, 64] {
            let kd = disk_rows(r);
            let mut h = Vec::new();
            let got = disk_dilate(&walls, r, &kd, &mut h);
            for y in 0..64i32 {
                for x in 0..64i32 {
                    let (px, py) = (x + 64, y + 64);
                    let mut want = false;
                    for (wy, row) in walls.iter().enumerate() {
                        for (wx, w) in row.iter().enumerate() {
                            for (yy, bits) in w.iter().enumerate() {
                                for xx in 0..64 {
                                    if bits >> xx & 1 == 1 {
                                        let (qx, qy) = (wx as i32 * 64 + xx, wy as i32 * 64 + yy as i32);
                                        let (dx, dy) = ((qx - px).unsigned_abs(), (qy - py).unsigned_abs());
                                        want |= dy <= r && dx <= kd[dy as usize];
                                    }
                                }
                            }
                        }
                    }
                    assert_eq!(got[y as usize] >> x & 1 == 1, want, "r {r} at ({x}, {y})");
                }
            }
        }
    }

    // ----- the single-step reference --------------------------------------

    /// One synchronous regrowth step of tile `i`: the new `(f, o)` bits, or
    /// `None` when nothing grows.
    fn grow_tile(g: &Geom, t: &BitGrid, f: &BitGrid, o: &BitGrid, i: usize) -> Option<(Bits, Bits)> {
        let (ti, fi, oi) = (t.get(i), f.get(i), o.get(i));
        let mut free = ZERO;
        let mut any = false;
        for y in 0..64 {
            free[y] = ti[y] & !fi[y] & !oi[y];
            any |= free[y] != 0;
        }
        if !any {
            return None;
        }
        let (tx, ty) = g.coord(i);
        let df = dilate(fi, &f.nb(g, tx, ty), false);
        let dof = dilate(oi, &o.nb(g, tx, ty), false);
        let (mut nf, mut no) = (*fi, *oi);
        let mut changed = false;
        for y in 0..64 {
            let (a, b) = (df[y] & free[y], dof[y] & free[y]);
            nf[y] |= a & !b;
            no[y] |= b;
            changed |= a | b != 0;
        }
        changed.then_some((nf, no))
    }

    /// Grow `f` and `o` (disjoint) one 4-connected step at a time into the
    /// free pixels of `t` in the `ring` tiles; a pixel both reach in the same
    /// step goes to `o`. `active` is scratch flags, all clear, one per slot.
    ///
    /// This is the geodesic Voronoi split of the ring between the two seeds:
    /// a pixel joins `f` exactly when it is strictly nearer to it.
    fn regrow_by_steps(g: &Geom, t: &BitGrid, f: &mut BitGrid, o: &mut BitGrid, ring: &[u32], active: &mut [u8]) {
        const RING: u8 = 1;
        const NEXT: u8 = 2;
        /// Below this many tiles a step runs on the calling thread.
        const PAR_MIN: usize = 24;
        for &i in ring {
            active[i as usize] |= RING;
        }
        let mut todo: Vec<u32> = ring.to_vec();
        let mut step: Vec<(u32, Bits, Bits)> = Vec::new();
        while !todo.is_empty() {
            step.clear();
            let one = |&i: &u32| grow_tile(g, t, f, o, i as usize).map(|(nf, no)| (i, nf, no));
            if todo.len() < PAR_MIN {
                step.extend(todo.iter().filter_map(one));
            } else {
                step.par_extend(todo.par_iter().filter_map(one));
            }
            todo.clear();
            for (i, nf, no) in &step {
                let i = *i as usize;
                // What changed decides which neighbours can grow next step.
                let (of, oo) = (f.get(i), o.get(i));
                let mut d = ZERO;
                for y in 0..64 {
                    d[y] = (nf[y] ^ of[y]) | (no[y] ^ oo[y]);
                }
                let (left, right) = d.iter().fold((0, 0), |(l, r), row| (l | (row & 1), r | (row >> 63)));
                let (tx, ty) = g.coord(i);
                let touched = [
                    Some(i),
                    (d[0] != 0).then(|| g.slot(tx, ty - 1)).flatten(),
                    (d[63] != 0).then(|| g.slot(tx, ty + 1)).flatten(),
                    (left != 0).then(|| g.slot(tx - 1, ty)).flatten(),
                    (right != 0).then(|| g.slot(tx + 1, ty)).flatten(),
                ];
                for j in touched.into_iter().flatten() {
                    if active[j] & (RING | NEXT) == RING {
                        active[j] |= NEXT;
                        todo.push(j as u32);
                    }
                }
            }
            for (i, nf, no) in &step {
                f.set(*i as usize, nf);
                o.set(*i as usize, no);
            }
            for &j in &todo {
                active[j as usize] &= !NEXT;
            }
        }
        for &i in ring {
            active[i as usize] = 0;
        }
    }

    #[test]
    fn regrow_rounds_match_single_steps() {
        let mut v = 0x1234_5678_9ABC_DEF1u64;
        let mut next = move || {
            v ^= v << 13;
            v ^= v >> 7;
            v ^= v << 17;
            v
        };
        for case in 0..6 {
            let g = Geom::new([300, 197, 640][case % 3], [260, 130, 70][case % 3]);
            let n = g.len();
            // T: mostly open with random walls; F and O: sparse seeds in T.
            let (mut t, mut f, mut o) = (BitGrid::default(), BitGrid::default(), BitGrid::default());
            t.reset(n, false);
            f.reset(n, false);
            o.reset(n, false);
            for i in 0..n {
                let (tx, ty) = g.coord(i);
                let m = g.mask(tx, ty);
                let (mut tb, mut fb, mut ob) = (ZERO, ZERO, ZERO);
                for y in 0..64 {
                    tb[y] = !(next() & next()) & m[y];
                    let seeds = next() & next() & next() & next() & next() & next() & tb[y];
                    fb[y] = seeds & next();
                    ob[y] = seeds & !fb[y] & next();
                }
                t.set(i, &tb);
                f.set(i, &fb);
                o.set(i, &ob);
            }
            let ring: Vec<u32> = (0..n as u32).collect();
            let (mut f2, mut o2) = (BitGrid::default(), BitGrid::default());
            f2.reset(n, false);
            o2.reset(n, false);
            for i in 0..n {
                f2.set(i, f.get(i));
                o2.set(i, o.get(i));
            }
            let mut flags = vec![0u8; n];
            regrow(&g, &t, &mut f, &mut o, &ring, &mut flags);
            regrow_by_steps(&g, &t, &mut f2, &mut o2, &ring, &mut flags);
            let mut grown = 0;
            for i in 0..n {
                assert_eq!(f.get(i), f2.get(i), "case {case} tile {i}: f");
                assert_eq!(o.get(i), o2.get(i), "case {case} tile {i}: o");
                grown += f.get(i).iter().map(|r| r.count_ones()).sum::<u32>();
            }
            assert!(grown > 1000, "case {case}: the test grows something");
            assert!(flags.iter().all(|&a| a == 0));
        }
    }
}
