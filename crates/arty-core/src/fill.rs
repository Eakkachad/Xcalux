//! Bucket fill and magic wand regions: a scanline flood on pass bits with
//! gap closing, area scaling and antialiasing, written as one pixel edit.
//!
//! The flood works on bit tiles (one `u64` per tile row) in a dense
//! page-indexed grid. Reference tiles are classified lazily: spans that
//! reach a tile not classified yet are parked on it, and each round
//! classifies every parked tile, and its neighbours, in parallel (a tile
//! wavefront).
//!
//! Gap closing with radius R: the plain flood gives an upper bound U; the
//! walls are dilated by a disk of radius R; the clicked component of the
//! rest of U is the core, and the core regrows into the dilation ring
//! against every other core (ties go to the others), so a gap narrower
//! than 2R no longer leaks. A corridor narrower than 2R between two cores
//! is split down the middle. Regrowth runs 16 synchronous steps per round
//! on each tile plus a 16 px border, which stays exact (see `gap.rs`).
//!
//! Pixels outside the page are never filled, and the page border is not a
//! wall for gap closing.

mod bits;
mod gap;
mod scale;
mod source;
mod write;

use std::collections::VecDeque;
use std::sync::Arc;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::fix15::ONE;
use crate::history::Edit;
use crate::layer::LayerId;
use crate::selection::{MaskPixels, MaskRef, MaskView, Selection, full_mask};
use crate::tile::{TILE_SIZE, TileCoord};

use bits::{BitGrid, Bits, Geom, Kind, ONES, ZERO, dilate, hfill};
use source::{Source, with_tls};

/// What the flood compares against the seed colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FillRef {
    /// The active layer.
    #[default]
    Active,
    /// The flattened page, paper included.
    AllVisible,
    /// Layers with `LayerProps::reference`.
    Reference,
}

