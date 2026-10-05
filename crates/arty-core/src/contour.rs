//! Selection outlines for the marching ants: marching squares at iso 127.5,
//! linked across tiles and simplified into levels of detail.
//!
//! Samples sit at pixel centres; pixels off the page count as 0, so every
//! outline closes at the page border. A crossing is linearly interpolated
//! along its cell edge. In a cell whose four samples are all 0 or 255 the
//! outline runs through the cell centre instead of cutting the corner, so a
//! hard-edged selection outlines its exact pixel edges (select-all is the
//! page rectangle, four segments). A saddle is resolved by the cell average.
//!
//! Work is split into one task per 64×64 block of cells (rayon). A block
//! whose 2×2 tiles are all unselected or all fully selected is skipped; one
//! that mixes only those two kinds emits its tile-seam runs directly. Each
//! task links its pieces by cell edge, then the open ends are stitched across
//! tile seams by a u64 grid-edge id.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ahash::AHashMap;
use rayon::prelude::*;

use crate::geom::Pt;
use crate::selection::{MaskPixels, MaskView, Selection, full_mask};
use crate::tile::{TILE_SIZE, TileCoord};

/// Douglas–Peucker tolerance of each level of detail, in document px.
pub const LOD_TOL: [f32; 3] = [0.25, 2.0, 8.0];

/// Marching-squares pieces allowed per extraction before it falls back to a
/// 2× then 4× mean-pooled mask (a piece is one cell's part of an outline, so
/// this bounds the finest-level segment count from above).
pub const SEGMENT_BUDGET: usize = 2_000_000;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polyline {
    pub pts: Vec<Pt>,
    pub closed: bool,
}

