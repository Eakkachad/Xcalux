//! Selection morphology: grow, shrink and feather.
//!
//! Only the ROI is computed: partial tiles, plus full or empty tiles within
//! `ceil(halo / 64)` tiles of a tile that could change them. Every other
//! tile is copied by pointer. The ROI is cut into runs of tiles along a tile
//! row; each run is one rayon task that reads a window of the mask (the run
//! plus the halo, off-page pixels as 0) and writes the run's tiles.
//!
//! - Circle grow / shrink: threshold at 128, then a Felzenszwalb–Huttenlocher
//!   distance transform capped just past `r`; grow is `clamp(0.5 + r − d_out)`,
//!   shrink `clamp(0.5 + d_in − r)`. Grow never lowers a pixel and shrink
//!   never raises one.
//! - Square grow / shrink: a separable van Herk / Gil-Werman max / min filter
//!   of the 8-bit mask.
//! - Feather σ: three separable box blurs (sliding sums) approximating a
//!   Gaussian.
//!
//! Off the page counts as unselected, so shrink and feather work in from the
//! page border too (as the outline and every other consumer see it).

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::selection::{Class, MaskPixels, MaskRef, MaskView, Selection, canonical, classify, page_tiles};
use crate::tile::{TILE_SIZE, TileCoord};

const T: usize = TILE_SIZE;
const TI: i32 = TILE_SIZE as i32;

/// Widest run of tiles one task computes (bounds its memory and keeps
/// every core busy).
const MAX_RUN_TILES: i32 = 16;
/// Tile rows one task computes, sharing one vertical halo.
const BAND_TILES: i32 = 4;

/// Structuring element of grow and shrink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MorphShape {
    #[default]
    Circle,
    Square,
}

/// Dilate by `r` px on a `w`×`h` page.
pub fn grow(sel: &Selection, r: u16, shape: MorphShape, w: u32, h: u32) -> Selection {
    if r == 0 || sel.is_empty() {
        return sel.clone();
    }
    let r = cap_radius(r, w, h);
    let job = Job { full_changes: false, empty_changes: true, merge: Merge::Max };
    match shape {
        MorphShape::Circle => job.run(sel, w, h, r + 1, |s, out| edt(s, out, r, false)),
        MorphShape::Square => job.run(sel, w, h, r, |s, out| square(s, out, r, u8::max)),
    }
}

/// Erode by `r` px on a `w`×`h` page.
pub fn shrink(sel: &Selection, r: u16, shape: MorphShape, w: u32, h: u32) -> Selection {
    if r == 0 || sel.is_empty() {
        return sel.clone();
    }
    let r = cap_radius(r, w, h);
    let job = Job { full_changes: true, empty_changes: false, merge: Merge::Min };
    match shape {
        MorphShape::Circle => job.run(sel, w, h, r + 1, |s, out| edt(s, out, r, true)),
        MorphShape::Square => job.run(sel, w, h, r, |s, out| square(s, out, r, u8::min)),
    }
}

/// Soften the mask edge (σ = `sigma` px) on a `w`×`h` page.
pub fn feather(sel: &Selection, sigma: u16, w: u32, h: u32) -> Selection {
    if sigma == 0 || sel.is_empty() {
        return sel.clone();
    }
    let radii = box_radii(f64::from(sigma.min(MAX_SIGMA)));
    let halo = radii.iter().sum::<usize>();
    if halo == 0 {
        return sel.clone();
    }
    let job = Job { full_changes: true, empty_changes: true, merge: Merge::Replace };
    job.run(sel, w, h, halo, |s, out| blur(s, out, radii))
}

/// Largest feather σ; a wider blur is clamped to it.
pub const MAX_SIGMA: u16 = 256;
/// Largest grow / shrink radius; a larger one is clamped to it (the work
/// grows with r², and nothing changes past the page diagonal anyway).
pub const MAX_RADIUS: u16 = 512;

fn cap_radius(r: u16, w: u32, h: u32) -> usize {
    let diag = f64::from(w).hypot(f64::from(h)).ceil() as usize + 1;
    usize::from(r.min(MAX_RADIUS)).min(diag)
}

