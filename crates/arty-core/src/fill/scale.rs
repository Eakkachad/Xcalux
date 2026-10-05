//! Area scaling: grow the region into the line art, or shrink it.

use rayon::prelude::*;

use crate::fix15::ONE;
use crate::tile::{TILE_SIZE, TileCoord};

use super::bits::{BitGrid, Bits, Geom, Kind, ZERO, dilate};
use super::source::{Source, with_tls};

/// Strength may drop this much per step and still count as "darker"
/// (absorbs noise on a line's plateau).
const DARKEST_EPS: u32 = ONE / 32;

/// On-page pixels of tile `i` that do not pass.
fn walls(g: &Geom, pass: &BitGrid, i: usize) -> Option<Bits> {
    if !pass.is_set(i) || pass.kind(i) == Kind::Ones {
        return None;
    }
    let (tx, ty) = g.coord(i);
    let (p, cols) = (pass.get(i), g.cols(tx));
    let mut out = ZERO;
    let mut any = false;
    for (o, r) in out.iter_mut().zip(p).take(g.rows(ty)) {
        *o = !r & cols;
        any |= *o != 0;
    }
    any.then_some(out)
}

/// The tiles with bits in `f`, plus their 8 neighbours.
fn with_halo(g: &Geom, f: &BitGrid, seen: &mut [u8]) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 0..g.len() {
        if f.any(i) {
            for j in g.around(i) {
                if seen[j] == 0 {
                    seen[j] = 1;
                    out.push(j as u32);
                }
            }
        }
    }
    for &j in &out {
        seen[j as usize] = 0;
    }
    out
}

/// Grow `f` by `n` steps into wall pixels, alternating 4- and
/// 8-connected steps (an octagonal distance).
pub(super) fn grow_plain(g: &Geom, pass: &BitGrid, f: &mut BitGrid, n: u32, seen: &mut [u8]) {
    let mut todo = with_halo(g, f, seen);
    for step in 0..n {
        let conn8 = step % 2 == 1;
        let changed: Vec<(u32, Bits)> = todo
            .par_iter()
            .filter_map(|&i| {
                let i = i as usize;
                let w = walls(g, pass, i)?;
                let (tx, ty) = g.coord(i);
                let fi = f.get(i);
                let d = dilate(fi, &f.nb(g, tx, ty), conn8);
                let mut out = *fi;
                let mut grew = false;
                for y in 0..TILE_SIZE {
                    let add = d[y] & w[y] & !fi[y];
                    out[y] |= add;
                    grew |= add != 0;
                }
                grew.then_some((i as u32, out))
            })
            .collect();
        if changed.is_empty() {
            return;
        }
        todo.clear();
        for (i, b) in &changed {
            f.set(*i as usize, b);
            for j in g.around(*i as usize) {
                if seen[j] == 0 {
                    seen[j] = 1;
                    todo.push(j as u32);
                }
            }
        }
        for &j in &todo {
            seen[j as usize] = 0;
        }
    }
}

/// Strength of every pixel of a tile, `[y][x]`.
pub(super) type Strengths = Box<[[u16; TILE_SIZE]; TILE_SIZE]>;

/// Whether `q` is a pixel a darkest-growth step may take: a classified wall
/// pixel not in `f`.
#[inline]
fn candidate(pass: &BitGrid, f: &BitGrid, (qi, qr, qc): (usize, usize, usize)) -> bool {
    let bit = 1u64 << qc;
    pass.is_set(qi) && pass.get(qi)[qr] & bit == 0 && f.get(qi)[qr] & bit == 0
}