/// How area scaling grows the region into the line art.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScaleMode {
    #[default]
    Plain,
    /// Advance only towards darker (stronger) pixels.
    ToDarkest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FillBlend {
    /// Over the layer.
    #[default]
    Normal,
    /// Under the layer's pixels (dst-over).
    Behind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillParams {
    pub reference: FillRef,
    /// Largest per-channel distance to the seed that still passes (fix15).
    pub tolerance: u16,
    /// Gap-closing radius in px (see [`gap_radius`]).
    pub gap_px: u8,
    /// Px to grow (> 0) or shrink (< 0) the region.
    pub area_scale: i8,
    pub scale_mode: ScaleMode,
    pub contiguous: bool,
    pub antialias: bool,
    /// Limit the region to the current selection (the fill tool always
    /// does; the magic wand never does).
    pub use_selection: bool,
}

impl Default for FillParams {
    fn default() -> Self {
        Self {
            reference: FillRef::Active,
            tolerance: 0,
            gap_px: 0,
            area_scale: 0,
            scale_mode: ScaleMode::Plain,
            contiguous: true,
            antialias: true,
            use_selection: true,
        }
    }
}

/// Buffers reused across fills.
#[derive(Default)]
pub struct FillScratch {
    /// Pixels that pass, per tile; unset until classified.
    pass: BitGrid,
    /// The plain flood (U), then the region.
    region: BitGrid,
    /// Gap closing: the cores, the regrown region and the other cores.
    core: BitGrid,
    grown: BitGrid,
    other: BitGrid,
    queue: VecDeque<u32>,
    parked: Vec<u32>,
    flags: Vec<u8>,
}

/// Gap-closing radius in px for UI level 0..=5 at `dpi`
/// (`{0, 2, 4, 8, 16, 32}·dpi/600`, at most 64).
pub fn gap_radius(level: u8, dpi: u32) -> u8 {
    const BASE: [u64; 6] = [0, 2, 4, 8, 16, 32];
    let base = BASE[level.min(5) as usize];
    if base == 0 {
        return 0;
    }
    ((base * dpi as u64 + 300) / 600).clamp(1, 64) as u8
}

/// The region a click at `seed` fills, as coverage. `None` when nothing
/// would be filled.
pub fn fill_region(doc: &Document, seed: (i32, i32), p: &FillParams, s: &mut FillScratch) -> Option<Selection> {
    let g = Geom::new(doc.width(), doc.height());
    g.pixel(seed.0, seed.1)?;
    let sel = (p.use_selection && doc.has_selection()).then(|| doc.selection());
    if sel.is_some_and(|sel| sel.value(seed.0, seed.1) == 0) {
        return None;
    }
    let src = with_tls(|tls| Source::new(doc, p.reference, seed, p.tolerance, tls));
    let n = g.len();
    s.pass.reset(n, true);
    s.flags.clear();
    s.flags.resize(n, 0);

    if p.contiguous {
        let mut fl = Flood {
            g,
            pass: &mut s.pass,
            fill: &mut s.region,
            queue: &mut s.queue,
            parked: &mut s.parked,
            flags: &mut s.flags,
            src: Some(&src),
        };
        fl.run(seed);
    } else {
        let all: Vec<u32> = (0..n as u32).collect();
        classify(&src, &mut s.pass, &all);
        s.region.reset(n, false);
        for i in 0..n {
            s.region.set(i, s.pass.get(i));
        }
    }

    let r = p.gap_px.min(64) as u32;
    let scale = p.area_scale as i32;
    if r > 0 || scale != 0 || p.antialias {
        // Every later step reads at most one tile past the region.
        let mut need = Vec::new();
        for i in 0..n {
            if s.region.any(i) {
                for j in g.around(i) {
                    if !s.pass.is_set(j) && s.flags[j] == 0 {
                        s.flags[j] = 1;
                        need.push(j as u32);
                    }
                }
            }
        }
        classify(&src, &mut s.pass, &need);
        s.flags.fill(0);
    }
    if r > 0 && p.contiguous {
        close_gaps(&g, seed, r, s);
    }
    // Strengths To darkest already built, for the antialiasing to reuse.
    let mut strengths = Vec::new();
    if scale > 0 {
        match p.scale_mode {
            ScaleMode::Plain => scale::grow_plain(&g, &s.pass, &mut s.region, scale as u32, &mut s.flags),
            ScaleMode::ToDarkest => strengths = scale::grow_darkest(&g, &src, &s.pass, &mut s.region, scale as u32),
        }
    } else if scale < 0 {
        scale::shrink(&g, &mut s.region, &mut s.core, scale.unsigned_abs(), &mut s.flags);
    }
    let out = coverage(&g, &src, &s.region, &s.pass, p.antialias, sel, &strengths, &mut s.flags);
    (!out.is_empty()).then_some(out)
}

/// Paint `color` (fix15 premultiplied) over `region` on `layer`. `None`
/// when nothing changed or the layer refuses (locked, folder).
pub fn apply_fill(
    doc: &mut Document,
    layer: LayerId,
    region: &Selection,
    color: [u16; 4],
    opacity: f32,
    blend: FillBlend,
) -> Option<Edit> {
    let l = doc.layer(layer)?;
    if l.props.locked {
        return None;
    }
    let grid = l.raster()?;
    let lock_alpha = l.props.lock_alpha;
    // Behind only adds alpha, which a locked alpha forbids.
    if lock_alpha && blend == FillBlend::Behind {
        return None;
    }
    let a = color[3].min(ONE as u16);
    let paint = write::Paint {
        color: [color[0].min(a), color[1].min(a), color[2].min(a), a],
        opacity: crate::fix15::from_f32(opacity) as u32,
        blend,
        lock_alpha,
    };
    if paint.opacity == 0 || a == 0 {
        return None;
    }
    let tiles: Vec<(TileCoord, &MaskRef)> = region.tiles().filter(|(c, _)| doc.contains_tile(*c)).collect();
    let new = write::paint_tiles(&paint, |c| grid.get_ref(c), &tiles, doc.width() as i32, doc.height() as i32);
    if new.is_empty() {
        return None;
    }
    let (grid, dirty) = doc.paint_target(layer)?;
    let mut old = Vec::with_capacity(new.len());
    for (c, t) in new {
        old.push((c, grid.replace(c, Some(t))));
        dirty.mark(c);
    }
    Some(Edit::Pixels { layer, tiles: old })
}

/// Fill the whole selection on `layer` with `color`.
pub fn fill_selection(doc: &mut Document, layer: LayerId, color: [u16; 4]) -> Option<Edit> {
    if !doc.has_selection() {
        return None;
    }
    let region = doc.selection().clone();
    apply_fill(doc, layer, &region, color, 1.0, FillBlend::Normal)
}

// ----- classification --------------------------------------------------------

/// Classify `tiles` (unset slots of `pass`) in parallel.
fn classify(src: &Source, pass: &mut BitGrid, tiles: &[u32]) {
    if tiles.is_empty() {
        return;
    }
    let g = src.geom;
    let out: Vec<(u32, Bits)> = tiles
        .par_iter()
        .map(|&i| {
            let (tx, ty) = g.coord(i as usize);
            (i, with_tls(|tls| src.pass_bits(tx, ty, tls)))
        })
        .collect();
    for (i, b) in &out {
        pass.set(*i as usize, b);
    }
}

// ----- flood -------------------------------------------------------------------

const QUEUED: u8 = 1;
const PARKED: u8 = 2;
const AHEAD: u8 = 4;

/// A 4-connected flood of `pass` from a seed into `fill`.
struct Flood<'a, 's> {
    g: Geom,
    pass: &'a mut BitGrid,
    fill: &'a mut BitGrid,
    queue: &'a mut VecDeque<u32>,
    parked: &'a mut Vec<u32>,
    flags: &'a mut Vec<u8>,
    /// Classifies unset tiles; without it they read as not passing.
    src: Option<&'a Source<'s>>,
}

impl Flood<'_, '_> {
    fn run(&mut self, seed: (i32, i32)) {
        let n = self.g.len();
        self.fill.reset(n, false);
        self.queue.clear();
        self.parked.clear();
        self.flags.clear();
        self.flags.resize(n, 0);
        let Some((i, y, x)) = self.g.pixel(seed.0, seed.1) else { return };
        self.deliver(i, y, 1 << x);
        loop {
            while let Some(i) = self.queue.pop_front() {
                self.flags[i as usize] &= !QUEUED;
                self.spread(i as usize);
            }
            if self.parked.is_empty() {
                break;
            }
            let parked = std::mem::take(self.parked);
            if let Some(src) = self.src {
                // The parked tiles and their neighbours: the flood will
                // likely reach those next, and a bigger round halves the
                // number of rounds.
                let mut unset = Vec::new();
                for &i in &parked {
                    for j in self.g.around(i as usize) {
                        if !self.pass.is_set(j) && self.flags[j] & AHEAD == 0 {
                            self.flags[j] |= AHEAD;
                            unset.push(j as u32);
                        }
                    }
                }
                classify(src, self.pass, &unset);
                for &j in &unset {
                    self.flags[j as usize] &= !AHEAD;
                }
            }
            for &i in &parked {
                let i = i as usize;
                self.flags[i] &= !PARKED;
                // The parked seeds, now limited to what passes.
                let (p, f) = (self.pass.get(i), self.fill.get(i));
                let mut b = ZERO;
                for y in 0..TILE_SIZE {
                    b[y] = f[y] & p[y];
                }
                self.fill.set(i, &b);
                if self.fill.any(i) && self.flags[i] & QUEUED == 0 {
                    self.flags[i] |= QUEUED;
                    self.queue.push_back(i as u32);
                }
            }
            *self.parked = parked;
            self.parked.clear();
        }
    }

    /// Seed row `y` of tile `j` with `bits`.
    #[inline]
    fn deliver(&mut self, j: usize, y: usize, bits: u64) {
        if !self.pass.is_set(j) {
            if self.src.is_none() {
                return;
            }
            self.fill.get_mut(j)[y] |= bits;
            if self.flags[j] & PARKED == 0 {
                self.flags[j] |= PARKED;
                self.parked.push(j as u32);
            }
            return;
        }
        let new = bits & self.pass.get(j)[y] & !self.fill.get(j)[y];
        if new == 0 {
            return;
        }
        self.fill.get_mut(j)[y] |= new;
        if self.flags[j] & QUEUED == 0 {
            self.flags[j] |= QUEUED;
            self.queue.push_back(j as u32);
        }
    }

    /// Flood inside tile `i` from its seeds, then seed its neighbours.
    fn spread(&mut self, i: usize) {
        let f = flood_tile(self.pass.get(i), self.fill.get(i), self.pass.kind(i) == Kind::Ones);
        self.fill.set(i, &f);
        let (tx, ty) = self.g.coord(i);
        if f[0] != 0
            && let Some(j) = self.g.slot(tx, ty - 1)
        {
            self.deliver(j, TILE_SIZE - 1, f[0]);
        }
        if f[TILE_SIZE - 1] != 0
            && let Some(j) = self.g.slot(tx, ty + 1)
        {
            self.deliver(j, 0, f[TILE_SIZE - 1]);
        }
        for (dx, from, to) in [(-1, 0, 63), (1, 63, 0)] {
            let Some(j) = self.g.slot(tx + dx, ty) else { continue };
            for (y, row) in f.iter().enumerate() {
                if row >> from & 1 == 1 {
                    self.deliver(j, y, 1 << to);
                }
            }
        }
    }
}

/// The pixels of `p` 4-connected to `seeds` within one tile.
fn flood_tile(p: &Bits, seeds: &Bits, all_pass: bool) -> Bits {
    if all_pass {
        return if seeds.iter().any(|&r| r != 0) { ONES } else { ZERO };
    }
    let mut f = *seeds;
    loop {
        let mut changed = false;
        let mut prev = 0u64;
        for y in 0..TILE_SIZE {
            let n = hfill(p[y], f[y] | prev);
            changed |= n != f[y];
            f[y] = n;
            prev = n;
        }
        prev = 0;
        for y in (0..TILE_SIZE).rev() {
            let n = hfill(p[y], f[y] | prev);
            changed |= n != f[y];
            f[y] = n;
            prev = n;
        }
        if !changed {
            return f;
        }
    }
}

// ----- gap closing -------------------------------------------------------------

/// Replace the plain flood in `s.region` with its gap-closed version.
fn close_gaps(g: &Geom, seed: (i32, i32), r: u32, s: &mut FillScratch) {
    let n = g.len();
    let tiles: Vec<u32> = (0..n as u32).filter(|&i| s.region.any(i as usize)).collect();
    // Core = U \ W′; tiles with ring pixels (U ∩ W′) regrow.
    s.core.reset(n, true);
    for &i in &tiles {
        s.core.set(i as usize, s.region.get(i as usize));
    }
    let mut ring = Vec::new();
    for (i, w) in gap::wall_dilation(g, &s.pass, &tiles, r) {
        let u = s.region.get(i as usize);
        let mut core = ZERO;
        let mut in_ring = false;
        for y in 0..TILE_SIZE {
            core[y] = u[y] & !w[y];
            in_ring |= u[y] & w[y] != 0;
        }
        s.core.set(i as usize, &core);
        if in_ring {
            ring.push(i);
        }
    }
    if ring.is_empty() {
        return;
    }
    // The clicked core: the seed's component, or the nearest core pixel
    // within 2R when the seed sits in the ring, or the seed alone.
    let core_at = |x: i32, y: i32| g.pixel(x, y).is_some_and(|(i, ry, rx)| s.core.get(i)[ry] >> rx & 1 == 1);
    let start = if core_at(seed.0, seed.1) { Some(seed) } else { nearest_core(g, seed, 2 * r, &s.region, &core_at) };
    match start {
        Some(p) => {
            let mut fl = Flood {
                g: *g,
                pass: &mut s.core,
                fill: &mut s.grown,
                queue: &mut s.queue,
                parked: &mut s.parked,
                flags: &mut s.flags,
                src: None,
            };
            fl.run(p);
        }
        None => {
            s.grown.reset(n, false);
            let (i, y, x) = g.pixel(seed.0, seed.1).expect("seed on page");
            s.grown.get_mut(i)[y] |= 1 << x;
        }
    }
    // Every other core competes for the ring.
    s.other.reset(n, false);
    let mut competing = false;
    for &i in &tiles {
        let i = i as usize;
        let (c, f) = (s.core.get(i), s.grown.get(i));
        let mut o = ZERO;
        for y in 0..TILE_SIZE {
            o[y] = c[y] & !f[y];
            competing |= o[y] != 0;
        }
        s.other.set(i, &o);
    }
    if !competing {
        // Nothing competes: the core regrows into all of U, which is
        // connected.
        return;
    }
    s.flags.fill(0);
    gap::regrow(g, &s.region, &mut s.grown, &mut s.other, &ring, &mut s.flags);
    std::mem::swap(&mut s.region, &mut s.grown);
}

/// The core pixel nearest to `seed` along `region` (U): a 4-connected walk
/// inside it of at most `reach` steps, so it never crosses a wall (the
/// straight-line nearest core is often the big region on the far side of
/// a thin line). Ties go to the first found, in a fixed order.
fn nearest_core(
    g: &Geom,
    seed: (i32, i32),
    reach: u32,
    region: &BitGrid,
    core_at: &dyn Fn(i32, i32) -> bool,
) -> Option<(i32, i32)> {
    let in_u = |x: i32, y: i32| g.pixel(x, y).is_some_and(|(i, ry, rx)| region.get(i)[ry] >> rx & 1 == 1);
    let side = 2 * reach as i32 + 1;
    let mut seen = vec![false; (side * side) as usize];
    let slot = |x: i32, y: i32| ((y - seed.1 + reach as i32) * side + (x - seed.0 + reach as i32)) as usize;
    let mut queue = VecDeque::from([(seed, 0u32)]);
    seen[slot(seed.0, seed.1)] = true;
    while let Some(((x, y), d)) = queue.pop_front() {
        if core_at(x, y) {
            return Some((x, y));
        }
        if d == reach {
            continue;
        }
        for (nx, ny) in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
            if (nx - seed.0).unsigned_abs() + (ny - seed.1).unsigned_abs() > reach {
                continue;
            }
            let k = slot(nx, ny);
            if !seen[k] && in_u(nx, ny) {
                seen[k] = true;
                queue.push_back(((nx, ny), d + 1));
            }
        }
    }
    None
}

