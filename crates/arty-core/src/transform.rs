//! Free transform (move, scale, rotate, flip) of a layer or of the selected
//! pixels: a floating session previewed into the layer itself, committed
//! as one history step.
//!
//! - **Begin** keeps the layer's grid (`orig`, an O(1) clone). A layer
//!   target transforms all of it; a selection target lifts `px·m/255` into
//!   `src` and leaves `px − lift` (`px·(1 − m/255)` up to rounding) in
//!   `base`.
//! - **Preview** writes `resample(src) over base` into every destination
//!   tile the transformed source can reach and puts `base` back on tiles it
//!   left, so the layer stacks with its neighbours exactly (no proxy).
//! - **Commit** resamples once more with the final filter and records every
//!   tile whose `Arc` differs from `orig`. **Cancel** puts `orig` back.
//!
//! Sampling maps each destination pixel centre `(x + .5, y + .5)` through the
//! inverse transform and subtracts `0.5`, so source pixel `i` sits at `i`
//! (the legacy code was half a pixel off). Downscales sample a 2× box mip
//! pyramid at level ⌊log2 of the inverse Jacobian's largest column norm⌋.
//! Whole-pixel translations are shifted copies, and translations by
//! multiples of 64 reuse the source tiles' `Arc`s.

mod pyramid;
mod sample;

use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::blend::{BlendMode, blend_tile};
use crate::document::Document;
use crate::geom::{Affine64, RectF};
use crate::grid::TileGrid;
use crate::history::Edit;
use crate::layer::LayerId;
use crate::selection::{MaskPixels, MaskRef, MaskView, Selection, full_mask};
use crate::tile::{TILE_SIZE, TILE_SIZE_I32, TileCoord, TilePixels, TileRef, new_tile};

use pyramid::{MAX_LEVEL, SrcPyramid};

/// Resampling filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Filter {
    Nearest,
    Bilinear,
    #[default]
    Bicubic,
}

/// What a session moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfTarget {
    /// The whole layer.
    Layer,
    /// The selected pixels, lifted off the layer, and the selection itself.
    Selection,
}

/// Why [`FloatSession::begin`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfRefused {
    Folder,
    Locked,
    Empty,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XfParams {
    /// Translation, doc px.
    pub t: [f64; 2],
    /// Scale; negative flips.
    pub s: [f64; 2],
    /// Rotation, radians.
    pub theta: f64,
    /// Centre of scale and rotation, doc px.
    pub pivot: [f64; 2],
}

impl XfParams {
    pub fn identity(pivot: [f64; 2]) -> Self {
        Self { t: [0.0; 2], s: [1.0; 2], theta: 0.0, pivot }
    }

    /// `T(t)·T(p)·R(θ)·S(s)·T(−p)`: scale, then rotate, about the pivot,
    /// then translate.
    pub fn affine(&self) -> Affine64 {
        let [px, py] = self.pivot;
        Affine64::translate(-px, -py)
            .then(Affine64::scale(self.s[0], self.s[1]))
            .then(Affine64::rotate(self.theta))
            .then(Affine64::translate(px + self.t[0], py + self.t[1]))
    }
}

/// Maps a destination pixel centre to a source position (affine now,
/// projective later).
pub trait DestMap {
    fn src(&self, x: f32, y: f32) -> [f32; 2];
}

/// An affine map used as a [`DestMap`] (pass the inverse transform). The
/// tile sampler specializes on affine maps to step along rows.
impl DestMap for Affine64 {
    fn src(&self, x: f32, y: f32) -> [f32; 2] {
        let p = self.apply([x as f64, y as f64]);
        [p[0] as f32, p[1] as f32]
    }
}