/// Radii of three box blurs whose composition approximates a Gaussian of
/// standard deviation `sigma` (box widths per Kovesi, "Fast almost-Gaussian
/// filtering").
fn box_radii(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let wlf = wl as f64;
    let m = ((12.0 * sigma * sigma - n * wlf * wlf - 4.0 * n * wlf - 3.0 * n) / (-4.0 * wlf - 4.0)).round() as i64;
    std::array::from_fn(|i| (((if (i as i64) < m { wl } else { wu }) - 1) / 2) as usize)
}

// ----- the ROI driver --------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Merge {
    /// `max(original, result)`: grow never unselects.
    Max,
    /// `min(original, result)`: shrink never selects.
    Min,
    Replace,
}

#[derive(Clone, Copy)]
struct Job {
    /// A full tile can change (it may lose pixels).
    full_changes: bool,
    /// An empty tile can change (it may gain pixels).
    empty_changes: bool,
    merge: Merge,
}

/// Per-task output: a tile and its new mask, or `None` to keep the original.
type TileResult = (TileCoord, Option<Option<MaskRef>>);

impl Job {
    /// Recompute every ROI tile with `op`, which fills the central
    /// `ch × cw` pixels of a window (row-major) from its source.
    fn run(self, sel: &Selection, w: u32, h: u32, halo: usize, op: impl Fn(&Src<'_>, &mut [u8]) + Sync) -> Selection {
        let (tw, th) = page_tiles(w, h);
        if tw == 0 || th == 0 {
            return sel.clone();
        }
        let k = halo.div_ceil(T) as i32;
        let roi = self.roi(sel, tw, th, k);
        let is_roi = |tx: i32, ty: i32| roi[(ty * tw + tx) as usize];
        // Bands of tile rows (so the vertical halo is shared), cut into runs
        // of columns holding ROI tiles; gaps cheaper to compute than a
        // second halo are bridged. Wide halos get bigger windows, so the
        // halo never outweighs the part computed.
        let bridge = (2 * halo).div_ceil(T) as i32;
        let (band, max_run) = (BAND_TILES.max(k), MAX_RUN_TILES.max(2 * k));
        let mut runs = Vec::new();
        for ty0 in (0..th).step_by(band as usize) {
            let ty1 = (ty0 + band).min(th);
            let col = |tx: i32| (ty0..ty1).any(|ty| is_roi(tx, ty));
            let mut tx = 0;
            while tx < tw {
                if !col(tx) {
                    tx += 1;
                    continue;
                }
                let start = tx;
                let mut end = tx + 1;
                let mut gap = 0;
                tx += 1;
                while tx < tw && end - start < max_run && gap <= bridge {
                    if col(tx) {
                        end = tx + 1;
                        gap = 0;
                    } else {
                        gap += 1;
                    }
                    tx += 1;
                }
                tx = end;
                runs.push((ty0, ty1, start, end));
            }
        }
        let task = |&(ty0, ty1, tx0, tx1): &(i32, i32, i32, i32)| -> Vec<TileResult> {
            let src = Src::new(sel, w, h, (ty0, ty1), (tx0, tx1), halo);
            let mut out = vec![0u8; src.ch * src.cw];
            op(&src, &mut out);
            (ty0..ty1)
                .flat_map(|ty| (tx0..tx1).map(move |tx| (tx, ty)))
                .filter(|&(tx, ty)| is_roi(tx, ty))
                .map(|(tx, ty)| {
                    let c = TileCoord::new(tx, ty);
                    let rows = &out[(ty - ty0) as usize * T * src.cw..];
                    (c, self.emit(sel.get(c), rows, src.cw, (tx - tx0) as usize * T, c, w, h))
                })
                .collect()
        };
        let results: Vec<Vec<TileResult>> = runs.par_iter().map(task).collect();
        let mut out = sel.clone();
        for (c, m) in results.into_iter().flatten() {
            if let Some(m) = m {
                out.set(c, m);
            }
        }
        out
    }

    /// Which page tiles (row-major) the op may change.
    fn roi(self, sel: &Selection, tw: i32, th: i32, k: i32) -> Vec<bool> {
        let class: Vec<Class> = (0..th)
            .flat_map(|ty| (0..tw).map(move |tx| TileCoord::new(tx, ty)))
            .map(|c| match sel.get(c) {
                MaskView::Empty => Class::Empty,
                MaskView::Full => Class::Full,
                MaskView::Partial(_) => Class::Partial,
            })
            .collect();
        let not_full = Prefix::new(&class, tw, th, |c| c != Class::Full);
        let not_empty = Prefix::new(&class, tw, th, |c| c != Class::Empty);
        let mut roi = vec![false; class.len()];
        for ty in 0..th {
            for tx in 0..tw {
                let (x0, y0, x1, y1) = (tx - k, ty - k, tx + k + 1, ty + k + 1);
                let i = (ty * tw + tx) as usize;
                roi[i] = match class[i] {
                    Class::Partial => true,
                    // Off the page counts as empty, so not full.
                    Class::Full => {
                        self.full_changes
                            && (x0 < 0 || y0 < 0 || x1 > tw || y1 > th || not_full.count(x0, y0, x1, y1) > 0)
                    }
                    Class::Empty => self.empty_changes && not_empty.count(x0, y0, x1, y1) > 0,
                };
            }
        }
        roi
    }

    /// The new mask of tile `c` from the op output (`cw` wide, tile at
    /// column `ox`): merged with the original, off-page pixels cleared, and
    /// `None` when equal to the original.
    #[allow(clippy::too_many_arguments)]
    fn emit(
        self,
        orig: MaskView<'_>,
        out: &[u8],
        cw: usize,
        ox: usize,
        c: TileCoord,
        w: u32,
        h: u32,
    ) -> Option<Option<MaskRef>> {
        let (px, py) = c.origin();
        let nx = (w as i32 - px).clamp(0, TI) as usize;
        let ny = (h as i32 - py).clamp(0, TI) as usize;
        let mut m: MaskPixels = [[0; T]; T];
        for ly in 0..ny {
            let src = &out[ly * cw + ox..][..nx];
            let dst = &mut m[ly][..nx];
            match (self.merge, orig) {
                (Merge::Replace, _) | (Merge::Max, MaskView::Empty) | (Merge::Min, MaskView::Full) => {
                    dst.copy_from_slice(src)
                }
                (Merge::Max, MaskView::Full) => dst.fill(255),
                (Merge::Min, MaskView::Empty) => {}
                (Merge::Max, MaskView::Partial(o)) => {
                    for ((d, &s), &o) in dst.iter_mut().zip(src).zip(&o[ly]) {
                        *d = s.max(o);
                    }
                }
                (Merge::Min, MaskView::Partial(o)) => {
                    for ((d, &s), &o) in dst.iter_mut().zip(src).zip(&o[ly]) {
                        *d = s.min(o);
                    }
                }
            }
        }
        let same = match orig {
            MaskView::Empty => classify(&m) == Class::Empty,
            MaskView::Full => classify(&m) == Class::Full,
            MaskView::Partial(o) => *o == m,
        };
        (!same).then(|| canonical(&m))
    }
}

/// Inclusive 2-D prefix counts of tiles of some classes.
struct Prefix {
    sums: Vec<u32>,
    tw: i32,
    th: i32,
}

impl Prefix {
    fn new(class: &[Class], tw: i32, th: i32, pick: impl Fn(Class) -> bool) -> Prefix {
        let stride = (tw + 1) as usize;
        let mut sums = vec![0u32; stride * (th + 1) as usize];
        for y in 0..th as usize {
            for x in 0..tw as usize {
                let v = u32::from(pick(class[y * tw as usize + x]));
                sums[(y + 1) * stride + x + 1] = v + sums[y * stride + x + 1] + sums[(y + 1) * stride + x] - sums[y * stride + x];
            }
        }
        Prefix { sums, tw, th }
    }

