//! Area scaling: grow the region into the line art, or shrink it.

use ahash::AHashMap;
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

/// Grow `f` by `n` 4-connected steps into wall pixels, advancing only
/// where the strength does not drop (more than [`DARKEST_EPS`]).
pub(super) fn grow_darkest(g: &Geom, src: &Source, pass: &BitGrid, f: &mut BitGrid, n: u32) {
    let mut cache: AHashMap<usize, Box<[[u16; TILE_SIZE]; TILE_SIZE]>> = AHashMap::new();
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