/// True when `a` moves no point by more than ~1e-9 px per px.
pub fn is_identity(a: &Affine64) -> bool {
    a.m.iter().zip(Affine64::IDENTITY.m).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// A filter reaches this many source px around the sample point.
const REACH: f64 = 2.0;
/// Past this many source tiles under one destination tile, assume it hits one.
const PROBE_CAP: i64 = 4096;

/// How one render maps the source.
#[derive(Clone, Copy)]
enum Plan {
    /// Translation by whole tiles: tile `c` comes from `c − (dx, dy)`.
    ShiftTiles { dx: i32, dy: i32 },
    /// Translation by whole pixels: a shifted copy.
    ShiftPx { dx: i32, dy: i32 },
    /// Everything else, at pyramid `level`; `a` maps destination pixel
    /// centres to that level's index space.
    Sample { level: u32, a: Affine64, filter: Filter },
}

impl Plan {
    /// `None` when `xf` is singular (nothing is drawn).
    fn new(xf: &Affine64, filter: Filter) -> Option<Plan> {
        let [m0, m1, m2, m3, m4, m5] = xf.m;
        let linear_id = (m0 - 1.0).abs() < 1e-9 && m1.abs() < 1e-9 && m3.abs() < 1e-9 && (m4 - 1.0).abs() < 1e-9;
        let whole = |v: f64| (v - v.round()).abs() < 1e-6 && v.abs() < 1.0e8;
        if linear_id && whole(m2) && whole(m5) {
            let (dx, dy) = (m2.round() as i32, m5.round() as i32);
            if dx % TILE_SIZE_I32 == 0 && dy % TILE_SIZE_I32 == 0 {
                return Some(Plan::ShiftTiles { dx: dx / TILE_SIZE_I32, dy: dy / TILE_SIZE_I32 });
            }
            return Some(Plan::ShiftPx { dx, dy });
        }
        let inv = xf.inverse()?;
        let [i0, i1, _, i3, i4, _] = inv.m;
        let norm = (i0 * i0 + i3 * i3).sqrt().max((i1 * i1 + i4 * i4).sqrt());
        let level = if norm > 1.0 { (norm.log2() + 1e-6).floor().min(MAX_LEVEL as f64) as u32 } else { 0 };
        let k = 1.0 / f64::from(1u32 << level);
        let a = inv.then(Affine64::scale(k, k)).then(Affine64::translate(-0.5, -0.5));
        Some(Plan::Sample { level, a, filter })
    }

    /// Reach of the filter in level-0 source px.
    fn margin(&self) -> f64 {
        match *self {
            Plan::ShiftTiles { .. } | Plan::ShiftPx { .. } => 0.0,
            Plan::Sample { level, .. } => REACH * f64::from(1u32 << level) + 1.0,
        }
    }
}

/// The tiles of the destination that `rect` (source px, grown by `margin`)
/// reaches under `xf`, clamped to the page ± one page per side.
fn dest_tiles(rect: RectF, margin: f64, xf: &Affine64, tw: i32, th: i32) -> Vec<TileCoord> {
    let (x0, y0) = (rect.x as f64 - margin, rect.y as f64 - margin);
    let (x1, y1) = ((rect.x + rect.w) as f64 + margin, (rect.y + rect.h) as f64 + margin);
    let quad = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| xf.apply(p));
    if quad.iter().flatten().any(|v| !v.is_finite()) {
        return Vec::new();
    }
    let t = TILE_SIZE as f64;
    let lo = |i: usize| quad.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min);
    let hi = |i: usize| quad.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max);
    let clamp_x = |v: f64| (v / t).floor().clamp(-tw as f64, (2 * tw) as f64) as i32;
    let clamp_y = |v: f64| (v / t).floor().clamp(-th as f64, (2 * th) as f64) as i32;
    let (tx0, tx1) = (clamp_x(lo(0)), clamp_x(hi(0)).min(2 * tw - 1));
    let (ty0, ty1) = (clamp_y(lo(1)), clamp_y(hi(1)).min(2 * th - 1));
    // Separating axes: the quad's edge normals (the tile axes are the bbox).
    let axes: Vec<[f64; 2]> = (0..2)
        .map(|i| {
            let (a, b) = (quad[i], quad[i + 1]);
            [b[1] - a[1], a[0] - b[0]]
        })
        .collect();
    let spans: Vec<(f64, f64)> = axes
        .iter()
        .map(|n| {
            let d = quad.map(|p| p[0] * n[0] + p[1] * n[1]);
            (d.iter().copied().fold(f64::INFINITY, f64::min), d.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        })
        .collect();
    let mut out = Vec::new();
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let (ox, oy) = (tx as f64 * t, ty as f64 * t);
            let corners = [[ox, oy], [ox + t, oy], [ox + t, oy + t], [ox, oy + t]];
            let hit = axes.iter().zip(&spans).all(|(n, &(lo, hi))| {
                let d = corners.map(|p| p[0] * n[0] + p[1] * n[1]);
                let (clo, chi) =
                    (d.iter().copied().fold(f64::INFINITY, f64::min), d.iter().copied().fold(f64::NEG_INFINITY, f64::max));
                chi >= lo && clo <= hi
            });
            if hit {
                out.push(TileCoord::new(tx, ty));
            }
        }
    }
    out
}

