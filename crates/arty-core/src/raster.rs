//! Polygon rasterization into selections: one signed-area coverage
//! accumulator for every shape (rect, ellipse, lasso, polygon), in
//! document space.
//!
//! Each edge adds its exact area deltas into an f32 row buffer; a prefix sum
//! along the row, then `min(1, |acc|)`, gives non-zero-winding coverage
//! (font-rs / stb_truetype v2 style). Rows are split into 64-row bands that
//! run in parallel; an edge's contribution to a row depends only on that
//! row, so the band split never changes the output. Within a tile row, only
//! the tile columns an edge reaches are summed pixel by pixel: the winding is
//! constant over a tile no edge enters, so that tile is full or empty as a
//! whole.

use rayon::prelude::*;

use crate::geom::Pt;
use crate::selection::{MaskPixels, MaskRef, Selection, canonical, full_mask};
use crate::tile::{TILE_SIZE, TileCoord};

const T: usize = TILE_SIZE;
const TI: i32 = TILE_SIZE as i32;

/// Rasterizer knobs, for tests and benches.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterOpts {
    /// Exact box-overlap path for axis-aligned rectangles.
    pub rect_fast_path: bool,
    /// Tile rows per band (`0`: the whole shape is one band).
    pub band_tiles: u32,
    /// Run the bands on the rayon pool.
    pub parallel: bool,
}

impl Default for RasterOpts {
    fn default() -> Self {
        Self { rect_fast_path: true, band_tiles: 1, parallel: true }
    }
}

/// Coverage of the closed polygon `pts` (non-zero winding) on a `w`×`h`
/// page, in canonical form. `antialias = false` thresholds at 0.5. Fewer
/// than 3 points, or any non-finite one, give an empty selection.
pub fn rasterize_polygon(pts: &[Pt], w: u32, h: u32, antialias: bool) -> Selection {
    rasterize_with(pts, w, h, antialias, RasterOpts::default())
}

#[doc(hidden)]
pub fn rasterize_with(pts: &[Pt], w: u32, h: u32, antialias: bool, o: RasterOpts) -> Selection {
    let mut sel = Selection::default();
    if o.rect_fast_path
        && let Some(r) = axis_rect(pts)
    {
        for (c, m) in rect_tiles(r, w, h, antialias) {
            sel.put(c, m);
        }
        return sel;
    }
    let Some(p) = Prep::new(pts, w, h) else { return sel };
    let tiles: Vec<(TileCoord, MaskRef)> = if o.parallel {
        // A few chunks of bands per thread, each with its own buffers (the
        // accumulator spans the whole bbox width).
        let bands = p.bands(o.band_tiles);
        let chunk = bands.len().div_ceil(4 * rayon::current_num_threads()).max(1);
        bands
            .par_chunks(chunk)
            .map(|chunk| {
                let mut s = Scratch::default();
                let mut sink = MaskSink::new(antialias);
                for (ty0, ty1, edges) in chunk {
                    p.render(*ty0, *ty1, edges, &mut s, &mut sink);
                }
                sink.out
            })
            .flatten()
            .collect()
    } else {
        let mut s = Scratch::default();
        let mut sink = MaskSink::new(antialias);
        for (ty0, ty1, edges) in p.bands(o.band_tiles) {
            p.render(ty0, ty1, &edges, &mut s, &mut sink);
        }
        sink.out
    };
    for (c, m) in tiles {
        sel.put(c, m);
    }
    sel
}

/// Unquantized coverage of `pts` on a `w`×`h` page, row-major (`w·h`
/// values), for tests.
#[doc(hidden)]
pub fn coverage(pts: &[Pt], w: u32, h: u32, o: RasterOpts) -> Vec<f32> {
    let mut out = vec![0.0f32; w as usize * h as usize];
    if o.rect_fast_path
        && let Some(r) = axis_rect(pts)
    {
        let r = r.clip(w, h);
        for y in 0..h as usize {
            let oy = overlap(y as f64, r[1], r[3]);
            for x in 0..w as usize {
                out[y * w as usize + x] = (overlap(x as f64, r[0], r[2]) * oy) as f32;
            }
        }
        return out;
    }
    let Some(p) = Prep::new(pts, w, h) else { return out };
    let mut s = Scratch::default();
    {
        let mut sink = DenseSink { out: &mut out, w: w as usize, h: h as usize, ty: 0, tx0: p.tx0 };
        for (ty0, ty1, edges) in p.bands(o.band_tiles) {
            p.render(ty0, ty1, &edges, &mut s, &mut sink);
        }
    }
    out
}

