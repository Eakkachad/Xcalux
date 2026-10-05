//! Frame border folders (CSP コマ枠): convex panels that mask a folder's
//! children and draw a border inside each panel.
//!
//! A [`Frame`] is immutable: built once per edit at edit time (allocating),
//! shared by `Arc`, and read per tile by the compositor without allocating.
//!
//! Orientation: a valid panel has a positive shoelace area
//! (`Σ xᵢ·yᵢ₊₁ − xᵢ₊₁·yᵢ > 0`), which the docs call counter-clockwise
//! (on screen, with y down, it runs clockwise). Its interior is on the
//! left of every edge, `cross(pᵢ₊₁ − pᵢ, x − pᵢ) ≥ 0`, and edge `i` runs
//! from vertex `i` to vertex `i + 1`.

use std::sync::Arc;

use rayon::prelude::*;

use crate::document::Document;
use crate::fix15::{self, ONE};
use crate::geom::{Pt, RectF};
use crate::layer::LayerId;
use crate::tile::{TILE_SIZE, TileCoord, TilePixels};

pub const MAX_PANELS: usize = 1024;
pub const MAX_PANEL_VERTS: usize = 64;

/// Cut pieces smaller than this (px²) are dropped.
pub const MIN_PIECE_AREA: f32 = 16.0;

/// Edges shorter than this (px) make a panel invalid.
const MIN_EDGE: f64 = 1e-3;
/// Points closer than this (px) are merged when clipping.
const MERGE_DIST: f32 = 1e-3;
/// A vertex whose edges turn right by more than this (sine) is concave.
const CONCAVE_SIN: f64 = 1e-5;

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: Pt, b: Pt) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Pt, b: Pt) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn len(a: Pt) -> f32 {
    a[0].hypot(a[1])
}

/// Twice the signed area (f64, so long thin panels stay exact enough).
fn signed_area2(pts: &[Pt]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a[0] as f64 * b[1] as f64 - b[0] as f64 * a[1] as f64
        })
        .sum()
}

/// Keep the part of the convex polygon `pts` where `n·x ≥ d`
/// (Sutherland–Hodgman), merging the duplicate points clipping makes.
fn clip_pts(pts: &[Pt], n: Pt, d: f32) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(pts.len() + 1);
    let push = |p: Pt, out: &mut Vec<Pt>| {
        if out.last().is_none_or(|q| len(sub(p, *q)) > MERGE_DIST) {
            out.push(p);
        }
    };
    for i in 0..pts.len() {
        let (cur, nxt) = (pts[i], pts[(i + 1) % pts.len()]);
        let (dc, dn) = (dot(n, cur) - d, dot(n, nxt) - d);
        if dc >= 0.0 {
            push(cur, &mut out);
        }
        if (dc >= 0.0) != (dn >= 0.0) {
            let t = dc / (dc - dn);
            push([cur[0] + (nxt[0] - cur[0]) * t, cur[1] + (nxt[1] - cur[1]) * t], &mut out);
        }
    }
    while out.len() > 1 && len(sub(out[0], out[out.len() - 1])) <= MERGE_DIST {
        out.pop();
    }
    out
}

/// Intersection of the lines `p + t·r` and `q + s·u`.
fn line_meet(p: Pt, r: Pt, q: Pt, u: Pt) -> Option<Pt> {
    let den = cross(r, u);
    if den.abs() <= 1e-9 * len(r) * len(u) {
        return None;
    }
    let t = cross(sub(q, p), u) / den;
    Some([p[0] + r[0] * t, p[1] + r[1] * t])
}

/// A convex panel: 3..=64 vertices, area > 0, counter-clockwise (see the
/// module docs). Vertices may lie outside the canvas within
/// `[-page, 2·page]` (bleed panels, 裁ち切り).
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pts: Vec<Pt>,
}

impl Panel {
    /// A panel from `pts` if they form a valid one; clockwise input is
    /// reversed.
    pub fn new(mut pts: Vec<Pt>) -> Option<Panel> {
        if pts.len() >= 3 && signed_area2(&pts) < 0.0 {
            pts.reverse();
        }
        Self::checked(pts)
    }