impl Polyline {
    /// Segments drawn for this polyline.
    pub fn segments(&self) -> usize {
        match self.pts.len() {
            0 | 1 => 0,
            n if self.closed => n,
            n => n - 1,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Contours {
    /// One outline set per [`LOD_TOL`] entry, finest first.
    pub lods: [Vec<Polyline>; 3],
    /// Segments at the finest level.
    pub segments: usize,
    /// The segment budget ran out; the outline is incomplete.
    pub truncated: bool,
    /// Mean-pooling factor the outline was extracted at: 1 (full
    /// resolution), 2 or 4 after a budget overflow; 0 for `default()`.
    pub pool: u32,
}

/// Outlines of `sel` on a `w`×`h` page (off-page counts as unselected).
pub fn extract(sel: &Selection, w: u32, h: u32) -> Contours {
    extract_with_budget(sel, w, h, SEGMENT_BUDGET)
}

/// [`extract`] with an explicit piece budget (see [`SEGMENT_BUDGET`]).
pub fn extract_with_budget(sel: &Selection, w: u32, h: u32, budget: usize) -> Contours {
    let mut raw = trace(sel, w, h, budget, false);
    let mut pool = 1;
    if raw.over {
        // Pooling by k roughly divides an outline's pieces by k. Skip 2× when
        // the blocks traced so far say it would overflow too.
        let estimate = raw.pieces as f64 * raw.tasks as f64 / raw.traced.max(1) as f64;
        let ks: &[u32] = if estimate <= 1.5 * budget as f64 { &[2, 4] } else { &[4] };
        for &k in ks {
            raw = trace(&pooled(sel, k, w, h), w.div_ceil(k), h.div_ceil(k), budget, k == 4);
            pool = k;
            if !raw.over {
                break;
            }
        }
        let s = pool as f32;
        raw.polys.par_iter_mut().for_each(|p| p.pts.iter_mut().for_each(|q| *q = [q[0] * s, q[1] * s]));
    }
    let lod0: Vec<Polyline> = raw
        .polys
        .into_par_iter()
        .map_init(DpScratch::default, |dp, mut p| {
            merge_collinear(&mut p.pts, p.closed);
            if p.closed {
                canonical_start(&mut p.pts);
            }
            simplify_in_place(p, LOD_TOL[0], dp)
        })
        .flatten()
        .collect();
    let coarse = |k: usize| {
        lod0.par_iter()
            .map_init(DpScratch::default, |dp, p| simplify(p, LOD_TOL[k], LOD_TOL[k], dp))
            .flatten()
            .collect::<Vec<_>>()
    };
    let (lod1, lod2) = (coarse(1), coarse(2));
    let segments = lod0.iter().map(Polyline::segments).sum();
    Contours { lods: [lod0, lod1, lod2], segments, truncated: raw.over, pool }
}

// ----- tracing ---------------------------------------------------------------

const T: i32 = TILE_SIZE as i32;
/// Samples per block side: the block's 64 pixels plus the next tile's first.
const S: usize = TILE_SIZE + 1;
const ISO: f32 = 127.5;
/// Local cell edges of a block: vertical (65 × 64) then horizontal (64 × 65).
const V_EDGES: usize = S * TILE_SIZE;
const EDGES: usize = 2 * V_EDGES;
const NONE: u32 = u32::MAX;

/// Tile kinds for skipping and the seam fast path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Unselected, or entirely off the page.
    Empty,
    /// Fully selected and entirely on the page.
    Full,
    Mixed,
}

fn class(sel: &Selection, c: TileCoord, w: i32, h: i32) -> Class {
    let (ox, oy) = c.origin();
    if ox < 0 || oy < 0 || ox >= w || oy >= h {
        return Class::Empty;
    }
    match sel.get(c) {
        MaskView::Empty => Class::Empty,
        MaskView::Full if ox + T <= w && oy + T <= h => Class::Full,
        _ => Class::Mixed,
    }
}

/// One cell's share of an outline, between two local cell edges.
#[derive(Clone, Copy)]
struct Piece {
    from: u16,
    to: u16,
    p0: Pt,
    /// The cell centre, for a hard-edged (all 0/255) cell.
    mid: Option<Pt>,
    p1: Pt,
}

/// A run of linked pieces that leaves the block at both ends.
struct Chain {
    start: u64,
    end: u64,
    pts: Vec<Pt>,
}

#[derive(Default)]
struct BlockOut {
    closed: Vec<Polyline>,
    open: Vec<Chain>,
}

struct Raw {
    polys: Vec<Polyline>,
    /// The budget ran out: some blocks were not traced.
    over: bool,
    /// Blocks to trace, blocks traced, and the pieces they gave.
    tasks: usize,
    traced: usize,
    pieces: usize,
}

/// Outline pieces of `sel`, linked. Past `budget` pieces the remaining
/// blocks are skipped; then the partial outline is linked only if `finish`.
fn trace(sel: &Selection, w: u32, h: u32, budget: usize, finish: bool) -> Raw {
    let (wi, hi) = (w as i32, h as i32);
    let full = |x: i32, y: i32| class(sel, TileCoord::new(x, y), wi, hi) == Class::Full;
    // A block is each tile with its right, lower and lower-right neighbours;
    // trace every block holding a selected tile, except around a full tile
    // whose 8 neighbours are full (all its blocks are full).
    let mut tasks: Vec<TileCoord> = sel
        .tiles()
        .filter(|&(c, _)| match class(sel, c, wi, hi) {
            Class::Empty => false,
            Class::Mixed => true,
            Class::Full => !(-1..=1).all(|dy| (-1..=1).all(|dx| full(c.x + dx, c.y + dy))),
        })
        .flat_map(|(c, _)| [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| TileCoord::new(c.x - dx, c.y - dy)))
        .collect();
    tasks.sort_unstable_by_key(|c| (c.y, c.x));
    tasks.dedup();

    let pieces = AtomicUsize::new(0);
    let traced = AtomicUsize::new(0);
    let over = AtomicBool::new(false);
    let outs: Vec<BlockOut> = tasks
        .par_iter()
        .map_init(Scratch::new, |scratch, &t| {
            if over.load(Ordering::Relaxed) {
                return BlockOut::default();
            }
            let out = trace_block(sel, t, wi, hi, scratch);
            traced.fetch_add(1, Ordering::Relaxed);
            if pieces.fetch_add(scratch.pieces.len(), Ordering::Relaxed) + scratch.pieces.len() > budget {
                over.store(true, Ordering::Relaxed);
            }
            out
        })
        .collect();
    let (over, traced, pieces) = (over.into_inner(), traced.into_inner(), pieces.into_inner());
    if over && !finish {
        return Raw { polys: Vec::new(), over, tasks: tasks.len(), traced, pieces };
    }

    let mut polys = Vec::new();
    let mut open = Vec::new();
    for o in outs {
        polys.extend(o.closed);
        open.extend(o.open);
    }
    stitch(open, &mut polys);
    Raw { polys, over, tasks: tasks.len(), traced, pieces }
}

struct Scratch {
    samples: Box<[[u8; S]; S]>,
    pieces: Vec<Piece>,
    /// Local edge → the piece leaving through it.
    start_at: Box<[u32; EDGES]>,
    used: Vec<bool>,
}

impl Scratch {
    fn new() -> Self {
        Self { samples: Box::new([[0; S]; S]), pieces: Vec::new(), start_at: Box::new([NONE; EDGES]), used: Vec::new() }
    }
}

/// Local index of the vertical cell edge below sample `(x, y)` (between
/// samples `(x, y)` and `(x, y + 1)`).
fn v_edge(x: usize, y: usize) -> u16 {
    (y * S + x) as u16
}

/// Local index of the horizontal cell edge right of sample `(x, y)`.
fn h_edge(x: usize, y: usize) -> u16 {
    (V_EDGES + y * TILE_SIZE + x) as u16
}

fn is_boundary(e: u16) -> bool {
    let e = e as usize;
    if e < V_EDGES {
        let x = e % S;
        x == 0 || x == TILE_SIZE
    } else {
        let y = (e - V_EDGES) / TILE_SIZE;
        y == 0 || y == TILE_SIZE
    }
}

/// Page-wide id of local edge `e` of the block whose first pixel is `(ox, oy)`.
fn global_edge(e: u16, ox: i32, oy: i32) -> u64 {
    let e = e as usize;
    let (kind, x, y) = if e < V_EDGES { (0, e % S, e / S) } else { (1, (e - V_EDGES) % TILE_SIZE, (e - V_EDGES) / TILE_SIZE) };
    let gx = (ox as i64 + x as i64 + (1 << 30)) as u64;
    let gy = (oy as i64 + y as i64 + (1 << 30)) as u64;
    (gx << 33) | (gy << 1) | kind
}

/// The (exiting edge, entering edge) pairs of one cell with corner samples
/// `[top-left, top-right, bottom-right, bottom-left]`. Edge `e` joins corner
/// `e` to corner `e + 1` (top, right, bottom, left); it is exiting when the
/// walk around the cell leaves the selection there. The outline enters the
/// cell through an exiting edge and leaves through an entering one, so each
/// crossing is the end of one piece and the start of the next.
fn cell_pairs(v: [u8; 4]) -> ([(u8, u8); 2], usize) {
    let inside = v.map(|x| x as f32 > ISO);
    let mut exits = [0u8; 2];
    let mut enters = [0u8; 2];
    let (mut nx, mut ne) = (0, 0);
    for e in 0..4 {
        match (inside[e], inside[(e + 1) % 4]) {
            (true, false) => {
                exits[nx] = e as u8;
                nx += 1;
            }
            (false, true) => {
                enters[ne] = e as u8;
                ne += 1;
            }
            _ => {}
        }
    }
    match nx {
        0 => ([(0, 0); 2], 0),
        1 => ([(exits[0], enters[0]), (0, 0)], 1),
        _ => {
            // Saddle: the centre (cell average) decides whether the two
            // selected corners connect.
            let centre = v.iter().map(|&x| x as u32).sum::<u32>() as f32 / 4.0 > ISO;
            let next = |e: u8| if centre { (e + 1) % 4 } else { (e + 3) % 4 };
            ([(exits[0], next(exits[0])), (exits[1], next(exits[1]))], 2)
        }
    }
}

fn trace_block(sel: &Selection, t: TileCoord, w: i32, h: i32, s: &mut Scratch) -> BlockOut {
    s.pieces.clear();
    let block = [(0, 0), (1, 0), (1, 1), (0, 1)].map(|(dx, dy)| class(sel, TileCoord::new(t.x + dx, t.y + dy), w, h));
    if block.iter().all(|&c| c == Class::Empty) || block.iter().all(|&c| c == Class::Full) {
        return BlockOut::default();
    }
    let (ox, oy) = t.origin();
    if block.iter().all(|&c| c != Class::Mixed) {
        seam_pieces(block, ox, oy, &mut s.pieces);
    } else {
        fill_samples(sel, t, w, h, &mut s.samples);
        cell_pieces(&s.samples, ox, oy, &mut s.pieces);
    }
    link(s, ox, oy)
}

/// The block's pixels plus the next tile's first column and row, 0 off
/// the page.
fn fill_samples(sel: &Selection, t: TileCoord, w: i32, h: i32, out: &mut [[u8; S]; S]) {
    let (ox, oy) = t.origin();
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (xs, ys) = (if dx == 0 { 0..TILE_SIZE } else { 0..1 }, if dy == 0 { 0..TILE_SIZE } else { 0..1 });
        let view = sel.get(TileCoord::new(t.x + dx, t.y + dy));
        for y in ys {
            let row = &mut out[dy as usize * TILE_SIZE + y];
            for x in xs.clone() {
                row[dx as usize * TILE_SIZE + x] = match view {
                    MaskView::Empty => 0,
                    MaskView::Full => 255,
                    MaskView::Partial(m) => m[y][x],
                };
            }
        }
    }
    // Off-page pixels are unselected.
    let (vw, vh) = ((w - ox).clamp(0, S as i32) as usize, (h - oy).clamp(0, S as i32) as usize);
    for (y, row) in out.iter_mut().enumerate() {
        if y >= vh {
            row.fill(0);
        } else {
            row[vw..].fill(0);
        }
    }
}

fn cell_pieces(sm: &[[u8; S]; S], ox: i32, oy: i32, out: &mut Vec<Piece>) {
    for cy in 0..TILE_SIZE {
        let (r0, r1) = (&sm[cy], &sm[cy + 1]);
        for cx in 0..TILE_SIZE {
            let v = [r0[cx], r0[cx + 1], r1[cx + 1], r1[cx]];
            let idx = (v[0] > 127) as u8 | ((v[1] > 127) as u8) << 1 | ((v[2] > 127) as u8) << 2 | ((v[3] > 127) as u8) << 3;
            if idx == 0 || idx == 15 {
                continue;
            }
            let (gx, gy) = ((ox + cx as i32) as f32, (oy + cy as i32) as f32);
            // Edge e: its local id and crossing, interpolated canonically
            // (left to right, top to bottom) so both cells sharing it agree.
            let edge = |e: u8| -> (u16, Pt) {
                let t = |a: u8, b: u8| (ISO - a as f32) / (b as f32 - a as f32);
                match e {
                    0 => (h_edge(cx, cy), [gx + 0.5 + t(v[0], v[1]), gy + 0.5]),
                    1 => (v_edge(cx + 1, cy), [gx + 1.5, gy + 0.5 + t(v[1], v[2])]),
                    2 => (h_edge(cx, cy + 1), [gx + 0.5 + t(v[3], v[2]), gy + 1.5]),
                    _ => (v_edge(cx, cy), [gx + 0.5, gy + 0.5 + t(v[0], v[3])]),
                }
            };
            let hard = v.iter().all(|&x| x == 0 || x == 255);
            let mid = hard.then_some([gx + 1.0, gy + 1.0]);
            let (pairs, n) = cell_pairs(v);
            for &(a, b) in &pairs[..n] {
                let ((from, p0), (to, p1)) = (edge(a), edge(b));
                out.push(Piece { from, to, p0, mid, p1 });
            }
        }
    }
}

/// A block made only of fully selected and unselected tiles: its outline
/// is the tile seams, meeting at the centre of the bottom-right cell. This
/// is the cell rule applied to the 2×2 tiles as one big hard-edged cell.
fn seam_pieces(block: [Class; 4], ox: i32, oy: i32, out: &mut Vec<Piece>) {
    let v = block.map(|c| if c == Class::Full { 255 } else { 0 });
    let last = TILE_SIZE - 1;
    let (cx, cy) = ((ox + T) as f32, (oy + T) as f32);
    let edge = |e: u8| -> (u16, Pt) {
        match e {
            0 => (h_edge(last, 0), [cx, oy as f32 + 0.5]),
            1 => (v_edge(TILE_SIZE, last), [cx + 0.5, cy]),
            2 => (h_edge(last, TILE_SIZE), [cx, cy + 0.5]),
            _ => (v_edge(0, last), [ox as f32 + 0.5, cy]),
        }
    };
    let (pairs, n) = cell_pairs(v);
    for &(a, b) in &pairs[..n] {
        let ((from, p0), (to, p1)) = (edge(a), edge(b));
        out.push(Piece { from, to, p0, mid: Some([cx, cy]), p1 });
    }
}

/// Link the block's pieces: runs from one block edge to another become
/// open chains, the rest close inside the block.
fn link(s: &mut Scratch, ox: i32, oy: i32) -> BlockOut {
    let mut out = BlockOut::default();
    let pieces = &s.pieces;
    for (i, p) in pieces.iter().enumerate() {
        s.start_at[p.from as usize] = i as u32;
    }
    s.used.clear();
    s.used.resize(pieces.len(), false);
    let follow = |first: usize, used: &mut [bool], pts: &mut Vec<Pt>| -> u16 {
        let mut i = first;
        loop {
            used[i] = true;
            let p = &pieces[i];
            pts.extend(p.mid);
            pts.push(p.p1);
            if is_boundary(p.to) {
                return p.to;
            }
            match s.start_at[p.to as usize] {
                NONE => return p.to,
                n if used[n as usize] => return p.to,
                n => i = n as usize,
            }
        }
    };
    for i in 0..pieces.len() {
        if !s.used[i] && is_boundary(pieces[i].from) {
            let mut pts = vec![pieces[i].p0];
            let end = follow(i, &mut s.used, &mut pts);
            out.open.push(Chain { start: global_edge(pieces[i].from, ox, oy), end: global_edge(end, ox, oy), pts });
        }
    }
    for i in 0..pieces.len() {
        if !s.used[i] {
            let mut pts = Vec::new();
            let end = follow(i, &mut s.used, &mut pts);
            // `pts` ends on the crossing it started from.
            let closed = end == pieces[i].from;
            if !closed {
                pts.insert(0, pieces[i].p0);
            }
            out.closed.push(Polyline { pts, closed });
        }
    }
    for p in pieces {
        s.start_at[p.from as usize] = NONE;
    }
    out
}

/// Join open chains end to start across block seams.
fn stitch(chains: Vec<Chain>, out: &mut Vec<Polyline>) {
    let by_start: AHashMap<u64, usize> = chains.iter().enumerate().map(|(i, c)| (c.start, i)).collect();
    let mut used = vec![false; chains.len()];
    for first in 0..chains.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut pts = chains[first].pts.clone();
        let mut cur = first;
        let closed = loop {
            match by_start.get(&chains[cur].end) {
                Some(&n) if n == first => break true,
                Some(&n) if !used[n] => {
                    used[n] = true;
                    // The next chain starts on the crossing this one ended on.
                    pts.extend_from_slice(&chains[n].pts[1..]);
                    cur = n;
                }
                _ => break false,
            }
        };
        if closed {
            pts.pop();
        }
        out.push(Polyline { pts, closed });
    }
}