// ----- axis-aligned rectangles -----------------------------------------------

/// `[x0, y0, x1, y1]`.
#[derive(Debug, Clone, Copy)]
struct Rect([f64; 4]);

impl Rect {
    fn clip(self, w: u32, h: u32) -> [f64; 4] {
        let [x0, y0, x1, y1] = self.0;
        [x0.clamp(0.0, w as f64), y0.clamp(0.0, h as f64), x1.clamp(0.0, w as f64), y1.clamp(0.0, h as f64)]
    }
}

/// The rectangle `pts` traces, if it is an axis-aligned one with area.
fn axis_rect(pts: &[Pt]) -> Option<Rect> {
    let n = if pts.len() == 5 && pts[0] == pts[4] { 4 } else { pts.len() };
    if n != 4 || pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return None;
    }
    let horizontal = |i: usize| pts[i][1] == pts[(i + 1) % 4][1];
    let vertical = |i: usize| pts[i][0] == pts[(i + 1) % 4][0];
    let alternating = (0..4).all(|i| if i % 2 == 0 { horizontal(i) } else { vertical(i) })
        || (0..4).all(|i| if i % 2 == 0 { vertical(i) } else { horizontal(i) });
    let (xs, ys) = (pts[..4].iter().map(|p| p[0] as f64), pts[..4].iter().map(|p| p[1] as f64));
    let (x0, x1) = (xs.clone().fold(f64::MAX, f64::min), xs.fold(f64::MIN, f64::max));
    let (y0, y1) = (ys.clone().fold(f64::MAX, f64::min), ys.fold(f64::MIN, f64::max));
    (alternating && x1 > x0 && y1 > y0).then_some(Rect([x0, y0, x1, y1]))
}

/// Length of `[p, p + 1] ∩ [a, b]`.
#[inline]
fn overlap(p: f64, a: f64, b: f64) -> f64 {
    ((p + 1.0).min(b) - p.max(a)).max(0.0)
}

fn rect_tiles(r: Rect, w: u32, h: u32, antialias: bool) -> Vec<(TileCoord, MaskRef)> {
    let [x0, y0, x1, y1] = r.clip(w, h);
    let mut out = Vec::new();
    if x1 <= x0 || y1 <= y0 {
        return out;
    }
    let (tx0, tx1) = ((x0 as i32).div_euclid(TI), (x1.ceil() as i32 - 1).div_euclid(TI) + 1);
    let (ty0, ty1) = ((y0 as i32).div_euclid(TI), (y1.ceil() as i32 - 1).div_euclid(TI) + 1);
    for ty in ty0..ty1 {
        let oy0 = (ty * TI) as f64;
        let oy: [f64; T] = std::array::from_fn(|i| overlap(oy0 + i as f64, y0, y1));
        for tx in tx0..tx1 {
            let ox0 = (tx * TI) as f64;
            let c = TileCoord::new(tx, ty);
            if ox0 >= x0 && ox0 + T as f64 <= x1 && oy0 >= y0 && oy0 + T as f64 <= y1 {
                out.push((c, full_mask().clone()));
                continue;
            }
            let ox: [f64; T] = std::array::from_fn(|i| overlap(ox0 + i as f64, x0, x1));
            let mut m: MaskPixels = [[0; T]; T];
            for (row, &fy) in m.iter_mut().zip(&oy) {
                for (v, &fx) in row.iter_mut().zip(&ox) {
                    *v = quantize((fx * fy) as f32, antialias);
                }
            }
            if let Some(m) = canonical(&m) {
                out.push((c, m));
            }
        }
    }
    out
}

#[inline]
fn quantize(cov: f32, antialias: bool) -> u8 {
    if antialias {
        (cov * 255.0 + 0.5) as u8
    } else if cov >= 0.5 {
        255
    } else {
        0
    }
}