    /// Picked tiles in `[x0, x1) × [y0, y1)`, clipped to the page.
    fn count(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> u32 {
        let (x0, x1) = (x0.clamp(0, self.tw) as usize, x1.clamp(0, self.tw) as usize);
        let (y0, y1) = (y0.clamp(0, self.th) as usize, y1.clamp(0, self.th) as usize);
        if x1 <= x0 || y1 <= y0 {
            return 0;
        }
        let s = (self.tw + 1) as usize;
        self.sums[y1 * s + x1] + self.sums[y0 * s + x0] - self.sums[y0 * s + x1] - self.sums[y1 * s + x0]
    }
}

/// A window of the mask: tiles `tx0..tx1` × `ty0..ty1` (the central part,
/// `cw` × `ch` px) plus `halo` px on every side.
struct Src<'a> {
    views: Vec<MaskView<'a>>,
    vtx0: i32,
    vty0: i32,
    vw: usize,
    /// Document pixel of window pixel (0, 0).
    x0: i32,
    y0: i32,
    ww: usize,
    wh: usize,
    cw: usize,
    ch: usize,
    halo: usize,
    w: i32,
    h: i32,
}

impl<'a> Src<'a> {
    fn new(sel: &'a Selection, w: u32, h: u32, (ty0, ty1): (i32, i32), (tx0, tx1): (i32, i32), halo: usize) -> Src<'a> {
        let (cw, ch) = ((tx1 - tx0) as usize * T, (ty1 - ty0) as usize * T);
        let (ww, wh) = (cw + 2 * halo, ch + 2 * halo);
        let (x0, y0) = (tx0 * TI - halo as i32, ty0 * TI - halo as i32);
        let (vtx0, vty0) = (x0.div_euclid(TI), y0.div_euclid(TI));
        let vtx1 = (x0 + ww as i32 - 1).div_euclid(TI) + 1;
        let vty1 = (y0 + wh as i32 - 1).div_euclid(TI) + 1;
        let vw = (vtx1 - vtx0) as usize;
        let views = (vty0..vty1).flat_map(|y| (vtx0..vtx1).map(move |x| sel.get(TileCoord::new(x, y)))).collect();
        Src { views, vtx0, vty0, vw, x0, y0, ww, wh, cw, ch, halo, w: w as i32, h: h as i32 }
    }

    /// Window row `y` into `out` (`ww` values; off the page is 0).
    fn row(&self, y: usize, out: &mut [u8]) {
        out.fill(0);
        let yd = self.y0 + y as i32;
        if yd < 0 || yd >= self.h {
            return;
        }
        let vrow = (yd.div_euclid(TI) - self.vty0) as usize * self.vw;
        let ly = yd.rem_euclid(TI) as usize;
        let (xa, xb) = (self.x0.max(0), (self.x0 + self.ww as i32).min(self.w));
        let mut x = xa;
        while x < xb {
            let tx = x.div_euclid(TI);
            let end = ((tx + 1) * TI).min(xb);
            let dst = &mut out[(x - self.x0) as usize..(end - self.x0) as usize];
            match self.views[vrow + (tx - self.vtx0) as usize] {
                MaskView::Empty => {}
                MaskView::Full => dst.fill(255),
                MaskView::Partial(m) => dst.copy_from_slice(&m[ly][(x - tx * TI) as usize..(end - tx * TI) as usize]),
            }
            x = end;
        }
    }
}

// ----- circle: distance transform --------------------------------------------

/// Distance-based grow (`inside = false`: distance to the nearest selected
/// pixel) or shrink (`inside = true`: to the nearest unselected one).
fn edt(s: &Src<'_>, out: &mut [u8], r: usize, inside: bool) {
    let (ww, wh, hl) = (s.ww, s.wh, s.halo);
    // Any distance at or past the cap gives a saturated result.
    let cap = (hl + 1) as u32;
    let feature = |v: u8| if inside { v < 128 } else { v >= 128 };
    let mut row = vec![0u8; ww];
    let mut cnt = vec![cap; ww];
    // Vertical distance to the nearest feature, central rows only.
    let mut g = vec![cap; s.ch * ww];
    for y in 0..wh {
        s.row(y, &mut row);
        for (c, &v) in cnt.iter_mut().zip(&row) {
            *c = if feature(v) { 0 } else { (*c + 1).min(cap) };
        }
        if (hl..hl + s.ch).contains(&y) {
            g[(y - hl) * ww..][..ww].copy_from_slice(&cnt);
        }
    }
    cnt.fill(cap);
    for y in (hl..wh).rev() {
        s.row(y, &mut row);
        for (c, &v) in cnt.iter_mut().zip(&row) {
            *c = if feature(v) { 0 } else { (*c + 1).min(cap) };
        }
        if y < hl + s.ch {
            for (gv, &c) in g[(y - hl) * ww..][..ww].iter_mut().zip(&cnt) {
                *gv = (*gv).min(c);
            }
        }
    }
    // Horizontal pass: lower envelope of the parabolas g(i)² + (x − i)².
    let mut env = Envelope::default();
    let rf = r as f64;
    // Squared distances at or below `near` / at or above `far` saturate,
    // so only the pixels in between need a square root.
    let (near, far) = ((rf - 0.5).powi(2), (rf + 0.5).powi(2));
    let (close, distant) = if inside { (0u8, 255u8) } else { (255, 0) };
    for ly in 0..s.ch {
        let gr = &g[ly * ww..][..ww];
        let dst = &mut out[ly * s.cw..][..s.cw];
        if gr[hl..hl + s.cw].iter().all(|&v| v == 0) {
            // Every central pixel is a feature.
            dst.fill(close);
            continue;
        }
        env.build(gr.iter().enumerate().filter(|&(_, &v)| v < cap).map(|(i, &v)| (i as f64, f64::from(v * v))));
        let mut it = env.eval(hl);
        for d in dst.iter_mut() {
            let d2 = it.next_d2();
            *d = if d2 <= near {
                close
            } else if d2 >= far {
                distant
            } else {
                let dist = d2.sqrt();
                let v = if inside { 0.5 + dist - rf } else { 0.5 + rf - dist };
                (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            };
        }
    }
}

/// Felzenszwalb–Huttenlocher lower envelope of parabolas over sparse sites.
#[derive(Default)]
struct Envelope {
    v: Vec<f64>,
    f: Vec<f64>,
    z: Vec<f64>,
}

impl Envelope {
    fn build(&mut self, sites: impl Iterator<Item = (f64, f64)>) {
        self.v.clear();
        self.f.clear();
        self.z.clear();
        for (q, fq) in sites {
            // Drop parabolas the new one hides entirely, then record where
            // the new one starts winning (−∞ for the first).
            let mut start = f64::NEG_INFINITY;
            while let Some(k) = self.v.len().checked_sub(1) {
                let (vk, fk) = (self.v[k], self.f[k]);
                let s = ((fq + q * q) - (fk + vk * vk)) / (2.0 * (q - vk));
                if k > 0 && s <= self.z[k] {
                    self.v.pop();
                    self.f.pop();
                    self.z.pop();
                    continue;
                }
                start = s;
                break;
            }
            self.z.push(start);
            self.v.push(q);
            self.f.push(fq);
        }
        // z[k] is where parabola k starts winning; one past the last is +∞.
        self.z.push(f64::INFINITY);
    }

    /// Squared distances at x = start, start + 1, …
    fn eval(&self, start: usize) -> EnvelopeIter<'_> {
        EnvelopeIter { env: self, k: 0, x: start as f64 }
    }
}

struct EnvelopeIter<'a> {
    env: &'a Envelope,
    k: usize,
    x: f64,
}

impl EnvelopeIter<'_> {
    fn next_d2(&mut self) -> f64 {
        let e = self.env;
        let x = self.x;
        self.x += 1.0;
        if e.v.is_empty() {
            return f64::INFINITY;
        }
        while e.z[self.k + 1] < x {
            self.k += 1;
        }
        let dx = x - e.v[self.k];
        dx * dx + e.f[self.k]
    }
}