/// A transform in progress. The layer shows the preview while it runs.
pub struct FloatSession {
    layer: LayerId,
    target: XfTarget,
    /// The layer's grid when the session began (O(1) clone).
    orig: TileGrid,
    /// The pixels being transformed.
    src: TileGrid,
    /// What stays under them: `orig` with the lifted pixels taken out
    /// (empty for a layer target).
    base: TileGrid,
    /// Tight bbox of alpha > 0 in `src`.
    src_bounds: RectF,
    sel_before: Option<Selection>,
    params: XfParams,
    /// Tiles where the layer differs from `base` (before the first
    /// render: every `src` tile, since the layer still holds them).
    touched: Vec<TileCoord>,
    pyramid: SrcPyramid,
    pool: TilePool,
    /// A render has written the layer.
    rendered: bool,
    /// Page size in pixels and tiles.
    page: (u32, u32),
    tiles: (i32, i32),
}

impl FloatSession {
    /// Start a session on `layer`; the target is the selection when the
    /// document has one. Folders, locked layers and empty sources are
    /// refused.
    pub fn begin(doc: &mut Document, layer: LayerId) -> Result<FloatSession, XfRefused> {
        let l = doc.layer(layer).ok_or(XfRefused::Unsupported)?;
        let grid = l.raster().ok_or(XfRefused::Folder)?;
        if l.props.locked {
            return Err(XfRefused::Locked);
        }
        let orig = grid.clone();
        let (w, h) = (doc.width(), doc.height());
        let sel = doc.selection();
        let (target, src, base, sel_before) = if sel.is_empty() {
            (XfTarget::Layer, orig.clone(), TileGrid::new(), None)
        } else {
            let (src, base) = lift(&orig, sel, w, h);
            (XfTarget::Selection, src, base, Some(sel.clone()))
        };
        let src_bounds = alpha_bounds(&src).ok_or(XfRefused::Empty)?;
        let pivot = [(src_bounds.x + src_bounds.w * 0.5) as f64, (src_bounds.y + src_bounds.h * 0.5) as f64];
        let mut touched: Vec<TileCoord> = src.coords().collect();
        touched.sort_unstable();
        Ok(FloatSession {
            layer,
            target,
            orig,
            src,
            base,
            src_bounds,
            sel_before,
            params: XfParams::identity(pivot),
            touched,
            pyramid: SrcPyramid::default(),
            pool: TilePool::default(),
            rendered: false,
            page: (w, h),
            tiles: (doc.tiles_wide() as i32, doc.tiles_high() as i32),
        })
    }

    pub fn layer(&self) -> LayerId {
        self.layer
    }

    pub fn target(&self) -> XfTarget {
        self.target
    }

    pub fn params(&self) -> XfParams {
        self.params
    }

    pub fn src_bounds(&self) -> RectF {
        self.src_bounds
    }

    pub fn affine(&self) -> Affine64 {
        self.params.affine()
    }

    /// Change the params without drawing them (the next [`Self::preview`]
    /// or [`Self::commit`] does).
    pub fn set_params(&mut self, p: XfParams) {
        self.params = p;
    }

    /// Show the layer transformed by `p`.
    pub fn preview(&mut self, doc: &mut Document, p: XfParams, f: Filter) {
        self.params = p;
        self.render(doc, f);
    }