// ----- general polygons -------------------------------------------------------

/// One edge, `y0 < y1`, x in buffer coordinates (`0..=bw`), y in document
/// rows.
#[derive(Debug, Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    y1: f32,
    dxdy: f32,
    /// +1 downward, −1 upward.
    dir: f32,
}

impl Edge {
    #[inline]
    fn x_at(&self, y: f32) -> f32 {
        self.x0 + (y - self.y0) * self.dxdy
    }
}

/// The edges of one polygon clipped to the page, and the bbox they live in.
struct Prep {
    edges: Vec<Edge>,
    /// Buffer column 0 is document column `bx0`; `bw` columns are output.
    bx0: i32,
    bx1: i32,
    bw: usize,
    by0: i32,
    by1: i32,
    /// Tile columns `tx0..tx1` and rows `ty0..ty1` hold the bbox.
    tx0: i32,
    tx1: i32,
    ty0: i32,
    ty1: i32,
}

impl Prep {
    fn new(pts: &[Pt], w: u32, h: u32) -> Option<Prep> {
        if pts.len() < 3 || pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return None;
        }
        let fold = |i: usize, f: fn(f64, f64) -> f64, init: f64| pts.iter().map(|p| p[i] as f64).fold(init, f);
        let bx0 = fold(0, f64::min, f64::MAX).floor().clamp(0.0, w as f64) as i32;
        let bx1 = fold(0, f64::max, f64::MIN).ceil().clamp(0.0, w as f64) as i32;
        let by0 = fold(1, f64::min, f64::MAX).floor().clamp(0.0, h as f64) as i32;
        let by1 = fold(1, f64::max, f64::MIN).ceil().clamp(0.0, h as f64) as i32;
        if bx1 <= bx0 || by1 <= by0 {
            return None;
        }
        let bw = (bx1 - bx0) as usize;
        let mut edges = Vec::with_capacity(pts.len());
        for (i, a) in pts.iter().enumerate() {
            let b = pts[(i + 1) % pts.len()];
            let a = [a[0] as f64 - bx0 as f64, a[1] as f64];
            let b = [b[0] as f64 - bx0 as f64, b[1] as f64];
            push_clipped(&mut edges, a, b, bw as f64, by0 as f64, by1 as f64);
        }
        if edges.is_empty() {
            return None;
        }
        edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        Some(Prep {
            edges,
            bx0,
            bx1,
            bw,
            by0,
            by1,
            tx0: bx0.div_euclid(TI),
            tx1: (bx1 - 1).div_euclid(TI) + 1,
            ty0: by0.div_euclid(TI),
            ty1: (by1 - 1).div_euclid(TI) + 1,
        })
    }

    /// Bands of `band_tiles` tile rows with the edges reaching each, in
    /// global (ymin) order.
    fn bands(&self, band_tiles: u32) -> Vec<(i32, i32, Vec<u32>)> {
        let rows = if band_tiles == 0 { self.ty1 - self.ty0 } else { band_tiles as i32 };
        let mut bands: Vec<(i32, i32, Vec<u32>)> = (self.ty0..self.ty1)
            .step_by(rows as usize)
            .map(|t| (t, (t + rows).min(self.ty1), Vec::new()))
            .collect();
        let last = bands.len() as i32 - 1;
        let band_of = |y: f32| ((y as i32).div_euclid(TI) - self.ty0).div_euclid(rows).clamp(0, last);
        for (i, e) in self.edges.iter().enumerate() {
            // The last row the edge reaches is the one holding y1 − ε.
            for b in band_of(e.y0)..=band_of(e.y1.next_down()) {
                bands[b as usize].2.push(i as u32);
            }
        }
        bands
    }

    /// Rasterize tile rows `ty0..ty1` (the edges reaching them: `edges`).
    fn render(&self, ty0: i32, ty1: i32, edges: &[u32], s: &mut Scratch, sink: &mut impl Sink) {
        let stride = self.bw + 2;
        let ncols = (self.tx1 - self.tx0) as usize;
        if s.acc.len() < stride * T {
            s.acc = vec![0.0; stride * T];
        }
        for ty in ty0..ty1 {
            let r0 = (ty * TI).max(self.by0);
            let r1 = (ty * TI + TI).min(self.by1);
            if r1 <= r0 {
                continue;
            }
            s.touched.clear();
            s.touched.resize(ncols, false);
            let (fr0, fr1) = (r0 as f32, r1 as f32);
            for e in edges.iter().map(|&i| &self.edges[i as usize]) {
                if e.y1 <= fr0 || e.y0 >= fr1 {
                    continue;
                }
                let (ya, yb) = (e.y0.max(fr0), e.y1.min(fr1));
                let (xa, xb) = (e.x_at(ya), e.x_at(yb));
                // One cell of slack each way for rounding.
                let c0 = (xa.min(xb).floor() as i32 - 1).max(0);
                let c1 = (xb.max(xa).ceil() as i32 + 1).min(self.bw as i32 - 1);
                for k in self.col_of(c0)..=self.col_of(c1.max(c0)) {
                    s.touched[k] = true;
                }
                let row_a = ya.floor() as i32;
                let row_b = yb.ceil() as i32;
                for r in row_a..row_b {
                    let line = &mut s.acc[(r - r0) as usize * stride..][..stride];
                    draw_row(line, e, r as f32, self.bw as f32);
                }
            }
            sink.begin(ty, &s.touched);
            let mut span = [0.0f32; T];
            for r in r0..r1 {
                let ly = (r - ty * TI) as usize;
                let line = &mut s.acc[(r - r0) as usize * stride..][..stride];
                let mut acc = 0.0f32;
                for k in 0..ncols {
                    let x0 = (self.tx0 + k as i32) * TI;
                    let (c0, c1) = ((x0.max(self.bx0) - self.bx0) as usize, ((x0 + TI).min(self.bx1) - self.bx0) as usize);
                    let lx0 = (c0 as i32 + self.bx0 - x0) as usize;
                    if s.touched[k] {
                        for (cell, out) in line[c0..c1].iter_mut().zip(&mut span) {
                            acc += *cell;
                            *cell = 0.0;
                            *out = acc.abs().min(1.0);
                        }
                        sink.span(k, ly, lx0, &span[..c1 - c0]);
                    } else {
                        sink.fill(k, ly, lx0, c1 - c0, acc.abs().min(1.0));
                    }
                }
                line[self.bw] = 0.0;
                line[self.bw + 1] = 0.0;
            }
            sink.end(ty, self.tx0);
        }
    }

    /// Tile column index (from `tx0`) of buffer cell `cell`.
    #[inline]
    fn col_of(&self, cell: i32) -> usize {
        ((self.bx0 + cell).div_euclid(TI) - self.tx0) as usize
    }
}