/// `sel` averaged over `k`×`k` pixel blocks (off-page pixels count as 0),
/// on a `ceil(w/k)`×`ceil(h/k)` page.
fn pooled(sel: &Selection, k: u32, w: u32, h: u32) -> Selection {
    let k = k as i32;
    let (w, h) = (w as i32, h as i32);
    let mut dst: Vec<TileCoord> = sel.tiles().map(|(c, _)| TileCoord::new(c.x.div_euclid(k), c.y.div_euclid(k))).collect();
    dst.sort_unstable_by_key(|c| (c.y, c.x));
    dst.dedup();
    let tiles: Vec<_> = dst
        .par_iter()
        .filter_map(|&d| {
            let src = |i: i32, j: i32| TileCoord::new(d.x * k + i, d.y * k + j);
            if (0..k).all(|j| (0..k).all(|i| class(sel, src(i, j), w, h) == Class::Full)) {
                return Some((d, full_mask().clone()));
            }
            let mut sum = vec![0u32; TILE_SIZE * TILE_SIZE];
            for j in 0..k {
                for i in 0..k {
                    let c = src(i, j);
                    let (ox, oy) = c.origin();
                    let view = match class(sel, c, w, h) {
                        Class::Empty => continue,
                        _ => sel.get(c),
                    };
                    // Pixels of this source tile that lie on the page.
                    let (vw, vh) = ((w - ox).min(T) as usize, (h - oy).min(T) as usize);
                    let (bx, by) = (i as usize * TILE_SIZE, j as usize * TILE_SIZE);
                    let k = k as usize;
                    for y in 0..vh {
                        let row = &mut sum[(by + y) / k * TILE_SIZE..][..TILE_SIZE];
                        for x in 0..vw {
                            row[(bx + x) / k] += match view {
                                MaskView::Empty => 0,
                                MaskView::Full => 255,
                                MaskView::Partial(m) => m[y][x] as u32,
                            };
                        }
                    }
                }
            }
            let area = (k * k) as u32;
            let mut m: Box<MaskPixels> = Box::new([[0; TILE_SIZE]; TILE_SIZE]);
            for (dst, s) in m.as_flattened_mut().iter_mut().zip(&sum) {
                *dst = ((s + area / 2) / area) as u8;
            }
            Some((d, Arc::from(m)))
        })
        .collect();
    let mut out = Selection::new();
    for (c, m) in tiles {
        out.insert_tile(c, m);
    }
    out
}