// ----- square: van Herk / Gil-Werman -----------------------------------------

/// Max (`op = max`) or min filter over a (2r+1)² square.
fn square(s: &Src<'_>, out: &mut [u8], r: usize, op: impl Fn(u8, u8) -> u8 + Copy) {
    let (ww, wh) = (s.ww, s.wh);
    let l = 2 * r + 1;
    let mut row = vec![0u8; ww];
    let mut run = vec![0u8; ww];
    // Vertically, central row c spans window rows c..=c+2r: the suffix of
    // its block from row c (`hh`, rows 0..ch) and the prefix of the next
    // block up to row c+2r (`g`, rows 2r..2r+ch).
    let mut g = vec![0u8; s.ch * ww];
    let mut hh = vec![0u8; s.ch * ww];
    let combine = |run: &mut [u8], row: &[u8]| {
        for (a, &b) in run.iter_mut().zip(row) {
            *a = op(*a, b);
        }
    };
    for y in 0..wh {
        s.row(y, &mut row);
        if y % l == 0 {
            run.copy_from_slice(&row);
        } else {
            combine(&mut run, &row);
        }
        if y >= 2 * r {
            g[(y - 2 * r) * ww..][..ww].copy_from_slice(&run);
        }
    }
    let last = ((s.ch - 1) / l + 1) * l - 1;
    let last = last.min(wh - 1);
    for y in (0..=last).rev() {
        s.row(y, &mut row);
        if y == last || y % l == l - 1 {
            run.copy_from_slice(&row);
        } else {
            combine(&mut run, &row);
        }
        if y < s.ch {
            hh[y * ww..][..ww].copy_from_slice(&run);
        }
    }
    let (mut gx, mut hx) = (vec![0u8; ww], vec![0u8; ww]);
    for c in 0..s.ch {
        let vr = &mut row;
        for ((v, &a), &b) in vr.iter_mut().zip(&hh[c * ww..][..ww]).zip(&g[c * ww..][..ww]) {
            *v = op(a, b);
        }
        for b in (0..ww).step_by(l) {
            let end = (b + l).min(ww);
            gx[b] = vr[b];
            for x in b + 1..end {
                gx[x] = op(gx[x - 1], vr[x]);
            }
            hx[end - 1] = vr[end - 1];
            for x in (b..end - 1).rev() {
                hx[x] = op(hx[x + 1], vr[x]);
            }
        }
        for (cx, d) in out[c * s.cw..][..s.cw].iter_mut().enumerate() {
            // Central column cx is window column r + cx.
            *d = op(hx[cx], gx[cx + 2 * r]);
        }
    }
}