/// Clip segment `a`–`b` (buffer x, document y) to rows `y0..y1`, split it
/// at x = 0 and x = `bw`, and push the pieces that can affect the output.
/// Parts left of the buffer collapse onto x = 0 (they still change the
/// winding of every pixel to their right); parts right of it never matter.
fn push_clipped(out: &mut Vec<Edge>, a: [f64; 2], b: [f64; 2], bw: f64, y0: f64, y1: f64) {
    if a[1] == b[1] || a[1].max(b[1]) <= y0 || a[1].min(b[1]) >= y1 {
        return;
    }
    let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let ty = |y: f64| (y - a[1]) / (b[1] - a[1]);
    let (t0, t1) = {
        let (ta, tb) = (ty(y0).clamp(0.0, 1.0), ty(y1).clamp(0.0, 1.0));
        (ta.min(tb), ta.max(tb))
    };
    let mut cuts = [t0, t1, t1, t1];
    let mut n = 1;
    if a[0] != b[0] {
        for x in [0.0, bw] {
            let t = (x - a[0]) / (b[0] - a[0]);
            if t > t0 && t < t1 {
                cuts[n] = t;
                n += 1;
            }
        }
    }
    cuts[n] = t1;
    cuts[..=n].sort_by(f64::total_cmp);
    for w in cuts[..=n].windows(2) {
        let (p, q) = (at(w[0]), at(w[1]));
        if p[0].min(q[0]) >= bw || p[1] == q[1] {
            continue;
        }
        let (p, q) = ([p[0].clamp(0.0, bw), p[1]], [q[0].clamp(0.0, bw), q[1]]);
        let (dir, p, q) = if p[1] < q[1] { (1.0, p, q) } else { (-1.0, q, p) };
        out.push(Edge {
            x0: p[0] as f32,
            y0: p[1] as f32,
            y1: q[1] as f32,
            dxdy: ((q[0] - p[0]) / (q[1] - p[1])) as f32,
            dir,
        });
    }
}