    /// Final resample with `f`. `Edit::Pixels`, or `Edit::Batch[Pixels,
    /// Selection]` for a selection target; `None` when nothing changed.
    pub fn commit(mut self, doc: &mut Document, f: Filter) -> Option<Edit> {
        let xf = self.params.affine();
        if is_identity(&xf) {
            // Not even a re-lift: `over(lift, base)` is not exactly `px` at
            // antialiased edges.
            self.restore(doc);
            return None;
        }
        self.render(doc, f);
        let grid = doc.layer(self.layer)?.raster()?;
        let mut tiles: Vec<(TileCoord, Option<TileRef>)> = grid
            .iter()
            .filter(|(c, t)| self.orig.get_ref(*c).is_none_or(|o| !Arc::ptr_eq(o, t)))
            .map(|(c, _)| (c, self.orig.get_ref(c).cloned()))
            .collect();
        tiles.extend(self.orig.iter().filter(|(c, _)| grid.get_ref(*c).is_none()).map(|(c, o)| (c, Some(o.clone()))));
        tiles.sort_unstable_by_key(|(c, _)| *c);
        let pixels = (!tiles.is_empty()).then_some(Edit::Pixels { layer: self.layer, tiles });
        match self.target {
            XfTarget::Layer => pixels,
            XfTarget::Selection => {
                let sel = self.sel_before.take().unwrap_or_default();
                let moved = transform_mask(&sel, &xf, self.page.0, self.page.1);
                let old = doc.swap_selection(moved);
                let mut edits: Vec<Edit> = pixels.into_iter().collect();
                edits.push(Edit::Selection(Box::new(old)));
                Some(Edit::Batch(edits))
            }
        }
    }

    /// Put the layer back as it was. No history entry.
    pub fn cancel(self, doc: &mut Document) {
        self.restore(doc);
    }

    /// `*grid = orig`, marking every tile a render may have changed.
    fn restore(&self, doc: &mut Document) {
        if !self.rendered {
            return;
        }
        let Some((grid, dirty)) = doc.paint_target(self.layer) else { return };
        *grid = self.orig.clone();
        for c in self.touched.iter().copied().chain(self.src.coords()) {
            dirty.mark(c);
        }
    }

    /// Write `resample(src) over base` for the current params into the
    /// layer, and `base` back on every tile the source left.
    fn render(&mut self, doc: &mut Document, f: Filter) {
        let xf = self.params.affine();
        let plan = Plan::new(&xf, f);
        let dest = match &plan {
            Some(p) => dest_tiles(self.src_bounds, p.margin(), &xf, self.tiles.0, self.tiles.1),
            None => Vec::new(),
        };
        if let Some(Plan::Sample { level, .. }) = plan {
            self.pyramid.ensure(&self.src, level);
        }
        let Some((grid, dirty)) = doc.paint_target(self.layer) else { return };
        self.rendered = true;
        // Put `base` back on every tile the last render changed. Its tiles
        // (held by nobody else) go to the pool, so this render draws into
        // them instead of allocating.
        for &c in &self.touched {
            self.pool.give(grid.replace(c, self.base.get_ref(c).cloned()));
            dirty.mark(c);
        }
        let results: Vec<(TileCoord, Option<TileRef>)> = match plan {
            Some(plan) => {
                let job = Job { src: &self.src, base: &self.base, pyramid: &self.pyramid, pool: &self.pool, plan };
                dest.par_iter().map(|&c| (c, job.tile(c))).collect()
            }
            None => Vec::new(),
        };
        let mut touched = Vec::with_capacity(results.len());
        for (c, t) in results {
            if !same(self.base.get_ref(c), t.as_ref()) {
                touched.push(c);
            }
            if !same(grid.get_ref(c), t.as_ref()) {
                self.pool.give(grid.replace(c, t));
                dirty.mark(c);
            }
        }
        self.touched = touched;
        self.pool.clear();
    }
}

/// Preview tiles nobody else holds any more, reused by the next render: a
/// fresh 32 KiB tile costs more than resampling into it.
#[derive(Default)]
struct TilePool(Mutex<Vec<TileRef>>);

impl TilePool {
    /// A tile with arbitrary pixels, for a caller that overwrites them all.
    fn take(&self) -> TileRef {
        self.0.lock().ok().and_then(|mut v| v.pop()).unwrap_or_else(new_tile)
    }

