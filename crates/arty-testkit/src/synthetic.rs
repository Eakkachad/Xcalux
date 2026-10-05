//! Synthetic manga pages for I/O benchmarks (feature `synthetic`).
//!
//! The reference page (spec §14) is B4 at 600 dpi with 30 raster layers
//! in 4 folders: 10 line-art layers covering 35% of the page's tiles, 8
//! flat fills at 40%, 6 dot-tone layers at 50% and 6 gradients at 20%,
//! about 135k tiles (4.2 GB raw). Each layer covers a few smooth blobs of
//! tiles; tiles on a blob's edge are cut by an anti-aliased edge. Every
//! tile is its own `Arc`, as painting makes them. The output depends only
//! on the page, so runs are comparable.

use std::f64::consts::{FRAC_1_SQRT_2, PI, TAU};
use std::sync::Arc;

use arty_core::tile::{TILE_SIZE, new_tile};
use arty_core::{DocParts, Document, Layer, LayerContent, LayerId, LayerProps, PAPER_WHITE, TileCoord, TileGrid, TileRef};
use rayon::prelude::*;

const ONE: f64 = (1u32 << 15) as f64;

/// Page size and resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
}

impl Page {
    /// B4 (257 × 364 mm) at 600 dpi: 95 × 135 tiles.
    pub const B4_600: Page = Page { width: 6071, height: 8598, dpi: 600 };
    /// B4 at 350 dpi, for machines that cannot hold the 600 dpi page.
    pub const B4_350: Page = Page { width: 3542, height: 5016, dpi: 350 };

    pub fn tiles_wide(&self) -> i32 {
        self.width.div_ceil(TILE_SIZE as u32) as i32
    }

    pub fn tiles_high(&self) -> i32 {
        self.height.div_ceil(TILE_SIZE as u32) as i32
    }
}

/// What a synthetic layer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerType {
    LineArt,
    Flat,
    Tone,
    Gradient,
}

impl LayerType {
    pub const ALL: [LayerType; 4] = [LayerType::LineArt, LayerType::Flat, LayerType::Tone, LayerType::Gradient];

    /// Folder and layer name prefix.
    pub fn name(self) -> &'static str {
        match self {
            LayerType::LineArt => "Line art",
            LayerType::Flat => "Flat",
            LayerType::Tone => "Tone",
            LayerType::Gradient => "Gradient",
        }
    }

    /// The type of a layer named by [`synthetic_manga_page`].
    pub fn of_layer(name: &str) -> Option<LayerType> {
        Self::ALL.into_iter().find(|t| name.starts_with(t.name()) && name.len() > t.name().len())
    }

    /// Layers of this type on the reference page, and the share of the
    /// page's tiles each covers.
    pub fn layers_and_coverage(self) -> (u32, f64) {
        match self {
            LayerType::LineArt => (10, 0.35),
            LayerType::Flat => (8, 0.40),
            LayerType::Tone => (6, 0.50),
            LayerType::Gradient => (6, 0.20),
        }
    }
}

/// splitmix64: a well-mixed hash of a seed.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(mix(seed))
    }

    /// Uniform in [0, 1).
    fn f(&mut self) -> f64 {
        self.0 = mix(self.0);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f()
    }
}

/// One layer's recipe.
struct Spec {
    kind: LayerType,
    seed: u64,
    coverage: f64,
    /// Straight colour, 0..1.
    colour: [f64; 3],
}

/// The tiles a layer covers: blobs of a smooth random field above the
/// quantile that gives `coverage`. Returns `(coord, on the blob's edge)`.
fn region(page: Page, spec: &Spec) -> Vec<(TileCoord, bool)> {
    let (tw, th) = (page.tiles_wide(), page.tiles_high());
    let mut rng = Rng::new(spec.seed ^ 0xB10B);
    let waves: Vec<(f64, f64, f64, f64)> = (0..4)
        .map(|_| {
            let (period, angle) = (rng.range(8.0, 40.0), rng.range(0.0, TAU));
            (angle.cos() * TAU / period, angle.sin() * TAU / period, rng.range(0.0, TAU), rng.range(0.5, 1.0))
        })
        .collect();
    let field = |x: i32, y: i32| waves.iter().map(|&(fx, fy, ph, a)| a * (fx * x as f64 + fy * y as f64 + ph).sin()).sum::<f64>();
    let values: Vec<f64> = (0..th).flat_map(|y| (0..tw).map(move |x| (x, y))).map(|(x, y)| field(x, y)).collect();
    let mut sorted = values.clone();
    sorted.sort_by(f64::total_cmp);
    let keep = ((values.len() as f64 * spec.coverage).round() as usize).min(values.len());
    let Some(&threshold) = sorted.len().checked_sub(keep).and_then(|i| sorted.get(i)) else { return Vec::new() };
    let inside = |x: i32, y: i32| {
        // Off the page counts as inside, so the page edge is not an edge.
        !(0..tw).contains(&x) || !(0..th).contains(&y) || values[(y * tw + x) as usize] >= threshold
    };
    let mut out = Vec::with_capacity(keep);
    for y in 0..th {
        for x in 0..tw {
            if values[(y * tw + x) as usize] >= threshold {
                let edge = !(inside(x - 1, y) && inside(x + 1, y) && inside(x, y - 1) && inside(x, y + 1));
                out.push((TileCoord::new(x, y), edge));
            }
        }
    }
    out
}