// ----- coverage ----------------------------------------------------------------

/// The region as a selection: 255 inside; with `aa`, wall pixels next to
/// it get `1 − strength` (when stronger than the seed); times `sel`.
/// `known`: strengths already built, per tile slot (may be empty).
#[allow(clippy::too_many_arguments)]
fn coverage(
    g: &Geom,
    src: &Source,
    f: &BitGrid,
    pass: &BitGrid,
    aa: bool,
    sel: Option<&Selection>,
    known: &[Option<scale::Strengths>],
    seen: &mut [u8],
) -> Selection {
    let mut tiles = Vec::new();
    for i in 0..g.len() {
        if f.any(i) {
            if aa {
                for j in g.around(i) {
                    if seen[j] == 0 {
                        seen[j] = 1;
                        tiles.push(j as u32);
                    }
                }
            } else {
                tiles.push(i as u32);
            }
        }
    }
    for &j in &tiles {
        seen[j as usize] = 0;
    }
    let seed_s = src.seed_strength as u32;
    let made: Vec<(TileCoord, MaskRef)> = tiles
        .par_iter()
        .map(|&i| {
            let i = i as usize;
            let (tx, ty) = g.coord(i);
            let c = TileCoord::new(tx, ty);
            let view = sel.map_or(MaskView::Full, |s| s.get(c));
            if matches!(view, MaskView::Empty) {
                return None;
            }
            let fb = f.get(i);
            let mut edge = ZERO;
            let mut any_edge = false;
            if aa && pass.is_set(i) && pass.kind(i) != Kind::Ones {
                let d = dilate(fb, &f.nb(g, tx, ty), true);
                let (p, m) = (pass.get(i), g.mask(tx, ty));
                for y in 0..TILE_SIZE {
                    edge[y] = d[y] & !fb[y] & !p[y] & m[y];
                    any_edge |= edge[y] != 0;
                }
            }
            if !any_edge {
                if f.kind(i) == Kind::Zero {
                    return None;
                }
                if matches!(view, MaskView::Full) && *fb == g.mask(tx, ty) {
                    return Some((c, full_mask().clone()));
                }
            }
            let mut m: MaskPixels = [[0; TILE_SIZE]; TILE_SIZE];
            for (row, bits) in m.iter_mut().zip(fb) {
                let mut b = *bits;
                while b != 0 {
                    row[b.trailing_zeros() as usize] = 255;
                    b &= b - 1;
                }
            }
            if any_edge {
                let mut own = [[0u16; TILE_SIZE]; TILE_SIZE];
                let st = match known.get(i).and_then(Option::as_deref) {
                    Some(st) => st,
                    None => {
                        with_tls(|tls| src.strengths(c, tls, &mut own));
                        &own
                    }
                };
                for (y, bits) in edge.iter().enumerate() {
                    let mut b = *bits;
                    while b != 0 {
                        let x = b.trailing_zeros() as usize;
                        b &= b - 1;
                        let s = st[y][x] as u32;
                        if s > seed_s {
                            m[y][x] = (((ONE - s) * 255 + ONE / 2) >> 15) as u8;
                        }
                    }
                }
            }
            if let MaskView::Partial(sm) = view {
                for (row, srow) in m.iter_mut().zip(sm) {
                    for (v, s) in row.iter_mut().zip(srow) {
                        *v = ((*v as u32 * *s as u32 + 127) / 255) as u8;
                    }
                }
            }
            Some((c, Arc::new(m)))
        })
        .flatten()
        .collect();
    let mut out = Selection::new();
    for (c, m) in made {
        out.insert_tile(c, m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_radius_scales_with_dpi() {
        assert_eq!([0, 1, 2, 3, 4, 5].map(|l| gap_radius(l, 600)), [0, 2, 4, 8, 16, 32]);
        assert_eq!([0, 1, 2, 3, 4, 5].map(|l| gap_radius(l, 300)), [0, 1, 2, 4, 8, 16]);
        assert_eq!(gap_radius(5, 2400), 64, "capped");
        assert_eq!(gap_radius(1, 72), 1, "never rounds a gap level to 0");
        assert_eq!(gap_radius(9, 600), 32);
    }

    #[test]
    fn flood_tile_follows_a_maze() {
        // A serpentine corridor: rows alternate open, with one gap at
        // alternating ends.
        let mut p = ZERO;
        for y in 0..TILE_SIZE {
            p[y] = if y % 2 == 0 { !0 } else if y % 4 == 1 { 1 << 63 } else { 1 };
        }
        let mut seeds = ZERO;
        seeds[0] = 1;
        assert_eq!(flood_tile(&p, &seeds, false), p);
        let mut cut = p;
        cut[33] = 0;
        let f = flood_tile(&cut, &seeds, false);
        assert!(f[..33] == cut[..33] && f[33..].iter().all(|&r| r == 0));
    }
}