    /// Keep `t` when this is its last reference.
    fn give(&self, t: Option<TileRef>) {
        if let Some(mut t) = t
            && Arc::get_mut(&mut t).is_some()
            && let Ok(mut v) = self.0.lock()
        {
            v.push(t);
        }
    }

    /// Free what the render did not reuse (the layer keeps its tiles).
    fn clear(&mut self) {
        if let Ok(v) = self.0.get_mut() {
            v.clear();
        }
    }
}

fn same(a: Option<&TileRef>, b: Option<&TileRef>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// What every destination tile of one render reads.
struct Job<'a> {
    src: &'a TileGrid,
    base: &'a TileGrid,
    pyramid: &'a SrcPyramid,
    pool: &'a TilePool,
    plan: Plan,
}

impl Job<'_> {
    /// Destination tile `c`: the transformed source over `base`.
    fn tile(&self, c: TileCoord) -> Option<TileRef> {
        let base = self.base.get_ref(c);
        match self.plan {
            Plan::ShiftTiles { dx, dy } => match (self.src.get_ref(TileCoord::new(c.x - dx, c.y - dy)), base) {
                (None, b) => b.cloned(),
                (Some(s), None) => Some(s.clone()),
                (Some(s), Some(b)) => Some(self.over(s, b)),
            },
            Plan::ShiftPx { dx, dy } => match sample::shift_window(self.src, dx, dy, c) {
                Some(mut win) => self.write_over(base, |px| sample::shift_copy(&mut win, dx, dy, c, px)),
                None => base.cloned(),
            },
            Plan::Sample { level, a, filter } => {
                let grid = self.pyramid.get(self.src, level);
                match sample::Window::for_dest(grid, &a, c, PROBE_CAP) {
                    Some(mut win) => self.write_over(base, |px| sample::resample(&mut win, &a, c, filter, px)),
                    None => base.cloned(),
                }
            }
        }
    }

    /// A tile drawn by `draw` (which writes every pixel and reports whether
    /// any has alpha), over `base`.
    fn write_over(&self, base: Option<&TileRef>, draw: impl FnOnce(&mut TilePixels) -> bool) -> Option<TileRef> {
        let mut out = self.pool.take();
        let any = draw(Arc::get_mut(&mut out).expect("pooled tiles are unshared"));
        match (any, base) {
            (false, b) => {
                self.pool.give(Some(out));
                b.cloned()
            }
            (true, None) => Some(out),
            (true, Some(b)) => {
                let t = self.over(&out, b);
                self.pool.give(Some(out));
                Some(t)
            }
        }
    }

    /// `src` over `base`, as a new tile.
    fn over(&self, src: &TilePixels, base: &TilePixels) -> TileRef {
        let mut out = self.pool.take();
        let px = Arc::get_mut(&mut out).expect("pooled tiles are unshared");
        *px = *base;
        blend_tile(px, src, 1.0, BlendMode::Normal);
        out
    }
}

/// Split the selected pixels off `orig`: `(lift, base)` with
/// `lift = px·m/255` (rounded) and `base = px − lift`. Mask pixels past the
/// page count as 0.
fn lift(orig: &TileGrid, sel: &Selection, w: u32, h: u32) -> (TileGrid, TileGrid) {
    let jobs: Vec<(TileCoord, &TileRef, MaskView<'_>)> =
        sel.tiles().filter_map(|(c, _)| orig.get_ref(c).map(|t| (c, t, sel.get(c)))).collect();
    let split: Vec<(TileCoord, TileRef, Option<TileRef>)> = jobs
        .par_iter()
        .filter_map(|&(c, px, view)| {
            let (ox, oy) = c.origin();
            let edge = ox + TILE_SIZE_I32 > w as i32 || oy + TILE_SIZE_I32 > h as i32;
            let mask: &MaskPixels = match view {
                MaskView::Empty => return None,
                MaskView::Full if !edge => return Some((c, px.clone(), None)),
                MaskView::Full => full_mask(),
                MaskView::Partial(m) => m,
            };
            let (mut s, mut b) = (new_tile(), new_tile());
            let (sp, bp) = (Arc::get_mut(&mut s).expect("fresh"), Arc::get_mut(&mut b).expect("fresh"));
            let (mut any_s, mut any_b) = (0u16, 0u16);
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let on_page = ox + (x as i32) < w as i32 && oy + (y as i32) < h as i32;
                    let m = if on_page { mask[y][x] as u32 } else { 0 };
                    let p = px[y][x];
                    for k in 0..4 {
                        let v = p[k] as u32;
                        let lifted = ((v * m + 127) / 255) as u16;
                        sp[y][x][k] = lifted;
                        bp[y][x][k] = p[k] - lifted;
                    }
                    any_s |= sp[y][x][3];
                    any_b |= bp[y][x][3];
                }
            }
            (any_s != 0).then(|| (c, s, (any_b != 0).then_some(b)))
        })
        .collect();
    let mut src = TileGrid::with_capacity(split.len());
    let mut base = orig.clone();
    for (c, s, b) in split {
        src.insert(c, s);
        base.replace(c, b);
    }
    (src, base)
}

