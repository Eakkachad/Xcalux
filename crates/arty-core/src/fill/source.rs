//! What the flood compares against: layer tiles or the flattened page,
//! read lazily one tile at a time.

use std::cell::RefCell;

use crate::blend::{BlendMode, blend_tile};
use crate::composite::CompositeScratch;
use crate::document::Document;
use crate::fill::FillRef;
use crate::fix15::ONE;
use crate::grid::TileGrid;
use crate::layer::{LayerContent, LayerId};
use crate::tile::{TILE_SIZE, TileCoord, TilePixels, clear_tile, new_tile_box};

use super::bits::{Bits, Geom, ZERO};

/// One reference tile.
pub(super) enum Px<'a> {
    /// Every pixel has this value (absent layer tiles are transparent).
    Const([u16; 4]),
    Tile(&'a TilePixels),
}

enum Kind<'a> {
    /// Raster layers, bottom to top, flattened with plain "over".
    Layers(Vec<&'a TileGrid>),
    /// `composite_tile`: every visible layer and the paper.
    Composite,
}

/// Per-thread buffers for reading reference tiles.
pub(super) struct Tls {
    buf: Box<TilePixels>,
    cs: Option<CompositeScratch>,
}

impl Tls {
    pub fn new() -> Tls {
        Tls { buf: new_tile_box(), cs: None }
    }
}

thread_local! {
    static TLS: RefCell<Option<Tls>> = const { RefCell::new(None) };
}

/// Run `f` with this thread's buffers (kept between fills, so worker
/// threads do not allocate a composite scratch per task).
pub(super) fn with_tls<R>(f: impl FnOnce(&mut Tls) -> R) -> R {
    TLS.with(|t| f(t.borrow_mut().get_or_insert_with(Tls::new)))
}

pub(super) struct Source<'a> {
    doc: &'a Document,
    kind: Kind<'a>,
    pub geom: Geom,
    seed: [u16; 4],
    tol: u16,
    /// Strength of the seed pixel (see [`Source::strength`]).
    pub seed_strength: u16,
}