// ----- feather: three box blurs ----------------------------------------------

/// Fixed-point scale of the blur buffers.
const BLUR_ONE: u32 = 128;

fn blur(s: &Src<'_>, out: &mut [u8], radii: [usize; 3]) {
    let (ww, wh, hl) = (s.ww, s.wh, s.halo);
    let mut a = vec![0u16; ww * wh];
    let mut b = vec![0u16; ww * wh];
    let mut row = vec![0u8; ww];
    for y in 0..wh {
        s.row(y, &mut row);
        for (d, &v) in a[y * ww..][..ww].iter_mut().zip(&row) {
            *d = (u32::from(v) * BLUR_ONE) as u16;
        }
    }
    // Horizontal passes on every row: a → b → a → b.
    for (i, &r) in radii.iter().enumerate() {
        let (src, dst) = if i % 2 == 0 { (&a, &mut b) } else { (&b, &mut a) };
        for y in 0..wh {
            box_row(&src[y * ww..][..ww], &mut dst[y * ww..][..ww], r);
        }
    }
    // Vertical passes on the central columns only: b → a → b → a.
    let cols = hl..hl + s.cw;
    let mut sums = vec![0u32; s.cw];
    for (i, &r) in radii.iter().enumerate() {
        let (src, dst) = if i % 2 == 0 { (&b, &mut a) } else { (&a, &mut b) };
        box_cols(src, dst, ww, wh, cols.clone(), r, &mut sums);
    }
    for ly in 0..s.ch {
        let src = &a[(hl + ly) * ww + hl..][..s.cw];
        for (d, &v) in out[ly * s.cw..][..s.cw].iter_mut().zip(src) {
            *d = ((u32::from(v) + BLUR_ONE / 2) / BLUR_ONE).min(255) as u8;
        }
    }
}