/// Tight pixel bbox of alpha > 0, or `None` when `g` is transparent.
fn alpha_bounds(g: &TileGrid) -> Option<RectF> {
    let tiles: Vec<(TileCoord, &TileRef)> = g.iter().collect();
    let b = tiles
        .par_iter()
        .filter_map(|&(c, t)| {
            let (ox, oy) = c.origin();
            let mut b: Option<(i32, i32, i32, i32)> = None;
            for (y, row) in t.iter().enumerate() {
                let Some(x0) = row.iter().position(|p| p[3] != 0) else { continue };
                let x1 = row.iter().rposition(|p| p[3] != 0).unwrap_or(x0);
                let (x0, x1, y) = (ox + x0 as i32, ox + x1 as i32 + 1, oy + y as i32);
                b = Some(match b {
                    None => (x0, y, x1, y + 1),
                    Some((a0, b0, a1, _)) => (a0.min(x0), b0, a1.max(x1), y + 1),
                });
            }
            b
        })
        .reduce_with(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))?;
    Some(RectF { x: b.0 as f32, y: b.1 as f32, w: (b.2 - b.0) as f32, h: (b.3 - b.1) as f32 })
}

/// `sel` moved by `xf` (bilinear) on a `w`×`h` page.
pub fn transform_mask(sel: &Selection, xf: &Affine64, w: u32, h: u32) -> Selection {
    if sel.is_empty() || is_identity(xf) {
        return sel.clone();
    }
    let (Some(inv), Some(b)) = (xf.inverse(), sel.bounds()) else { return Selection::default() };
    let t = TILE_SIZE as f32;
    let rect = RectF { x: b.x0 as f32 * t, y: b.y0 as f32 * t, w: (b.x1 - b.x0) as f32 * t, h: (b.y1 - b.y0) as f32 * t };
    let (tw, th) = (w.div_ceil(TILE_SIZE as u32) as i32, h.div_ceil(TILE_SIZE as u32) as i32);
    let dest: Vec<TileCoord> = dest_tiles(rect, 1.0, xf, tw, th)
        .into_iter()
        .filter(|c| c.x >= 0 && c.y >= 0 && c.x < tw && c.y < th)
        .collect();
    let a = inv.then(Affine64::translate(-0.5, -0.5));
    let tiles: Vec<(TileCoord, MaskRef)> =
        dest.par_iter().filter_map(|&c| mask_tile(sel, &a, c, w, h).map(|m| (c, m))).collect();
    let mut out = Selection::default();
    for (c, m) in tiles {
        out.insert_tile(c, m);
    }
    out
}