/// Grow `f` by `n` 4-connected steps into wall pixels, advancing only
/// where the strength does not drop (more than [`DARKEST_EPS`]).
///
/// The walk itself is serial, but the strengths it reads are computed
/// first, in parallel: each step collects the tiles of its frontier pixels
/// that have a candidate neighbour, and of those neighbours, and builds
/// the ones not seen yet (a composite per tile for `AllVisible`). Returns
/// them, per tile slot, for the antialiasing pass to reuse.
pub(super) fn grow_darkest(g: &Geom, src: &Source, pass: &BitGrid, f: &mut BitGrid, n: u32) -> Vec<Option<Strengths>> {
    let mut store: Vec<Option<Strengths>> = (0..g.len()).map(|_| None).collect();
    let mut wanted = vec![false; g.len()];
    // Start from the region's pixels that have a 4-neighbour outside it.
    let mut frontier: Vec<(i32, i32)> = Vec::new();
    for i in 0..g.len() {
        if !f.any(i) {
            continue;
        }
        let (tx, ty) = g.coord(i);
        let fi = f.get(i);
        let nb = f.nb(g, tx, ty);
        for y in 0..TILE_SIZE {
            let up = if y == 0 { nb.above } else { fi[y - 1] };
            let dn = if y == TILE_SIZE - 1 { nb.below } else { fi[y + 1] };
            let l = ((nb.left >> (y + 1)) & 1) as u64;
            let r = ((nb.right >> (y + 1)) & 1) as u64;
            let inner = fi[y] & up & dn & ((fi[y] << 1) | l) & ((fi[y] >> 1) | (r << 63));
            let mut edge = fi[y] & !inner;
            while edge != 0 {
                let x = edge.trailing_zeros() as i32;
                edge &= edge - 1;
                frontier.push((tx * 64 + x, ty * 64 + y as i32));
            }
        }
    }
    let nbrs = |(px, py): (i32, i32)| [(px - 1, py), (px + 1, py), (px, py - 1), (px, py + 1)];
    let mut next = Vec::new();
    let mut want: Vec<u32> = Vec::new();
    for _ in 0..n {
        // The strengths this step reads: `f` only grows within a step, so
        // the candidates as of its start are a superset.
        want.clear();
        for &p in &frontier {
            let Some((pi, ..)) = g.pixel(p.0, p.1) else { continue };
            for (qx, qy) in nbrs(p) {
                let Some(q) = g.pixel(qx, qy) else { continue };
                if candidate(pass, f, q) {
                    for i in [pi, q.0] {
                        if !wanted[i] {
                            wanted[i] = true;
                            want.push(i as u32);
                        }
                    }
                }
            }
        }
        let built: Vec<(u32, Strengths)> = want
            .par_iter()
            .map(|&i| {
                let (tx, ty) = g.coord(i as usize);
                let mut s = Box::new([[0u16; TILE_SIZE]; TILE_SIZE]);
                with_tls(|tls| src.strengths(TileCoord::new(tx, ty), tls, &mut s));
                (i, s)
            })
            .collect();
        for (i, s) in built {
            store[i as usize] = Some(s);
        }
        let strength = |(i, y, x): (usize, usize, usize)| store[i].as_ref().expect("strengths built")[y][x] as u32;
        for &p in &frontier {
            let Some(pp) = g.pixel(p.0, p.1) else { continue };
            let mut sp = None;
            for (qx, qy) in nbrs(p) {
                let Some(q) = g.pixel(qx, qy) else { continue };
                if !candidate(pass, f, q) {
                    continue;
                }
                let sp = *sp.get_or_insert_with(|| strength(pp));
                if strength(q) + DARKEST_EPS >= sp {
                    f.get_mut(q.0)[q.1] |= 1u64 << q.2;
                    next.push((qx, qy));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        std::mem::swap(&mut frontier, &mut next);
        next.clear();
    }
    store
}

/// Remove from `f` every pixel within `n` steps of an on-page pixel
/// outside it (the page border does not erode). `aux` is scratch.
pub(super) fn shrink(g: &Geom, f: &mut BitGrid, aux: &mut BitGrid, n: u32, seen: &mut [u8]) {
    aux.reset(g.len(), false);
    for j in with_halo(g, f, seen) {
        let j = j as usize;
        let (tx, ty) = g.coord(j);
        let (m, fj) = (g.mask(tx, ty), f.get(j));
        let mut out = ZERO;
        for y in 0..TILE_SIZE {
            out[y] = m[y] & !fj[y];
        }
        aux.set(j, &out);
    }
    let tiles: Vec<u32> = (0..g.len() as u32).filter(|&i| f.any(i as usize)).collect();
    for step in 0..n {
        let conn8 = step % 2 == 1;
        let eaten: Vec<(u32, Bits)> = tiles
            .par_iter()
            .filter_map(|&i| {
                let i = i as usize;
                let (tx, ty) = g.coord(i);
                let d = dilate(aux.get(i), &aux.nb(g, tx, ty), conn8);
                let fi = f.get(i);
                let mut out = ZERO;
                let mut any = false;
                for y in 0..TILE_SIZE {
                    out[y] = d[y] & fi[y];
                    any |= out[y] != 0;
                }
                any.then_some((i as u32, out))
            })
            .collect();
        if eaten.is_empty() {
            return;
        }
        for (i, d) in &eaten {
            let i = *i as usize;
            let (mut fi, mut ai) = (*f.get(i), *aux.get(i));
            for y in 0..TILE_SIZE {
                fi[y] &= !d[y];
                ai[y] |= d[y];
            }
            f.set(i, &fi);
            aux.set(i, &ai);
        }
    }
}

#[cfg(test)]
mod tests {
    use ahash::AHashMap;

    use super::*;
    use crate::document::Document;
    use crate::fill::FillRef;

    /// The walk as first written: strengths built lazily, one tile at a
    /// time, on the calling thread.
    fn grow_darkest_lazy(g: &Geom, src: &Source, pass: &BitGrid, f: &mut BitGrid, n: u32) {
        let mut cache: AHashMap<usize, Strengths> = AHashMap::new();
        let mut strength = |i: usize, y: usize, x: usize| -> u16 {
            cache
                .entry(i)
                .or_insert_with(|| {
                    let (tx, ty) = g.coord(i);
                    let mut s = Box::new([[0u16; TILE_SIZE]; TILE_SIZE]);
                    with_tls(|tls| src.strengths(TileCoord::new(tx, ty), tls, &mut s));
                    s
                })
                .as_ref()[y][x]
        };
        let inside = |f: &BitGrid, qx: i32, qy: i32| g.pixel(qx, qy).is_some_and(|(j, r, c)| f.get(j)[r] >> c & 1 == 1);
        let mut frontier: Vec<(i32, i32)> = Vec::new();
        for py in 0..g.h {
            for px in 0..g.w {
                if inside(f, px, py)
                    && [(px - 1, py), (px + 1, py), (px, py - 1), (px, py + 1)].iter().any(|&(qx, qy)| !inside(f, qx, qy))
                {
                    frontier.push((px, py));
                }
            }
        }
        let mut next = Vec::new();
        for _ in 0..n {
            for &(px, py) in &frontier {
                let Some((pi, pr, pc)) = g.pixel(px, py) else { continue };
                let sp = strength(pi, pr, pc) as u32;
                for (qx, qy) in [(px - 1, py), (px + 1, py), (px, py - 1), (px, py + 1)] {
                    let Some((qi, qr, qc)) = g.pixel(qx, qy) else { continue };
                    let bit = 1u64 << qc;
                    if pass.get(qi)[qr] & bit != 0 || f.get(qi)[qr] & bit != 0 || !pass.is_set(qi) {
                        continue;
                    }
                    if strength(qi, qr, qc) as u32 + DARKEST_EPS >= sp {
                        f.get_mut(qi)[qr] |= bit;
                        next.push((qx, qy));
                    }
                }
            }
            if next.is_empty() {
                return;
            }
            std::mem::swap(&mut frontier, &mut next);
            next.clear();
        }
    }

    #[test]
    fn darkest_growth_matches_the_lazy_walk() {
        // Soft lines of varying strength over three tiles each way.
        let (w, h) = (170u32, 150u32);
        let mut doc = Document::new(w, h, 600);
        let id = doc.active();
        let (grid, _) = doc.paint_target(id).unwrap();
        let mut rng = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for _ in 0..40 {
            let (cx, cy) = ((next() % w as u64) as f32, (next() % h as u64) as f32);
            let (dx, dy) = ((next() % 100) as f32 / 50.0 - 1.0, (next() % 100) as f32 / 50.0 - 1.0);
            let peak = 0.3 + (next() % 70) as f32 / 100.0;
            for t in -60..60 {
                let (x0, y0) = (cx + dx * t as f32, cy + dy * t as f32);
                for oy in -3..=3 {
                    for ox in -3..=3 {
                        let (x, y) = (x0 as i32 + ox, y0 as i32 + oy);
                        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                            continue;
                        }
                        let d = ((ox * ox + oy * oy) as f32).sqrt();
                        let a = (peak * (1.0 - d / 3.5)).max(0.0);
                        let c = TileCoord::from_pixel(x, y);
                        let (tx0, ty0) = c.origin();
                        let px = &mut grid.get_mut_or_create(c)[(y - ty0) as usize][(x - tx0) as usize];
                        let v = (a * ONE as f32) as u16;
                        if v > px[3] {
                            *px = [0, 0, 0, v];
                        }
                    }
                }
            }
        }
        let g = Geom::new(w, h);
        let src = with_tls(|tls| Source::new(&doc, FillRef::Active, (0, 0), 0, tls));
        let mut pass = BitGrid::default();
        pass.reset(g.len(), true);
        let all: Vec<u32> = (0..g.len() as u32).collect();
        super::super::classify(&src, &mut pass, &all);
        for n in [1, 2, 5, 10] {
            let (mut a, mut b) = (BitGrid::default(), BitGrid::default());
            a.reset(g.len(), false);
            b.reset(g.len(), false);
            for i in 0..g.len() {
                a.set(i, pass.get(i));
                b.set(i, pass.get(i));
            }
            let store = grow_darkest(&g, &src, &pass, &mut a, n);
            assert!(store.iter().any(Option::is_some));
            grow_darkest_lazy(&g, &src, &pass, &mut b, n);
            let grown: u32 =
                (0..g.len()).map(|i| (0..TILE_SIZE).map(|y| (a.get(i)[y] & !pass.get(i)[y]).count_ones()).sum::<u32>()).sum();
            assert!(grown > 0, "n {n}: nothing grew");
            for i in 0..g.len() {
                assert_eq!(a.get(i), b.get(i), "n {n}, tile {i}");
            }
        }
    }
}