/// `sum / (2r + 1)`, rounded, by multiplication.
#[derive(Clone, Copy)]
struct Div {
    mul: u64,
}

impl Div {
    fn new(r: usize) -> Div {
        Div { mul: (1u64 << 32) / (2 * r as u64 + 1) }
    }

    #[inline]
    fn apply(self, sum: u32) -> u16 {
        ((u64::from(sum) * self.mul + (1 << 31)) >> 32) as u16
    }
}

/// Box blur of radius `r` along one row (zero past the ends).
fn box_row(src: &[u16], dst: &mut [u16], r: usize) {
    let n = src.len();
    let div = Div::new(r);
    let mut sum: u32 = src[..(r + 1).min(n)].iter().map(|&v| u32::from(v)).sum();
    if n <= 2 * r + 1 {
        for x in 0..n {
            dst[x] = div.apply(sum);
            if x + r + 1 < n {
                sum += u32::from(src[x + r + 1]);
            }
            if x >= r {
                sum -= u32::from(src[x - r]);
            }
        }
        return;
    }
    // Head (nothing leaves yet), body, tail (nothing enters).
    for x in 0..r {
        dst[x] = div.apply(sum);
        sum += u32::from(src[x + r + 1]);
    }
    for x in r..n - r - 1 {
        dst[x] = div.apply(sum);
        sum = sum + u32::from(src[x + r + 1]) - u32::from(src[x - r]);
    }
    for x in n - r - 1..n {
        dst[x] = div.apply(sum);
        sum -= u32::from(src[x - r]);
    }
}