/// One destination tile of [`transform_mask`]; `None` when all 0.
fn mask_tile(sel: &Selection, a: &Affine64, c: TileCoord, w: u32, h: u32) -> Option<MaskRef> {
    let (ox, oy) = c.origin();
    let [m0, m1, m2, m3, m4, m5] = a.m;
    let mut out = Box::new([[0u8; TILE_SIZE]; TILE_SIZE]);
    let mut cache: Option<(TileCoord, MaskView<'_>)> = None;
    let mut value = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return 0.0;
        }
        let tc = TileCoord::new(x >> 6, y >> 6);
        let view = match cache {
            Some((k, v)) if k == tc => v,
            _ => {
                let v = sel.get(tc);
                cache = Some((tc, v));
                v
            }
        };
        match view {
            MaskView::Empty => 0.0,
            MaskView::Full => 255.0,
            MaskView::Partial(m) => m[(y & 63) as usize][(x & 63) as usize] as f32,
        }
    };
    let mut any = false;
    for (y, row) in out.iter_mut().enumerate() {
        let py = (oy + y as i32) as f64 + 0.5;
        if oy + y as i32 >= h as i32 {
            break;
        }
        for (x, d) in row.iter_mut().enumerate() {
            if ox + x as i32 >= w as i32 {
                break;
            }
            let px = (ox + x as i32) as f64 + 0.5;
            let (u, v) = (m0 * px + m1 * py + m2, m3 * px + m4 * py + m5);
            let (fu, fv) = (u.floor(), v.floor());
            let (tx, ty) = ((u - fu) as f32, (v - fv) as f32);
            let (x0, y0) = (fu.clamp(-1.0e8, 1.0e8) as i32, fv.clamp(-1.0e8, 1.0e8) as i32);
            let s = (value(x0, y0) * (1.0 - tx) + value(x0 + 1, y0) * tx) * (1.0 - ty)
                + (value(x0, y0 + 1) * (1.0 - tx) + value(x0 + 1, y0 + 1) * tx) * ty;
            *d = (s + 0.5).clamp(0.0, 255.0) as u8;
            any |= *d != 0;
        }
    }
    any.then(|| Arc::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_compose_about_the_pivot() {
        let pivot = [10.0, 20.0];
        assert_eq!(XfParams::identity(pivot).affine().apply([3.0, 4.0]), [3.0, 4.0]);
        let p = XfParams { t: [5.0, -1.0], s: [2.0, -1.0], theta: std::f64::consts::FRAC_PI_2, pivot };
        let a = p.affine();
        // The pivot only translates.
        let q = a.apply(pivot);
        assert!((q[0] - 15.0).abs() < 1e-9 && (q[1] - 19.0).abs() < 1e-9, "{q:?}");
        // (pivot + (1, 0)) scales to +2 in x, then turns to +2 in y.
        let q = a.apply([11.0, 20.0]);
        assert!((q[0] - 15.0).abs() < 1e-9 && (q[1] - 21.0).abs() < 1e-9, "{q:?}");
    }

    #[test]
    fn plans_pick_the_fast_paths() {
        let t = |x, y| Affine64::translate(x, y);
        assert!(matches!(Plan::new(&t(128.0, -64.0), Filter::Bicubic), Some(Plan::ShiftTiles { dx: 2, dy: -1 })));
        assert!(matches!(Plan::new(&t(3.0, -64.0), Filter::Bicubic), Some(Plan::ShiftPx { dx: 3, dy: -64 })));
        assert!(matches!(Plan::new(&t(3.5, 0.0), Filter::Bicubic), Some(Plan::Sample { level: 0, .. })));
        let down = Affine64::scale(0.125, 0.125);
        assert!(matches!(Plan::new(&down, Filter::Nearest), Some(Plan::Sample { level: 3, .. })));
        let thin = Affine64::scale(1.0, 0.3);
        assert!(matches!(Plan::new(&thin, Filter::Nearest), Some(Plan::Sample { level: 1, .. })));
        assert!(Plan::new(&Affine64::scale(0.0, 1.0), Filter::Nearest).is_none());
    }

    #[test]
    fn dest_tiles_follow_a_rotated_quad() {
        let rect = RectF { x: 0.0, y: 0.0, w: 640.0, h: 64.0 };
        // A thin bar rotated 45° about its start: tiles off the diagonal are skipped.
        let xf = Affine64::rotate(std::f64::consts::FRAC_PI_4);
        let d = dest_tiles(rect, 0.0, &xf, 100, 100);
        assert!(d.contains(&TileCoord::new(3, 3)));
        assert!(!d.contains(&TileCoord::new(6, 0)), "{d:?}");
        assert!(d.len() < 40, "{}", d.len());
        // Everything stays within the page ± one page.
        let big = dest_tiles(rect, 0.0, &Affine64::scale(100.0, 100.0), 4, 4);
        assert!(big.iter().all(|c| (-4..8).contains(&c.x) && (-4..8).contains(&c.y)));
    }
}