/// Add edge `e`'s area deltas for document row `row` into `line`
/// (`bw + 2` cells).
#[inline]
fn draw_row(line: &mut [f32], e: &Edge, row: f32, bw: f32) {
    let (yt, yb) = (row.max(e.y0), (row + 1.0).min(e.y1));
    if yb <= yt {
        return;
    }
    let d = (yb - yt) * e.dir;
    let x = e.x_at(yt).clamp(0.0, bw);
    let xnext = e.x_at(yb).clamp(0.0, bw);
    let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
    let x0f = x0.floor();
    let x0i = x0f as usize;
    let x1c = x1.ceil();
    let x1i = x1c as usize;
    if x1i <= x0i + 1 {
        let xmf = 0.5 * (x + xnext) - x0f;
        line[x0i] += d - d * xmf;
        line[x0i + 1] += d * xmf;
    } else {
        let s = (x1 - x0).recip();
        let x0r = x0 - x0f;
        let a0 = 0.5 * s * (1.0 - x0r) * (1.0 - x0r);
        let x1r = x1 - x1c + 1.0;
        let am = 0.5 * s * x1r * x1r;
        line[x0i] += d * a0;
        if x1i == x0i + 2 {
            line[x0i + 1] += d * (1.0 - a0 - am);
        } else {
            let a1 = s * (1.5 - x0r);
            line[x0i + 1] += d * (a1 - a0);
            for cell in &mut line[x0i + 2..x1i - 1] {
                *cell += d * s;
            }
            let a2 = a1 + (x1i - x0i - 3) as f32 * s;
            line[x1i - 1] += d * (1.0 - a2 - am);
        }
        line[x1i] += d * am;
    }
}

/// Per-thread buffers: the accumulator (kept all-zero between uses) and the
/// touched-column flags.
#[derive(Default)]
struct Scratch {
    acc: Vec<f32>,
    touched: Vec<bool>,
}

/// Receives one tile row of coverage.
trait Sink {
    fn begin(&mut self, ty: i32, touched: &[bool]);
    /// Coverage of tile column `k`, row `ly`, from pixel `lx0` on.
    fn span(&mut self, k: usize, ly: usize, lx0: usize, cov: &[f32]);
    /// `n` pixels of the same coverage (a tile no edge enters).
    fn fill(&mut self, k: usize, ly: usize, lx0: usize, n: usize, cov: f32);
    fn end(&mut self, ty: i32, tx0: i32);
}

/// Builds canonical mask tiles.
struct MaskSink {
    antialias: bool,
    /// Per tile column: the mask being built (touched columns) or `None`.
    tiles: Vec<Option<Box<MaskPixels>>>,
    /// Per untouched tile column: the value of each row and its span.
    rows: Vec<[(u8, u8, u8); T]>,
    pool: Vec<Box<MaskPixels>>,
    out: Vec<(TileCoord, MaskRef)>,
}

impl MaskSink {
    fn new(antialias: bool) -> Self {
        Self { antialias, tiles: Vec::new(), rows: Vec::new(), pool: Vec::new(), out: Vec::new() }
    }
}

impl Sink for MaskSink {
    fn begin(&mut self, _ty: i32, touched: &[bool]) {
        self.tiles.resize_with(touched.len(), || None);
        self.rows.resize(touched.len(), [(0, 0, 0); T]);
        for (k, &t) in touched.iter().enumerate() {
            self.rows[k] = [(0, 0, 0); T];
            if t {
                let mut m = self.pool.pop().unwrap_or_else(|| Box::new([[0; T]; T]));
                *m = [[0; T]; T];
                self.tiles[k] = Some(m);
            }
        }
    }