// ----- simplification ----------------------------------------------------------

fn sub(a: Pt, b: Pt) -> [f64; 2] {
    [a[0] as f64 - b[0] as f64, a[1] as f64 - b[1] as f64]
}

/// `b` adds nothing between `a` and `c`: a repeat, or straight on.
fn collinear(a: Pt, b: Pt, c: Pt) -> bool {
    let (d1, d2) = (sub(b, a), sub(c, b));
    let (l1, l2) = ((d1[0] * d1[0] + d1[1] * d1[1]).sqrt(), (d2[0] * d2[0] + d2[1] * d2[1]).sqrt());
    if l1 == 0.0 || l2 == 0.0 {
        return true;
    }
    let cross = d1[0] * d2[1] - d1[1] * d2[0];
    let dot = d1[0] * d2[0] + d1[1] * d2[1];
    cross.abs() <= 1e-6 * l1 * l2 && dot > 0.0
}

/// Drop repeated points and points on a straight run.
fn merge_collinear(out: &mut Vec<Pt>, closed: bool) {
    // Compact in place: `out[..n]` is the merged prefix.
    let mut n = 0;
    for i in 0..out.len() {
        let p = out[i];
        if n > 0 && out[n - 1] == p {
            continue;
        }
        out[n] = p;
        n += 1;
        while n >= 3 && collinear(out[n - 3], out[n - 2], out[n - 1]) {
            out[n - 2] = out[n - 1];
            n -= 1;
        }
    }
    out.truncate(n);
    if closed {
        let mut start = 0;
        loop {
            let n = out.len() - start;
            if n < 3 {
                break;
            }
            let (first, last) = (out[start], out[out.len() - 1]);
            if collinear(out[out.len() - 2], last, first) {
                out.pop();
            } else if collinear(last, first, out[start + 1]) {
                start += 1;
            } else {
                break;
            }
        }
        out.drain(..start);
    }
}