/// fix15 premultiplied pixel from a straight colour and coverage.
fn px(colour: [f64; 3], alpha: f64) -> [u16; 4] {
    let a = alpha.clamp(0.0, 1.0);
    let q = |v: f64| (v * a * ONE).round() as u16;
    [q(colour[0]), q(colour[1]), q(colour[2]), (a * ONE).round() as u16]
}

/// Coverage of a pixel by a segment of half-width `hw`, anti-aliased.
fn segment(x: f64, y: f64, (ax, ay, bx, by): (f64, f64, f64, f64), hw: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy).max(1e-9)).clamp(0.0, 1.0);
    let d = ((x - ax - t * dx).powi(2) + (y - ay - t * dy).powi(2)).sqrt();
    (hw + 0.5 - d).clamp(0.0, 1.0)
}

fn make_tile(page: Page, spec: &Spec, c: TileCoord, edge: bool) -> TileRef {
    let mut rng = Rng::new(spec.seed ^ ((c.x as u64) << 32 | c.y as u32 as u64).wrapping_mul(0x2545_F491_4F6C_DD1D));
    let scale = page.dpi as f64 / 600.0;
    let size = TILE_SIZE as f64;
    let (ox, oy) = (c.x as f64 * size, c.y as f64 * size);
    // A straight anti-aliased edge cutting edge tiles, 1 inside.
    let cut = edge.then(|| {
        let angle = rng.range(0.0, TAU);
        (angle.cos(), angle.sin(), rng.range(-20.0, 20.0))
    });
    let mask = |x: f64, y: f64| match cut {
        Some((nx, ny, off)) => (0.5 - ((x - 32.0) * nx + (y - 32.0) * ny - off)).clamp(0.0, 1.0),
        None => 1.0,
    };
    let mut tile = new_tile();
    let pixels = Arc::get_mut(&mut tile).expect("new tile is unique");
    match spec.kind {
        LayerType::LineArt => {
            let border = |rng: &mut Rng| match (rng.f() * 4.0) as u32 {
                0 => (rng.range(0.0, size), 0.0),
                1 => (rng.range(0.0, size), size),
                2 => (0.0, rng.range(0.0, size)),
                _ => (size, rng.range(0.0, size)),
            };
            let strokes: Vec<((f64, f64, f64, f64), f64)> = (0..2 + (rng.f() * 4.0) as usize)
                .map(|_| {
                    let ((ax, ay), (bx, by)) = (border(&mut rng), border(&mut rng));
                    ((ax, ay, bx, by), rng.range(0.8, 2.2) * scale)
                })
                .collect();
            for (y, row) in pixels.iter_mut().enumerate() {
                for (x, p) in row.iter_mut().enumerate() {
                    let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
                    let a = strokes.iter().map(|&(s, hw)| segment(fx, fy, s, hw)).fold(0.0, f64::max);
                    *p = px([0.0; 3], a);
                }
            }
        }
        LayerType::Flat if !edge => pixels.as_flattened_mut().fill(px(spec.colour, 1.0)),
        LayerType::Flat => {
            for (y, row) in pixels.iter_mut().enumerate() {
                for (x, p) in row.iter_mut().enumerate() {
                    *p = px(spec.colour, mask(x as f64 + 0.5, y as f64 + 0.5));
                }
            }
        }
        LayerType::Tone => {
            // 60 lpi dots on a 45° screen; the spacing in pixels is not an
            // integer, so no two tiles of a layer are alike.
            let spacing = 10.0 * scale;
            let density = 0.1 + 0.4 * (spec.seed % 5) as f64 / 4.0;
            let radius = spacing * (density / PI).sqrt();
            for (y, row) in pixels.iter_mut().enumerate() {
                for (x, p) in row.iter_mut().enumerate() {
                    let (gx, gy) = (ox + x as f64 + 0.5, oy + y as f64 + 0.5);
                    let (u, v) = ((gx + gy) * FRAC_1_SQRT_2, (gx - gy) * FRAC_1_SQRT_2);
                    let (du, dv) = (u - spacing * (u / spacing).round(), v - spacing * (v / spacing).round());
                    let dot = (radius + 0.5 - (du * du + dv * dv).sqrt()).clamp(0.0, 1.0);
                    *p = px([0.0; 3], dot * mask(x as f64 + 0.5, y as f64 + 0.5));
                }
            }
        }
        LayerType::Gradient => {
            let angle = (spec.seed % 7) as f64;
            let (nx, ny) = (angle.cos(), angle.sin());
            let span = (page.width as f64).hypot(page.height as f64);
            for (y, row) in pixels.iter_mut().enumerate() {
                for (x, p) in row.iter_mut().enumerate() {
                    let (gx, gy) = (ox + x as f64 + 0.5, oy + y as f64 + 0.5);
                    let t = ((gx * nx + gy * ny) / span).rem_euclid(1.0);
                    let colour = spec.colour.map(|c| c * (0.6 + 0.4 * t));
                    *p = px(colour, (0.3 + 0.7 * t) * mask(x as f64 + 0.5, y as f64 + 0.5));
                }
            }
        }
    }
    tile
}