    /// `pts` as they are, if valid: finite, no tiny edges, positive area,
    /// convex (collinear vertices allowed) and turning once around.
    fn checked(mut pts: Vec<Pt>) -> Option<Panel> {
        if pts.len() > MAX_PANEL_VERTS {
            // Clipping can add a collinear vertex; drop those first.
            drop_collinear(&mut pts);
        }
        let n = pts.len();
        if !(3..=MAX_PANEL_VERTS).contains(&n) || pts.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        if signed_area2(&pts) <= 0.0 {
            return None;
        }
        let edge = |i: usize| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            [b[0] as f64 - a[0] as f64, b[1] as f64 - a[1] as f64]
        };
        let mut turning = 0.0f64;
        for i in 0..n {
            let (e0, e1) = (edge((i + n - 1) % n), edge(i));
            let (l0, l1) = (e0[0].hypot(e0[1]), e1[0].hypot(e1[1]));
            if l1 < MIN_EDGE {
                return None;
            }
            let c = (e0[0] * e1[1] - e0[1] * e1[0]) / (l0 * l1);
            let d = (e0[0] * e1[0] + e0[1] * e1[1]) / (l0 * l1);
            // A right turn is concave; a reversal is a spike.
            if c < -CONCAVE_SIN || (c.abs() <= CONCAVE_SIN && d < 0.0) {
                return None;
            }
            turning += c.atan2(d);
        }
        // All left turns can still wind twice (a pentagram).
        if (turning - std::f64::consts::TAU).abs() > 1e-3 {
            return None;
        }
        Some(Panel { pts })
    }

    /// An axis-aligned rectangle (any sign of `w`/`h`).
    pub fn rect(r: RectF) -> Option<Panel> {
        let (x0, x1) = (r.x.min(r.x + r.w), r.x.max(r.x + r.w));
        let (y0, y1) = (r.y.min(r.y + r.h), r.y.max(r.y + r.h));
        Self::new(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
    }

    pub fn points(&self) -> &[Pt] {
        &self.pts
    }

    pub fn area(&self) -> f32 {
        (signed_area2(&self.pts) * 0.5) as f32
    }

    /// The bounding box.
    pub fn bounds(&self) -> RectF {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for p in &self.pts {
            (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
        }
        RectF { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    /// Edge `i`: vertex `i` to vertex `i + 1`.
    pub fn edge(&self, i: usize) -> (Pt, Pt) {
        let n = self.pts.len();
        (self.pts[i % n], self.pts[(i + 1) % n])
    }

    /// Unit outward normal of edge `i`.
    pub fn normal(&self, i: usize) -> Pt {
        let (a, b) = self.edge(i);
        let e = sub(b, a);
        let l = len(e);
        [e[1] / l, -e[0] / l]
    }

    /// Inside or on the boundary.
    pub fn contains(&self, p: Pt) -> bool {
        (0..self.pts.len()).all(|i| {
            let (a, b) = self.edge(i);
            cross(sub(b, a), sub(p, a)) >= 0.0
        })
    }

    /// The part on the side `n·x ≥ d`.
    pub fn clip_half_plane(&self, n: Pt, d: f32) -> Option<Panel> {
        Self::checked(clip_pts(&self.pts, n, d))
    }

    /// Every edge moved in by `w` (miter joins); `None` when nothing is
    /// left (the panel is thinner than `2w`).
    pub fn inset(&self, w: f32) -> Option<Panel> {
        if w <= 0.0 {
            return Some(self.clone());
        }
        let mut pts = self.pts.clone();
        for i in 0..self.pts.len() {
            let (a, _) = self.edge(i);
            let n = self.normal(i);
            // Keep n·x ≤ n·a − w.
            pts = clip_pts(&pts, [-n[0], -n[1]], w - dot(n, a));
            if pts.len() < 3 {
                return None;
            }
        }
        Self::checked(pts)
    }

    /// The two pieces of a cut along the line `a`→`b` with a gap of `gap`:
    /// `n·x ≥ n·a + gap/2` and `n·x ≤ n·a − gap/2`, `n` the line's left
    /// normal. Pieces under [`MIN_PIECE_AREA`] are dropped.
    pub fn split(&self, a: Pt, b: Pt, gap: f32) -> (Option<Panel>, Option<Panel>) {
        let d = sub(b, a);
        let l = len(d);
        if l < MERGE_DIST || !l.is_finite() {
            return (None, None);
        }
        let n = [-d[1] / l, d[0] / l];
        let c = dot(n, a);
        let g = gap.max(0.0) * 0.5;
        let keep = |p: Option<Panel>| p.filter(|p| p.area() >= MIN_PIECE_AREA);
        (keep(self.clip_half_plane(n, c + g)), keep(self.clip_half_plane([-n[0], -n[1]], g - c)))
    }

    /// Whether the segment `a`→`b` runs through the interior and its line
    /// has vertices on both sides (so a cut along it divides the panel).
    pub fn crossed_by(&self, a: Pt, b: Pt) -> bool {
        let d = sub(b, a);
        let l = len(d);
        if l < MERGE_DIST {
            return false;
        }
        // Cyrus–Beck: clip the segment to the panel.
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for i in 0..self.pts.len() {
            let (p, q) = self.edge(i);
            let e = sub(q, p);
            // Inside: cross(e, a + t·d − p) ≥ 0.
            let (c0, dc) = (cross(e, sub(a, p)), cross(e, d));
            if dc.abs() < 1e-12 {
                if c0 < 0.0 {
                    return false;
                }
            } else if dc > 0.0 {
                t0 = t0.max(-c0 / dc);
            } else {
                t1 = t1.min(-c0 / dc);
            }
        }
        if (t1 - t0) * l <= 0.5 {
            return false;
        }
        let n = [-d[1] / l, d[0] / l];
        let side = |p: &Pt| dot(n, sub(*p, a));
        self.pts.iter().any(|p| side(p) > 0.25) && self.pts.iter().any(|p| side(p) < -0.25)
    }

    /// The chord the line through `a` with direction `d` cuts from the
    /// panel, if it crosses it.
    pub fn chord(&self, a: Pt, d: Pt) -> Option<(Pt, Pt)> {
        let l = len(d);
        if l < MERGE_DIST {
            return None;
        }
        let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
        for i in 0..self.pts.len() {
            let (p, q) = self.edge(i);
            let e = sub(q, p);
            let (c0, dc) = (cross(e, sub(a, p)), cross(e, d));
            if dc.abs() < 1e-12 {
                if c0 < 0.0 {
                    return None;
                }
            } else if dc > 0.0 {
                t0 = t0.max(-c0 / dc);
            } else {
                t1 = t1.min(-c0 / dc);
            }
        }
        (t1 > t0 && t0.is_finite() && t1.is_finite())
            .then(|| ([a[0] + d[0] * t0, a[1] + d[1] * t0], [a[0] + d[0] * t1, a[1] + d[1] * t1]))
    }

    /// Vertex `i` moved to `p`; `None` if that breaks convexity.
    pub fn with_vertex(&self, i: usize, p: Pt) -> Option<Panel> {
        let mut pts = self.pts.clone();
        *pts.get_mut(i)? = p;
        Self::checked(pts)
    }

    /// Edge `i` moved `d` px along its outward normal, its neighbours
    /// keeping their angle.
    pub fn with_edge_offset(&self, i: usize, d: f32) -> Option<Panel> {
        let n = self.pts.len();
        if i >= n {
            return None;
        }
        let (a, b) = self.edge(i);
        let nrm = self.normal(i);
        let e = sub(b, a);
        let shift = [nrm[0] * d, nrm[1] * d];
        let (prev, next) = (self.pts[(i + n - 1) % n], self.pts[(i + 2) % n]);
        let new_a = line_meet(prev, sub(a, prev), [a[0] + shift[0], a[1] + shift[1]], e)?;
        let new_b = line_meet(next, sub(b, next), [b[0] + shift[0], b[1] + shift[1]], e)?;
        let mut pts = self.pts.clone();
        pts[i] = new_a;
        pts[(i + 1) % n] = new_b;
        Self::checked(pts)
    }

    /// A vertex `p` inserted on edge `i` (after vertex `i`).
    pub fn with_inserted_vertex(&self, i: usize, p: Pt) -> Option<Panel> {
        if i >= self.pts.len() {
            return None;
        }
        let mut pts = self.pts.clone();
        pts.insert(i + 1, p);
        if pts.len() > MAX_PANEL_VERTS {
            return None;
        }
        Self::checked(pts)
    }

    pub fn without_vertex(&self, i: usize) -> Option<Panel> {
        if i >= self.pts.len() || self.pts.len() <= 3 {
            return None;
        }
        let mut pts = self.pts.clone();
        pts.remove(i);
        Self::checked(pts)
    }

    pub fn translated(&self, d: Pt) -> Panel {
        Panel { pts: self.pts.iter().map(|p| [p[0] + d[0], p[1] + d[1]]).collect() }
    }

    /// Scaled about the origin by `s > 0`; `None` if it degenerates.
    pub fn scaled(&self, s: f32) -> Option<Panel> {
        Self::checked(self.pts.iter().map(|p| [p[0] * s, p[1] * s]).collect())
    }
}

/// Remove vertices whose edges are collinear.
fn drop_collinear(pts: &mut Vec<Pt>) {
    let mut i = 0;
    while pts.len() > 3 && i < pts.len() {
        let n = pts.len();
        let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let (e0, e1) = (sub(b, a), sub(c, b));
        if (cross(e0, e1) / (len(e0) * len(e1)).max(f32::MIN_POSITIVE)).abs() <= 1e-6 && dot(e0, e1) > 0.0 {
            pts.remove(i);
        } else {
            i += 1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderStyle {
    /// Px; 0 draws no border.
    pub width: f32,
    /// fix15 premultiplied.
    pub color: [u16; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameShape {
    pub panels: Vec<Panel>,
    pub border: BorderStyle,
}

impl FrameShape {
    /// Cut every panel the segment `a`→`b` crosses, with gutters `gap_h`
    /// (between pieces stacked vertically) and `gap_v` (side by side).
    /// Returns the new shape and the indices of the B pieces; `None` when
    /// the segment divides no panel. Pieces under [`MIN_PIECE_AREA`] or
    /// narrower than twice the border are dropped; a panel that would
    /// lose both pieces (a gutter wider than it) is left whole.
    pub fn cut(&self, a: Pt, b: Pt, gap_h: f32, gap_v: f32) -> Option<(FrameShape, Vec<usize>)> {
        let d = sub(b, a);
        // A mostly horizontal line stacks the pieces vertically.
        let gap = if d[0].abs() >= d[1].abs() { gap_h } else { gap_v };
        let w = self.border.width;
        let keep = |p: Option<Panel>| p.filter(|p| w <= 0.0 || p.inset(w).is_some());
        let mut panels = Vec::with_capacity(self.panels.len() + 4);
        let mut bs = Vec::new();
        let mut crossed = false;
        for p in &self.panels {
            if !p.crossed_by(a, b) {
                panels.push(p.clone());
                continue;
            }
            let (pa, pb) = p.split(a, b, gap);
            let (pa, pb) = (keep(pa), keep(pb));
            if pa.is_none() && pb.is_none() {
                // The gutter is wider than the panel: dividing it would
                // delete it (and hide its art), so it is not divided.
                panels.push(p.clone());
                continue;
            }
            crossed = true;
            panels.extend(pa);
            if let Some(pb) = pb {
                bs.push(panels.len());
                panels.push(pb);
            }
        }
        (crossed && panels.len() <= MAX_PANELS).then_some((FrameShape { panels, border: self.border }, bs))
    }

    /// Scaled about the origin by `s > 0` (thumbnails); panels that
    /// degenerate are left out.
    pub fn scaled(&self, s: f32) -> FrameShape {
        FrameShape {
            panels: self.panels.iter().filter_map(|p| p.scaled(s)).collect(),
            border: BorderStyle { width: self.border.width * s, color: self.border.color },
        }
    }
}

pub type MaskTile = crate::selection::MaskPixels;

/// A frame's coverage of one tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cov<'a> {
    None,
    Full,
    Partial(&'a MaskTile),
}

pub struct Frame {
    shape: FrameShape,
    raster: FrameRaster,
}

/// Per page tile: 0 Outside, 1 Full, `k + 2` → `masks[k]`.
struct FrameRaster {
    tw: u32,
    th: u32,
    content: Box<[u32]>,
    border: Box<[u32]>,
    masks: Vec<Box<MaskTile>>,
}

const OUTSIDE: u32 = 0;
const FULL: u32 = 1;

impl FrameRaster {
    /// Every tile Outside.
    fn outside(tw: u32, th: u32) -> Self {
        let n = (tw * th) as usize;
        Self {
            tw,
            th,
            content: vec![0; n].into_boxed_slice(),
            border: vec![0; n].into_boxed_slice(),
            masks: Vec::new(),
        }
    }

    fn index(&self, c: TileCoord) -> Option<usize> {
        (c.x >= 0 && c.y >= 0 && (c.x as u32) < self.tw && (c.y as u32) < self.th)
            .then(|| c.y as usize * self.tw as usize + c.x as usize)
    }

    fn cov(&self, codes: &[u32], c: TileCoord) -> Cov<'_> {
        match self.index(c).map(|i| codes[i]) {
            None | Some(0) => Cov::None,
            Some(1) => Cov::Full,
            Some(k) => self.masks.get(k as usize - 2).map_or(Cov::None, |m| Cov::Partial(m)),
        }
    }
}

/// How a tile square relates to a convex panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Outside,
    Partial,
    Full,
}

fn classify(p: &Panel, x0: f32, y0: f32) -> Class {
    let s = TILE_SIZE as f32;
    let corners = [[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]];
    let mut all_in = true;
    for i in 0..p.pts.len() {
        let (a, b) = p.edge(i);
        let e = sub(b, a);
        let side = corners.map(|c| cross(e, sub(c, a)));
        // On or past one edge with every corner: interiors are disjoint.
        if side.iter().all(|&v| v <= 0.0) {
            return Class::Outside;
        }
        all_in &= side.iter().all(|&v| v >= 0.0);
    }
    if all_in { Class::Full } else { Class::Partial }
}

/// Row stride of the accumulator: 64 pixels, the column right of the tile
/// that edges clamped to its right side land in, and one spare column.
const ACC_W: usize = TILE_SIZE + 2;

/// Signed-area accumulator for one tile (font-rs style): an edge adds the
/// area it covers to the left of each pixel boundary, and a prefix sum
/// along each row gives the winding coverage.
struct Acc {
    a: Box<[f32]>,
}

impl Acc {
    fn new() -> Self {
        Self { a: vec![0.0; ACC_W * TILE_SIZE].into_boxed_slice() }
    }

    /// Add the edge `p0`→`p1` (tile-local px) with weight `sign`.
    fn edge(&mut self, p0: Pt, p1: Pt, sign: f32) {
        let s = TILE_SIZE as f32;
        if (p0[1] <= 0.0 && p1[1] <= 0.0) || (p0[1] >= s && p1[1] >= s) || p0[1] == p1[1] {
            return;
        }
        // Clip to the tile's rows: edges above or below it do not change
        // the winding of its pixels.
        let at_y = |y: f32| {
            let t = (y - p0[1]) / (p1[1] - p0[1]);
            [p0[0] + (p1[0] - p0[0]) * t, y]
        };
        let clip = |p: Pt| {
            if p[1] < 0.0 {
                at_y(0.0)
            } else if p[1] > s {
                at_y(s)
            } else {
                p
            }
        };
        let (a, b) = (clip(p0), clip(p1));
        // Split at x = 0 and x = 64 and clamp: past the left side an edge
        // covers whole rows, past the right side nothing in the tile.
        let mut cuts = [0.0f32, 1.0, 1.0, 1.0];
        let mut k = 1;
        for xc in [0.0, s] {
            if (a[0] - xc) * (b[0] - xc) < 0.0 {
                cuts[k] = (xc - a[0]) / (b[0] - a[0]);
                k += 1;
            }
        }
        cuts[k] = 1.0;
        cuts[1..k].sort_by(f32::total_cmp);
        let lerp = |t: f32| [(a[0] + (b[0] - a[0]) * t).clamp(0.0, s), a[1] + (b[1] - a[1]) * t];
        for w in 0..k {
            let (q0, q1) = (lerp(cuts[w]), lerp(cuts[w + 1]));
            self.line(q0, q1, sign);
        }
    }

    /// One edge with `0 ≤ x ≤ 64` and `0 ≤ y ≤ 64` (font-rs `draw_line`).
    fn line(&mut self, p0: Pt, p1: Pt, sign: f32) {
        if p0[1] == p1[1] {
            return;
        }
        let (dir, p0, p1) = if p0[1] < p1[1] { (sign, p0, p1) } else { (-sign, p1, p0) };
        let dxdy = (p1[0] - p0[0]) / (p1[1] - p0[1]);
        let mut x = p0[0];
        let y0 = p0[1].max(0.0) as usize;
        let y1 = (p1[1].ceil() as usize).min(TILE_SIZE);
        for y in y0..y1 {
            let row = y * ACC_W;
            let dy = ((y + 1) as f32).min(p1[1]) - (y as f32).max(p0[1]);
            let xnext = (x + dxdy * dy).clamp(0.0, TILE_SIZE as f32);
            let d = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0f = x0.floor();
            let x0i = x0f as usize;
            let x1c = x1.ceil();
            let x1i = x1c as usize;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x + xnext) - x0f;
                self.a[row + x0i] += d - d * xmf;
                self.a[row + x0i + 1] += d * xmf;
            } else {
                let s = (x1 - x0).recip();
                let x0r = x0 - x0f;
                let a0 = 0.5 * s * (1.0 - x0r) * (1.0 - x0r);
                let x1r = x1 - x1c + 1.0;
                let am = 0.5 * s * x1r * x1r;
                self.a[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.a[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0r);
                    self.a[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.a[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.a[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                self.a[row + x1i] += d * am;
            }
            x = xnext;
        }
    }

    /// Add every edge of `p`, in the tile at `(x0, y0)`. `sign` 1 adds the
    /// panel's coverage, −1 subtracts it.
    fn panel(&mut self, p: &Panel, x0: f32, y0: f32, sign: f32) {
        // Positive-area panels wind −1 in the accumulator's convention.
        let n = p.pts.len();
        for i in 0..n {
            let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
            self.edge([a[0] - x0, a[1] - y0], [b[0] - x0, b[1] - y0], -sign);
        }
    }

    /// Prefix-sum into `m` (coverage clamped to [0, 1]) and reset.
    fn resolve(&mut self, m: &mut MaskTile) -> Class {
        let (mut any, mut all) = (false, true);
        for (y, row) in m.iter_mut().enumerate() {
            let src = &mut self.a[y * ACC_W..(y + 1) * ACC_W];
            let mut acc = 0.0f32;
            for (x, v) in row.iter_mut().enumerate() {
                acc += src[x];
                *v = (acc.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                any |= *v != 0;
                all &= *v == 255;
            }
            src.fill(0.0);
        }
        match (any, all) {
            (false, _) => Class::Outside,
            (_, true) => Class::Full,
            _ => Class::Partial,
        }
    }
}

/// One partial tile to rasterize: its index and the panels touching it.
struct Job {
    tile: usize,
    /// `(panel, border)`: `border` adds the panel minus its inset.
    panels: Vec<(u32, bool)>,
}

impl Frame {
    /// Rasterize `shape` for a `page_w`×`page_h` page. Allocates; runs at
    /// edit time, partial tiles in parallel.
    pub fn build(shape: FrameShape, page_w: u32, page_h: u32) -> Arc<Frame> {
        let tiles = |side: u32| side.div_ceil(TILE_SIZE as u32);
        let mut raster = FrameRaster::outside(tiles(page_w), tiles(page_h));
        let (tw, th) = (raster.tw as i32, raster.th as i32);
        let width = shape.border.width;
        let has_border = width > 0.0 && width.is_finite();
        let insets: Vec<Option<Panel>> =
            shape.panels.iter().map(|p| if has_border { p.inset(width) } else { None }).collect();

        let ts = TILE_SIZE as f32;
        let mut content_jobs: Vec<(usize, u32)> = Vec::new();
        let mut border_jobs: Vec<(usize, u32)> = Vec::new();
        for (k, p) in shape.panels.iter().enumerate() {
            let b = p.bounds();
            let tx0 = ((b.x / ts).floor() as i32).clamp(0, tw);
            let ty0 = ((b.y / ts).floor() as i32).clamp(0, th);
            let tx1 = (((b.x + b.w) / ts).ceil() as i32).clamp(0, tw);
            let ty1 = (((b.y + b.h) / ts).ceil() as i32).clamp(0, th);
            for ty in ty0..ty1 {
                for tx in tx0..tx1 {
                    let i = ty as usize * tw as usize + tx as usize;
                    let (x0, y0) = (tx as f32 * ts, ty as f32 * ts);
                    let pc = classify(p, x0, y0);
                    match pc {
                        Class::Outside => continue,
                        Class::Full => raster.content[i] = FULL,
                        Class::Partial => content_jobs.push((i, k as u32)),
                    }
                    if has_border {
                        let qc = insets[k].as_ref().map(|q| classify(q, x0, y0));
                        match (pc, qc) {
                            (_, Some(Class::Full)) => {}
                            (Class::Full, None | Some(Class::Outside)) => raster.border[i] = FULL,
                            _ => border_jobs.push((i, k as u32)),
                        }
                    }
                }
            }
        }

        // Full beats Partial: drop jobs on tiles another panel fills.
        let group = |jobs: &mut Vec<(usize, u32)>, codes: &[u32], border: bool| -> Vec<Job> {
            jobs.sort_unstable();
            let mut out: Vec<Job> = Vec::new();
            for &(tile, k) in jobs.iter() {
                if codes[tile] == FULL {
                    continue;
                }
                match out.last_mut() {
                    Some(j) if j.tile == tile => j.panels.push((k, border)),
                    _ => out.push(Job { tile, panels: vec![(k, border)] }),
                }
            }
            out
        };
        let content_jobs = group(&mut content_jobs, &raster.content, false);
        let border_jobs = group(&mut border_jobs, &raster.border, true);

        let rasterize = |acc: &mut Acc, job: &Job| -> (Class, Box<MaskTile>) {
            let tx = (job.tile % tw as usize) as f32 * ts;
            let ty = (job.tile / tw as usize) as f32 * ts;
            let mut m: Box<MaskTile> = Box::new([[0; TILE_SIZE]; TILE_SIZE]);
            let class = if job.panels.len() > 1 && job.panels[0].1 {
                // Several panels' rings: each ring is clamped to [0, 1]
                // before the union, so one panel's inset cannot cancel
                // another's ring.
                acc_rings(acc, &shape.panels, &insets, job, tx, ty, &mut m)
            } else {
                for &(k, border) in &job.panels {
                    acc.panel(&shape.panels[k as usize], tx, ty, 1.0);
                    if border && let Some(q) = &insets[k as usize] {
                        acc.panel(q, tx, ty, -1.0);
                    }
                }
                acc.resolve(&mut m)
            };
            (class, m)
        };
        let place = |codes: &mut [u32], jobs: &[Job], masks: &mut Vec<Box<MaskTile>>| {
            let done: Vec<(Class, Box<MaskTile>)> = jobs.par_iter().map_init(Acc::new, &rasterize).collect();
            for (job, (class, m)) in jobs.iter().zip(done) {
                codes[job.tile] = match class {
                    Class::Outside => OUTSIDE,
                    Class::Full => FULL,
                    Class::Partial => {
                        masks.push(m);
                        masks.len() as u32 + 1
                    }
                };
            }
        };
        let mut masks = Vec::new();
        place(&mut raster.content, &content_jobs, &mut masks);
        place(&mut raster.border, &border_jobs, &mut masks);
        raster.masks = masks;
        Arc::new(Frame { shape, raster })
    }

    pub fn shape(&self) -> &FrameShape {
        &self.shape
    }

    /// Panel coverage of tile `c` (what shows of the children).
    pub fn content(&self, c: TileCoord) -> Cov<'_> {
        self.raster.cov(&self.raster.content, c)
    }

    /// Border coverage of tile `c`.
    pub fn border(&self, c: TileCoord) -> Cov<'_> {
        self.raster.cov(&self.raster.border, c)
    }

    /// Tiles where the frame shows anything (content or border).
    pub fn touched_tiles(&self) -> impl Iterator<Item = TileCoord> + '_ {
        let r = &self.raster;
        let tw = r.tw.max(1) as usize;
        r.content
            .iter()
            .zip(r.border.iter())
            .enumerate()
            .filter(|(_, (c, b))| **c != 0 || **b != 0)
            .map(move |(i, _)| TileCoord::new((i % tw) as i32, (i / tw) as i32))
    }

    /// Index of the topmost (last) panel containing `p`.
    pub fn hit(&self, p: Pt) -> Option<usize> {
        self.shape.panels.iter().rposition(|q| q.contains(p))
    }

    /// Page size in tiles this frame was built for.
    pub fn tiles(&self) -> (u32, u32) {
        (self.raster.tw, self.raster.th)
    }

    /// Bytes of partial-tile masks.
    pub fn mask_bytes(&self) -> usize {
        self.raster.masks.len() * std::mem::size_of::<MaskTile>()
    }

    /// Heap bytes this frame holds: itself, the tile tables, the masks and
    /// the panels (the undo budget's cost of an old frame).
    pub fn heap_bytes(&self) -> usize {
        let r = &self.raster;
        let panels = &self.shape.panels;
        size_of::<Frame>()
            + (r.content.len() + r.border.len()) * size_of::<u32>()
            + r.masks.capacity() * size_of::<Box<MaskTile>>()
            + r.masks.len() * size_of::<MaskTile>()
            + panels.capacity() * size_of::<Panel>()
            + panels.iter().map(|p| p.pts.capacity() * size_of::<Pt>()).sum::<usize>()
    }

    /// A frame whose content is Full on `full` tiles and Outside elsewhere
    /// (tests of dirty marking and history).
    #[cfg(test)]
    pub(crate) fn with_full_tiles(shape: FrameShape, page_w: u32, page_h: u32, full: &[TileCoord]) -> Arc<Frame> {
        let tiles = |side: u32| side.div_ceil(TILE_SIZE as u32);
        let mut raster = FrameRaster::outside(tiles(page_w), tiles(page_h));
        for &c in full {
            if let Some(i) = raster.index(c) {
                raster.content[i] = 1;
            }
        }
        Arc::new(Frame { shape, raster })
    }
}

/// The union of several panels' rings on one tile: each ring
/// (`P − inset P`) is resolved and clamped on its own, then the rings are
/// combined with a clamped sum.
fn acc_rings(
    acc: &mut Acc,
    panels: &[Panel],
    insets: &[Option<Panel>],
    job: &Job,
    tx: f32,
    ty: f32,
    m: &mut MaskTile,
) -> Class {
    let mut sum = [[0u16; TILE_SIZE]; TILE_SIZE];
    let mut one = [[0u8; TILE_SIZE]; TILE_SIZE];
    for &(k, _) in &job.panels {
        acc.panel(&panels[k as usize], tx, ty, 1.0);
        if let Some(q) = &insets[k as usize] {
            acc.panel(q, tx, ty, -1.0);
        }
        acc.resolve(&mut one);
        for (s, o) in sum.as_flattened_mut().iter_mut().zip(one.as_flattened()) {
            *s += *o as u16;
        }
    }
    let (mut any, mut all) = (false, true);
    for (v, s) in m.as_flattened_mut().iter_mut().zip(sum.as_flattened()) {
        *v = (*s).min(255) as u8;
        any |= *v != 0;
        all &= *v == 255;
    }
    match (any, all) {
        (false, _) => Class::Outside,
        (_, true) => Class::Full,
        _ => Class::Partial,
    }
}

// ----- compositing helpers (in place, fix15, allocation-free) ---------------

/// 8-bit coverage → fix15.
static COV_FIX15: [u16; 256] = {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = ((i as u32 * ONE + 127) / 255) as u16;
        i += 1;
    }
    t
};

/// Eight mask bytes as one word: all 0 or all 255 are skipped wholesale
/// (a partial tile is mostly both).
fn chunks(row: &[u8; TILE_SIZE]) -> impl Iterator<Item = (usize, u64)> + '_ {
    row.chunks_exact(8).enumerate().map(|(i, c)| (i * 8, u64::from_ne_bytes(c.try_into().unwrap_or([0; 8]))))
}

/// `px *= m` (isolated folders: hide what lies outside the panels).
pub fn mask_tile(px: &mut TilePixels, m: &MaskTile) {
    for (prow, mrow) in px.iter_mut().zip(m) {
        for (x, word) in chunks(mrow) {
            match word {
                u64::MAX => {}
                0 => prow[x..x + 8].fill([0; 4]),
                _ => {
                    for (p, &c) in prow[x..x + 8].iter_mut().zip(&mrow[x..x + 8]) {
                        let k = COV_FIX15[c as usize] as u32;
                        *p = p.map(|v| fix15::mul(v as u32, k) as u16);
                    }
                }
            }
        }
    }
}

/// Whether most of `m` is fully covered (in 8-pixel runs).
pub fn mostly_full(m: &MaskTile) -> bool {
    let full: usize = m.iter().map(|row| chunks(row).filter(|&(_, w)| w == u64::MAX).count()).sum();
    full * 2 > TILE_SIZE * TILE_SIZE / 8
}

/// `dst = dst + (src − dst)·m`, leaving `dst` alone where `m` is 0: the
/// same result as [`mask_toward`] with the roles of the tiles swapped.
pub fn mask_into(dst: &mut TilePixels, src: &TilePixels, m: &MaskTile) {
    for ((drow, srow), mrow) in dst.iter_mut().zip(src).zip(m) {
        for (x, word) in chunks(mrow) {
            match word {
                0 => {}
                u64::MAX => drow[x..x + 8].copy_from_slice(&srow[x..x + 8]),
                _ => {
                    for ((d, s), &c) in drow[x..x + 8].iter_mut().zip(&srow[x..x + 8]).zip(&mrow[x..x + 8]) {
                        let t = COV_FIX15[c as usize] as u32;
                        let inv = ONE - t;
                        for ch in 0..4 {
                            d[ch] = (fix15::mul(s[ch] as u32, t) + fix15::mul(d[ch] as u32, inv)) as u16;
                        }
                    }
                }
            }
        }
    }
}

/// Copy `src` into `dst` in the 8-pixel runs where `m` is not full: all
/// that [`mask_toward`] reads of its `base`.
pub fn copy_unmasked(dst: &mut TilePixels, src: &TilePixels, m: &MaskTile) {
    for ((drow, srow), mrow) in dst.iter_mut().zip(src).zip(m) {
        for (x, word) in chunks(mrow) {
            if word != u64::MAX {
                drow[x..x + 8].copy_from_slice(&srow[x..x + 8]);
            }
        }
    }
}

/// `px = base + (px − base)·m` (pass-through folders: outside the panels
/// the backdrop shows unchanged).
pub fn mask_toward(px: &mut TilePixels, base: &TilePixels, m: &MaskTile) {
    for ((prow, brow), mrow) in px.iter_mut().zip(base).zip(m) {
        for (x, word) in chunks(mrow) {
            match word {
                u64::MAX => {}
                0 => prow[x..x + 8].copy_from_slice(&brow[x..x + 8]),
                _ => {
                    for ((p, b), &c) in prow[x..x + 8].iter_mut().zip(&brow[x..x + 8]).zip(&mrow[x..x + 8]) {
                        let t = COV_FIX15[c as usize] as u32;
                        let inv = ONE - t;
                        for ch in 0..4 {
                            p[ch] = (fix15::mul(p[ch] as u32, t) + fix15::mul(b[ch] as u32, inv)) as u16;
                        }
                    }
                }
            }
        }
    }
}

/// Paint `color` (fix15 premultiplied) over `px` with coverage `line`.
pub fn over_color(px: &mut TilePixels, color: [u16; 4], line: Cov<'_>) {
    let scaled = |k: u32| color.map(|v| fix15::mul(v as u32, k));
    let over = |p: &mut [u16; 4], s: [u32; 4]| {
        let inv = ONE - s[3];
        for ch in 0..4 {
            p[ch] = (s[ch] + fix15::mul(p[ch] as u32, inv)).min(ONE) as u16;
        }
    };
    let full = scaled(ONE);
    match line {
        Cov::None => {}
        Cov::Full => {
            for p in px.as_flattened_mut() {
                over(p, full);
            }
        }
        Cov::Partial(m) => {
            for (prow, mrow) in px.iter_mut().zip(m) {
                for (x, word) in chunks(mrow) {
                    match word {
                        0 => {}
                        u64::MAX => prow[x..x + 8].iter_mut().for_each(|p| over(p, full)),
                        _ => {
                            for (p, &c) in prow[x..x + 8].iter_mut().zip(&mrow[x..x + 8]) {
                                if c != 0 {
                                    over(p, scaled(COV_FIX15[c as usize] as u32));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}


/// Add a frame folder holding `shape` and an empty raster child above the
/// active layer; the child becomes active. Inside a frame folder, the new
/// one goes directly above the outermost enclosing frame folder instead:
/// nested in it, the enclosing panels would mask it. Not recorded; callers
/// wrap it in a structure edit. `None` for a shape with no panels or when
/// layer ids are used up.
pub fn add_frame_folder(doc: &mut Document, shape: FrameShape) -> Option<LayerId> {
    if shape.panels.is_empty() {
        return None;
    }
    let active = doc.active();
    let mut at = active;
    while let Some(f) = doc.frame_folder_of(at) {
        doc.set_active(f);
        let Some(Some(parent)) = doc.location(f).map(|l| l.0) else { break };
        at = parent;
    }
    let Some(folder) = doc.add_folder() else {
        doc.set_active(active);
        return None;
    };
    if let Some(mut props) = doc.layer(folder).map(|l| l.props.clone()) {
        props.name = format!("Frame {}", folder.0);
        doc.set_props(folder, props);
    }
    let frame = Frame::build(shape, doc.width(), doc.height());
    doc.set_frame(folder, Some(frame));
    if let Some(child) = doc.add_raster_layer() {
        doc.move_layer(child, Some(folder), 0);
    }
    Some(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> FrameShape {
        FrameShape { panels: Vec::new(), border: BorderStyle { width: 4.0, color: [0, 0, 0, 1 << 15] } }
    }

    #[test]
    fn built_frame_is_sized_to_the_page() {
        let f = Frame::build(shape(), 130, 64);
        assert_eq!(f.tiles(), (3, 1));
        assert_eq!(f.shape(), &shape());
        assert_eq!(f.content(TileCoord::new(0, 0)), Cov::None);
        assert_eq!(f.border(TileCoord::new(9, 0)), Cov::None, "off the page");
        assert_eq!(f.touched_tiles().count(), 0);
        assert_eq!(f.mask_bytes(), 0);

        let f = Frame::with_full_tiles(shape(), 130, 130, &[TileCoord::new(2, 1), TileCoord::new(5, 5)]);
        assert_eq!(f.content(TileCoord::new(2, 1)), Cov::Full);
        assert_eq!(f.touched_tiles().collect::<Vec<_>>(), [TileCoord::new(2, 1)]);
    }
}