impl<'a> Source<'a> {
    /// `seed` must be on the page.
    pub fn new(doc: &'a Document, r: FillRef, seed: (i32, i32), tol: u16, tls: &mut Tls) -> Source<'a> {
        let kind = match r {
            FillRef::AllVisible => Kind::Composite,
            FillRef::Active => Kind::Layers(active_layers(doc)),
            FillRef::Reference => {
                let mut refs = Vec::new();
                collect(doc, doc.root(), false, true, &mut refs);
                // No reference layer: behave like Active rather than fill
                // the whole page.
                Kind::Layers(if refs.is_empty() { active_layers(doc) } else { refs })
            }
        };
        let mut s = Source { doc, kind, geom: Geom::new(doc.width(), doc.height()), seed: [0; 4], tol, seed_strength: 0 };
        let c = TileCoord::from_pixel(seed.0, seed.1);
        let (ox, oy) = c.origin();
        let p = match s.tile(c, tls) {
            Px::Const(v) => v,
            Px::Tile(t) => t[(seed.1 - oy) as usize][(seed.0 - ox) as usize],
        };
        s.seed = p;
        s.seed_strength = s.strength(p);
        s
    }

    /// The reference pixels of tile `c`.
    pub fn tile<'b>(&'b self, c: TileCoord, tls: &'b mut Tls) -> Px<'b> {
        match &self.kind {
            Kind::Composite => {
                let cs = tls.cs.get_or_insert_with(CompositeScratch::new);
                self.doc.composite_tile(c, &mut tls.buf, cs);
                Px::Tile(&tls.buf)
            }
            Kind::Layers(grids) => {
                let mut present = grids.iter().filter_map(|g| g.get(c));
                let Some(first) = present.next() else { return Px::Const([0; 4]) };
                let Some(second) = present.next() else { return Px::Tile(first) };
                let buf = &mut *tls.buf;
                clear_tile(buf);
                blend_tile(buf, first, 1.0, BlendMode::Normal);
                blend_tile(buf, second, 1.0, BlendMode::Normal);
                for t in present {
                    blend_tile(buf, t, 1.0, BlendMode::Normal);
                }
                Px::Tile(buf)
            }
        }
    }

    /// Within tolerance of the seed on every premultiplied channel.
    #[inline]
    pub fn passes(&self, p: [u16; 4]) -> bool {
        let s = self.seed;
        let d = p[0].abs_diff(s[0]).max(p[1].abs_diff(s[1])).max(p[2].abs_diff(s[2])).max(p[3].abs_diff(s[3]));
        d <= self.tol
    }

    /// How much line a pixel holds (fix15): alpha for layers, darkness
    /// over white for the flattened page.
    #[inline]
    pub fn strength(&self, p: [u16; 4]) -> u16 {
        match self.kind {
            Kind::Layers(_) => p[3],
            Kind::Composite => {
                // Rec. 709 luma of the premultiplied colour (weights sum to ONE).
                let luma = (p[0] as u32 * 6966 + p[1] as u32 * 23436 + p[2] as u32 * 2366) >> 15;
                (p[3] as u32).saturating_sub(luma).min(ONE) as u16
            }
        }
    }

    /// The pixels of tile `(tx, ty)` that pass. Off-page pixels never do.
    pub fn pass_bits(&self, tx: i32, ty: i32, tls: &mut Tls) -> Bits {
        let g = self.geom;
        let (rows, cols) = (g.rows(ty), g.cols(tx));
        match self.tile(TileCoord::new(tx, ty), tls) {
            Px::Const(v) => {
                if self.passes(v) {
                    g.mask(tx, ty)
                } else {
                    ZERO
                }
            }
            Px::Tile(t) => {
                let mut out = ZERO;
                let seed = self.seed;
                let tol = self.tol;
                for (o, row) in out[..rows].iter_mut().zip(t.iter()) {
                    // Channel by channel, so the compare vectorizes.
                    let flat = row.as_flattened();
                    let mut ok = [0u8; TILE_SIZE * 4];
                    for (k, (o, v)) in ok.iter_mut().zip(flat).enumerate() {
                        *o = (v.abs_diff(seed[k & 3]) <= tol) as u8;
                    }
                    let mut b = 0u64;
                    for (x, q) in ok.chunks_exact(4).enumerate() {
                        b |= ((q[0] & q[1] & q[2] & q[3]) as u64) << x;
                    }
                    *o = b & cols;
                }
                out
            }
        }
    }

    /// Strength of every pixel of tile `c`, `[y][x]`.
    pub fn strengths(&self, c: TileCoord, tls: &mut Tls, out: &mut [[u16; TILE_SIZE]; TILE_SIZE]) {
        match self.tile(c, tls) {
            Px::Const(v) => {
                let s = self.strength(v);
                for row in out.iter_mut() {
                    row.fill(s);
                }
            }
            Px::Tile(t) => {
                for (o, row) in out.iter_mut().zip(t.iter()) {
                    for (s, p) in o.iter_mut().zip(row) {
                        *s = self.strength(*p);
                    }
                }
            }
        }
    }
}

/// The active layer, or a folder's visible raster descendants.
fn active_layers(doc: &Document) -> Vec<&TileGrid> {
    let layer = doc.active_layer();
    match &layer.content {
        LayerContent::Raster(g) => vec![g],
        LayerContent::Folder { children, .. } => {
            let mut out = Vec::new();
            collect(doc, children, false, false, &mut out);
            out
        }
    }
}

/// Visible raster layers under `ids`, bottom to top. With `want_ref`, only
/// those marked as reference layers themselves or through a folder.
fn collect<'a>(doc: &'a Document, ids: &[LayerId], inherited: bool, want_ref: bool, out: &mut Vec<&'a TileGrid>) {
    for id in ids {
        let Some(l) = doc.layer(*id) else { continue };
        if !l.props.visible {
            continue;
        }
        let is_ref = inherited || l.props.reference;
        match &l.content {
            LayerContent::Raster(g) => {
                if !want_ref || is_ref {
                    out.push(g);
                }
            }
            LayerContent::Folder { children, .. } => collect(doc, children, is_ref, want_ref, out),
        }
    }
}