/// The reference manga page at `page`'s size: 4 folders (flats, gradients,
/// tones, line art, bottom to top) holding 30 raster layers. Tiles are
/// generated in parallel on the global rayon pool.
pub fn synthetic_manga_page(page: Page) -> Document {
    let order = [LayerType::Flat, LayerType::Gradient, LayerType::Tone, LayerType::LineArt];
    let mut layers = Vec::new();
    let mut root = Vec::new();
    let mut next = 1u32;
    let mut specs = Vec::new();
    for kind in order {
        let folder = LayerId(next);
        next += 1;
        root.push(folder);
        let (count, coverage) = kind.layers_and_coverage();
        let mut children = Vec::new();
        for i in 0..count {
            let id = LayerId(next);
            next += 1;
            children.push(id);
            let mut rng = Rng::new(u64::from(id.0) * 7919);
            let colour = [rng.range(0.2, 1.0), rng.range(0.2, 1.0), rng.range(0.2, 1.0)];
            specs.push((id, format!("{} {}", kind.name(), i + 1), Spec { kind, seed: u64::from(id.0), coverage, colour }));
        }
        let content = LayerContent::Folder { children, expanded: true, frame: None };
        layers.push(Layer { id: folder, props: LayerProps::named(kind.name()), content });
    }
    let mut active = LayerId(1);
    for (id, name, spec) in specs {
        let cells = region(page, &spec);
        let tiles: Vec<(TileCoord, TileRef)> =
            cells.par_iter().map(|&(c, edge)| (c, make_tile(page, &spec, c, edge))).collect();
        let mut grid = TileGrid::with_capacity(tiles.len());
        for (c, t) in tiles {
            grid.insert(c, t);
        }
        let mut props = LayerProps::named(name);
        if spec.kind == LayerType::Tone {
            props.blend = arty_core::BlendMode::Multiply;
        }
        layers.push(Layer { id, props, content: LayerContent::Raster(grid) });
        active = id;
    }
    Document::from_parts(DocParts {
        width: page.width,
        height: page.height,
        dpi: page.dpi,
        paper: Some(PAPER_WHITE),
        layers,
        root,
        active,
        next_id: next,
    })
    .expect("the synthetic tree is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_page_matches_the_recipe() {
        let page = Page { width: 640, height: 640, dpi: 300 };
        let doc = synthetic_manga_page(page);
        assert_eq!(doc.layer_count(), 34);
        assert_eq!(doc.root().len(), 4);
        let mut counts = std::collections::HashMap::new();
        for f in doc.root() {
            for &id in doc.layer(*f).unwrap().children().unwrap() {
                let l = doc.layer(id).unwrap();
                let kind = LayerType::of_layer(&l.props.name).unwrap();
                let (_, coverage) = kind.layers_and_coverage();
                assert_eq!(l.raster().unwrap().len(), (100.0 * coverage).round() as usize, "{}", l.props.name);
                *counts.entry(kind).or_insert(0) += 1;
            }
        }
        for kind in LayerType::ALL {
            assert_eq!(counts[&kind], kind.layers_and_coverage().0, "{kind:?}");
        }
        // Same page, same pixels.
        let again = synthetic_manga_page(page);
        let id = doc.active();
        let (a, b) = (doc.layer(id).unwrap().raster().unwrap(), again.layer(id).unwrap().raster().unwrap());
        assert!(a.iter().all(|(c, t)| **t == *b.get(c).unwrap()));
        // Premultiplied, in range.
        for (_, t) in a.iter() {
            assert!(t.as_flattened().iter().all(|p| p[3] <= 1 << 15 && p[..3].iter().all(|&c| c <= p[3])));
        }
    }
}