/// Start a closed outline at its top-most, then left-most point, so the
/// result does not depend on where tracing met it.
fn canonical_start(pts: &mut [Pt]) {
    let key = |p: &Pt| (p[1], p[0]);
    if let Some((i, _)) =
        pts.iter().enumerate().min_by(|a, b| key(a.1).partial_cmp(&key(b.1)).unwrap_or(std::cmp::Ordering::Equal))
    {
        pts.rotate_left(i);
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let (ab, ap) = (sub(b, a), sub(p, a));
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 { ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (ap[0] - t * ab[0], ap[1] - t * ab[1]);
    (dx * dx + dy * dy).sqrt()
}

/// Buffers reused across Douglas–Peucker runs.
#[derive(Default)]
struct DpScratch {
    stack: Vec<(usize, usize)>,
    keep: Vec<bool>,
}

/// Douglas–Peucker over points `a..=b` of `pts` (indices wrap, for closed
/// outlines), marking kept points in `s.keep`.
fn dp(pts: &[Pt], a: usize, b: usize, tol: f64, s: &mut DpScratch) {
    let n = pts.len();
    s.stack.push((a, b));
    while let Some((a, b)) = s.stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (pts[a % n], pts[b % n]);
        let (mut far, mut dmax) = (a, -1.0);
        for i in a + 1..b {
            let d = seg_dist(pts[i % n], pa, pb);
            if d > dmax {
                (far, dmax) = (i, d);
            }
        }
        if dmax > tol {
            s.keep[far % n] = true;
            s.stack.push((a, far));
            s.stack.push((far, b));
        }
    }
}

/// Mark in `s.keep` the points of `p` that Douglas–Peucker keeps at `tol`.
/// False when `p` is degenerate or its extent is under `min_extent`.
fn mark_kept(p: &Polyline, tol: f32, min_extent: f32, s: &mut DpScratch) -> bool {
    let pts = &p.pts;
    let n = pts.len();
    if n < 2 {
        return false;
    }
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for q in pts {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    if (hi[0] - lo[0]).max(hi[1] - lo[1]) < min_extent {
        return false;
    }
    let tol = tol as f64;
    s.keep.clear();
    s.keep.resize(n, false);
    s.keep[0] = true;
    if p.closed {
        // Split at the point farthest from the start, then simplify each
        // half (the second one wraps back to the start).
        let dist2 = |i: usize| {
            let d = sub(pts[i], pts[0]);
            d[0] * d[0] + d[1] * d[1]
        };
        let far = (1..n).max_by(|&i, &j| dist2(i).partial_cmp(&dist2(j)).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0);
        s.keep[far] = true;
        dp(pts, 0, far, tol, s);
        dp(pts, far, n, tol, s);
    } else {
        s.keep[n - 1] = true;
        dp(pts, 0, n - 1, tol, s);
    }
    true
}

/// `p` simplified to `tol` in place, as [`simplify`].
fn simplify_in_place(mut p: Polyline, tol: f32, s: &mut DpScratch) -> Option<Polyline> {
    if !mark_kept(&p, tol, 0.0, s) {
        return None;
    }
    let mut n = 0;
    for i in 0..p.pts.len() {
        if s.keep[i] {
            p.pts[n] = p.pts[i];
            n += 1;
        }
    }
    p.pts.truncate(n);
    (n >= 2).then_some(p)
}

/// `p` simplified to `tol`, or `None` when its extent is under `min_extent`
/// (too small to see at that level) or nothing drawable is left.
fn simplify(p: &Polyline, tol: f32, min_extent: f32, s: &mut DpScratch) -> Option<Polyline> {
    if !mark_kept(p, tol, min_extent, s) {
        return None;
    }
    let n = s.keep.iter().filter(|&&k| k).count();
    if n < 2 {
        return None;
    }
    let mut pts = Vec::with_capacity(n);
    pts.extend(p.pts.iter().zip(&s.keep).filter(|(_, k)| **k).map(|(q, _)| *q));
    Some(Polyline { pts, closed: p.closed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_pairs_follow_the_walk() {
        // Only the bottom-right corner selected: enter on the bottom edge,
        // leave on the right one.
        assert_eq!(cell_pairs([0, 0, 255, 0]), ([(2, 1), (0, 0)], 1));
        assert_eq!(cell_pairs([0; 4]).1, 0);
        assert_eq!(cell_pairs([255; 4]).1, 0);
        // Saddles: a high centre joins the selected corners.
        assert_eq!(cell_pairs([255, 100, 255, 100]).0, [(0, 1), (2, 3)]);
        assert_eq!(cell_pairs([255, 0, 255, 0]).0, [(0, 3), (2, 1)]);
    }

    #[test]
    fn merge_drops_straight_runs_and_repeats() {
        let mut pts = vec![[1.0, 0.0], [2.0, 0.0], [2.0, 0.0], [2.0, 1.0], [2.0, 2.0], [0.0, 2.0], [0.0, 0.0]];
        merge_collinear(&mut pts, true);
        assert_eq!(pts, vec![[2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [0.0, 0.0]]);
    }

    #[test]
    fn edge_ids_are_shared_by_neighbouring_blocks() {
        // The right edge column of one block is the left one of the next.
        assert_eq!(global_edge(v_edge(TILE_SIZE, 5), 0, 0), global_edge(v_edge(0, 5), 64, 0));
        assert_eq!(global_edge(h_edge(7, TILE_SIZE), 0, 0), global_edge(h_edge(7, 0), 0, 64));
        assert_ne!(global_edge(v_edge(3, 3), 0, 0), global_edge(h_edge(3, 3), 0, 0));
    }
}