/// Box blur of radius `r` down columns `cols` of a `ww`-wide buffer.
fn box_cols(src: &[u16], dst: &mut [u16], ww: usize, wh: usize, cols: std::ops::Range<usize>, r: usize, sums: &mut [u32]) {
    let div = Div::new(r);
    let at = |y: usize| &src[y * ww + cols.start..y * ww + cols.end];
    sums.fill(0);
    for y in 0..(r + 1).min(wh) {
        for (s, &v) in sums.iter_mut().zip(at(y)) {
            *s += u32::from(v);
        }
    }
    for y in 0..wh {
        for (d, &s) in dst[y * ww + cols.start..y * ww + cols.end].iter_mut().zip(sums.iter()) {
            *d = div.apply(s);
        }
        if y + r + 1 < wh {
            for (s, &v) in sums.iter_mut().zip(at(y + r + 1)) {
                *s += u32::from(v);
            }
        }
        if y >= r {
            for (s, &v) in sums.iter_mut().zip(at(y - r)) {
                *s -= u32::from(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_radii_approximate_sigma() {
        for sigma in [1.0, 4.0, 16.0, 40.0] {
            let r = box_radii(sigma);
            // Variance of a box of width 2r+1 is ((2r+1)² − 1) / 12.
            let var: f64 = r.iter().map(|&r| (((2 * r + 1) * (2 * r + 1)) as f64 - 1.0) / 12.0).sum();
            assert!((var.sqrt() - sigma).abs() < 0.6, "σ {sigma}: {r:?} gives {}", var.sqrt());
        }
    }

    #[test]
    fn envelope_matches_brute_force() {
        let sites = [(2.0, 4.0), (5.0, 0.0), (9.0, 9.0), (10.0, 1.0)];
        let mut env = Envelope::default();
        env.build(sites.iter().copied());
        let mut it = env.eval(0);
        for x in 0..16 {
            let want = sites.iter().map(|&(q, f)| (x as f64 - q).powi(2) + f).fold(f64::MAX, f64::min);
            assert!((it.next_d2() - want).abs() < 1e-9, "x {x}");
        }
    }
}