    fn span(&mut self, k: usize, ly: usize, lx0: usize, cov: &[f32]) {
        if let Some(m) = &mut self.tiles[k] {
            for (v, &c) in m[ly][lx0..].iter_mut().zip(cov) {
                *v = quantize(c, self.antialias);
            }
        }
    }

    fn fill(&mut self, k: usize, ly: usize, lx0: usize, n: usize, cov: f32) {
        self.rows[k][ly] = (quantize(cov, self.antialias), lx0 as u8, (lx0 + n) as u8);
    }

    fn end(&mut self, ty: i32, tx0: i32) {
        for k in 0..self.tiles.len() {
            let c = TileCoord::new(tx0 + k as i32, ty);
            if let Some(m) = self.tiles[k].take() {
                if let Some(m) = canonical(&m) {
                    self.out.push((c, m));
                }
                self.pool.push(m);
                continue;
            }
            let rows = &self.rows[k];
            if rows.iter().all(|r| r.0 == 0 || r.1 == r.2) {
                continue;
            }
            if rows.iter().all(|&(v, a, b)| v == 255 && a == 0 && b as usize == T) {
                self.out.push((c, full_mask().clone()));
                continue;
            }
            let mut m: MaskPixels = [[0; T]; T];
            for (row, &(v, a, b)) in m.iter_mut().zip(rows) {
                row[a as usize..b as usize].fill(v);
            }
            if let Some(m) = canonical(&m) {
                self.out.push((c, m));
            }
        }
    }
}

/// Writes raw coverage into a page-sized buffer.
struct DenseSink<'a> {
    out: &'a mut [f32],
    w: usize,
    h: usize,
    ty: i32,
    tx0: i32,
}

impl DenseSink<'_> {
    fn at(&mut self, k: usize, ly: usize, lx0: usize) -> Option<&mut [f32]> {
        let y = self.ty as usize * T + ly;
        let x = (self.tx0 as usize + k) * T + lx0;
        (y < self.h && x < self.w).then(|| &mut self.out[y * self.w + x..(y + 1) * self.w])
    }
}

impl Sink for DenseSink<'_> {
    fn begin(&mut self, ty: i32, _touched: &[bool]) {
        self.ty = ty;
    }

    fn span(&mut self, k: usize, ly: usize, lx0: usize, cov: &[f32]) {
        if let Some(row) = self.at(k, ly, lx0) {
            for (o, &c) in row.iter_mut().zip(cov) {
                *o = c;
            }
        }
    }

    fn fill(&mut self, k: usize, ly: usize, lx0: usize, n: usize, cov: f32) {
        if let Some(row) = self.at(k, ly, lx0) {
            let n = n.min(row.len());
            row[..n].fill(cov);
        }
    }

    fn end(&mut self, _ty: i32, _tx0: i32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_area_and_canonical_tiles() {
        let pts = [[10.0, 10.0], [200.0, 30.0], [60.0, 150.0]];
        let cov = coverage(&pts, 256, 256, RasterOpts::default());
        let sum: f64 = cov.iter().map(|&v| v as f64).sum();
        let area = 0.5 * ((200.0 - 10.0) * (150.0 - 10.0) - (60.0 - 10.0) * (30.0 - 10.0f64)).abs();
        assert!((sum - area).abs() < 0.05, "{sum} vs {area}");
        let sel = rasterize_polygon(&pts, 256, 256, true);
        assert!(!sel.is_empty());
        assert_eq!(sel.value(80, 50), 255);
        assert_eq!(sel.value(5, 5), 0);
    }

    #[test]
    fn degenerate_input_is_empty() {
        assert!(rasterize_polygon(&[[0.0, 0.0], [10.0, 10.0]], 64, 64, true).is_empty());
        assert!(rasterize_polygon(&[[0.0, 0.0], [10.0, f32::NAN], [3.0, 9.0]], 64, 64, true).is_empty());
        assert!(rasterize_polygon(&[[-50.0, 0.0], [-10.0, 0.0], [-30.0, 40.0]], 64, 64, true).is_empty());
    }
}
