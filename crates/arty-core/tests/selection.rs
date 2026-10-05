//! SEL-CORE (plans/m3_page_tools.md §9.1): selection model, ops, rasterizer,
//! morphology and clearing the selected area.

use std::sync::Arc;

use arty_core::morph::{self, MorphShape};
use arty_core::raster::{self, RasterOpts};
use arty_core::selection::{erase_selected, full_mask};
use arty_core::{Document, Edit, History, MaskPixels, MaskView, Pt, SelectOp, Selection, TILE_SIZE, TileCoord, TileRef};

const T: usize = TILE_SIZE;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn tiles_of(w: u32, h: u32) -> (i32, i32) {
    (w.div_ceil(64) as i32, h.div_ceil(64) as i32)
}

/// A selection mixing every tile kind: empty, full, noisy, hard-edged and
/// smooth partial tiles.
fn random_sel(rng: &mut Rng, w: u32, h: u32) -> Selection {
    let (tw, th) = tiles_of(w, h);
    let mut s = Selection::new();
    for ty in 0..th {
        for tx in 0..tw {
            let c = TileCoord::new(tx, ty);
            let mut m: MaskPixels = [[0; T]; T];
            match rng.below(6) {
                0 | 1 => continue,
                2 => {
                    s.insert_tile(c, full_mask().clone());
                    continue;
                }
                3 => m.as_flattened_mut().iter_mut().for_each(|v| *v = rng.next() as u8),
                4 => {
                    let cut = rng.below(64) as usize;
                    for (y, row) in m.iter_mut().enumerate() {
                        for (x, v) in row.iter_mut().enumerate() {
                            *v = if x + y > cut { 255 } else { 0 };
                        }
                    }
                }
                _ => {
                    for (y, row) in m.iter_mut().enumerate() {
                        for (x, v) in row.iter_mut().enumerate() {
                            *v = ((x * 4 + y) % 256) as u8;
                        }
                    }
                }
            }
            s.insert_tile(c, Arc::new(m));
        }
    }
    s
}

/// Page values, row-major.
fn dense(s: &Selection, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w * h) as usize];
    for (c, m) in s.tiles() {
        let (ox, oy) = c.origin();
        for ly in 0..T as i32 {
            for lx in 0..T as i32 {
                let (x, y) = (ox + lx, oy + ly);
                if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    out[(y as u32 * w + x as u32) as usize] = m[ly as usize][lx as usize];
                }
            }
        }
    }
    out
}

fn assert_canonical(s: &Selection, what: &str) {
    for (c, m) in s.tiles() {
        let flat = m.as_flattened();
        assert!(!flat.iter().all(|&v| v == 0), "{what}: all-0 tile stored at {c:?}");
        if flat.iter().all(|&v| v == 255) {
            assert!(Arc::ptr_eq(m, full_mask()), "{what}: all-255 tile at {c:?} is not the shared full tile");
        }
        if let Some(b) = s.bounds() {
            assert!(b.contains(c), "{what}: {c:?} outside the bounds");
        }
    }
}

fn polygon_sel(pts: &[Pt], w: u32, h: u32) -> Selection {
    raster::rasterize_polygon(pts, w, h, true)
}

/// A wobbly closed lasso around (`cx`, `cy`).
fn lasso(rng: &mut Rng, cx: f32, cy: f32, r: f32, n: usize) -> Vec<Pt> {
    let (a, b) = (rng.f() * 0.3, rng.f() * 0.2);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            let rr = r * (1.0 + a * (3.0 * t).sin() + b * (7.0 * t + 1.0).cos());
            [cx + rr * t.cos(), cy + rr * t.sin()]
        })
        .collect()
}

fn disk(cx: f32, cy: f32, r: f32) -> Vec<Pt> {
    (0..720)
        .map(|i| {
            let t = i as f32 / 720.0 * std::f32::consts::TAU;
            [cx + r * t.cos(), cy + r * t.sin()]
        })
        .collect()
}

const OPS: [SelectOp; 4] = [SelectOp::Replace, SelectOp::Add, SelectOp::Subtract, SelectOp::Intersect];

#[test]
fn sc01_canonical_form_after_every_op() {
    let (w, h) = (300, 200);
    let mut rng = Rng(11);
    for round in 0..4 {
        let a = random_sel(&mut rng, w, h);
        let b = random_sel(&mut rng, w, h);
        assert_canonical(&a, "insert");
        for op in OPS {
            let mut s = a.clone();
            s.combine(&b, op);
            assert_canonical(&s, &format!("{round}: {op:?}"));
        }
        assert_canonical(&a.inverted(w, h), "invert");
        assert_canonical(&Selection::all(w, h), "all");
        for shape in [MorphShape::Circle, MorphShape::Square] {
            assert_canonical(&morph::grow(&a, 5, shape, w, h), "grow");
            assert_canonical(&morph::shrink(&a, 5, shape, w, h), "shrink");
        }
        assert_canonical(&morph::feather(&a, 3, w, h), "feather");
        let pts = lasso(&mut rng, 150.0, 100.0, 70.0, 300);
        assert_canonical(&polygon_sel(&pts, w, h), "lasso");
        assert_canonical(&raster::rasterize_polygon(&[[10.0, 10.0], [138.0, 10.0], [138.0, 140.0], [10.0, 140.0]], w, h, true), "rect");
    }
}

#[test]
fn sc02_copy_on_write() {
    let (w, h) = (256, 256);
    let mut rng = Rng(5);
    let mut s = random_sel(&mut rng, w, h);
    let snap = s.clone();
    let before = dense(&snap, w, h);
    let ptrs: Vec<(TileCoord, *const MaskPixels)> = snap.tiles().map(|(c, m)| (c, Arc::as_ptr(m))).collect();
    let other = random_sel(&mut rng, w, h);
    s.combine(&other, SelectOp::Add);
    s.combine(&other, SelectOp::Subtract);
    s.insert_tile(TileCoord::new(0, 0), Arc::new([[7; T]; T]));
    s.remove_tile(TileCoord::new(1, 1));
    assert!(!s.shares_storage(&snap));
    assert_eq!(dense(&snap, w, h), before, "the clone kept its pixels");
    for (c, p) in ptrs {
        assert!(snap.tiles().any(|(d, m)| d == c && Arc::as_ptr(m) == p), "{c:?} was copied");
    }
}

#[test]
fn sc03_combine_matches_scalar_reference() {
    let (w, h) = (250, 190);
    let mut rng = Rng(77);
    for _ in 0..6 {
        let a = random_sel(&mut rng, w, h);
        let b = random_sel(&mut rng, w, h);
        let (da, db) = (dense(&a, w, h), dense(&b, w, h));
        for op in OPS {
            let mut s = a.clone();
            s.combine(&b, op);
            let got = dense(&s, w, h);
            for i in 0..da.len() {
                let (m, v) = (da[i], db[i]);
                let want = match op {
                    SelectOp::Replace => v,
                    SelectOp::Add => m.max(v),
                    SelectOp::Subtract => m.min(255 - v),
                    SelectOp::Intersect => m.min(v),
                };
                assert_eq!(got[i], want, "{op:?} at {i}");
            }
        }
    }
    // Intersect clears everything outside the shape, its bbox included
    // (the legacy code kept pixels outside the shape's bbox).
    let mut s = Selection::all(w, h);
    let rect = raster::rasterize_polygon(&[[70.0, 70.0], [90.0, 70.0], [90.0, 100.0], [70.0, 100.0]], w, h, true);
    s.combine(&rect, SelectOp::Intersect);
    assert_eq!(dense(&s, w, h), dense(&rect, w, h));
    assert_eq!(s.value(5, 5), 0);
    assert_eq!(s.tile_count(), rect.tile_count());
}

#[test]
fn sc04_invert_all_and_subtract_everything() {
    let (w, h) = (300, 130);
    let mut rng = Rng(3);
    let a = random_sel(&mut rng, w, h);
    let back = a.inverted(w, h).inverted(w, h);
    let tiles = |s: &Selection| {
        let mut v: Vec<_> = s.tiles().map(|(c, m)| (c, **m)).collect();
        v.sort_by_key(|(c, _)| (c.y, c.x));
        v
    };
    // On the page: past it, edge tiles hold 0 after an invert.
    let mut on_page = a.clone();
    on_page.clip_to_page(w, h);
    assert!(tiles(&back) == tiles(&on_page), "invert ∘ invert = id");
    assert!(dense(&on_page, w, h) == dense(&a, w, h));
    let inv = dense(&a.inverted(w, h), w, h);
    assert!(dense(&a, w, h).iter().zip(&inv).all(|(&m, &i)| i == 255 - m));

    let all = Selection::all(w, h);
    let (tw, th) = tiles_of(w, h);
    assert_eq!(all.tile_count(), (tw * th) as usize);
    for ty in -1..=th {
        for tx in -1..=tw {
            let page = (0..tw).contains(&tx) && (0..th).contains(&ty);
            assert_eq!(matches!(all.get(TileCoord::new(tx, ty)), MaskView::Full), page, "({tx}, {ty})");
        }
    }
    assert!(Selection::all(0, 10).is_empty());
    assert!(all.inverted(w, h).is_empty());
    assert_eq!(Selection::new().inverted(w, h).tile_count(), all.tile_count());

    let mut s = a.clone();
    s.combine(&all, SelectOp::Subtract);
    assert!(s.is_empty(), "subtracting everything deselects");
    let mut s = a;
    s.combine(&Selection::new(), SelectOp::Intersect);
    assert!(s.is_empty());
}

fn shoelace(pts: &[Pt]) -> (f64, f64) {
    let mut area = 0.0;
    let mut perim = 0.0;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        area += a[0] as f64 * b[1] as f64 - b[0] as f64 * a[1] as f64;
        perim += ((b[0] - a[0]) as f64).hypot((b[1] - a[1]) as f64);
    }
    ((area / 2.0).abs(), perim)
}

fn sum(v: &[f32]) -> f64 {
    v.iter().map(|&x| x as f64).sum()
}

const GENERAL: RasterOpts = RasterOpts { rect_fast_path: false, band_tiles: 1, parallel: true };

#[test]
fn sc05_rasterizer() {
    let (w, h) = (400, 300);
    // Axis-aligned rect at fractional corners.
    let r = [[10.3f32, 20.7], [210.9, 20.7], [210.9, 150.2], [10.3, 150.2]];
    let area = shoelace(&r).0;
    let cov = raster::coverage(&r, w, h, RasterOpts::default());
    assert!((sum(&cov) - area).abs() < 1e-3, "rect {} vs {area}", sum(&cov));
    let general = sum(&raster::coverage(&r, w, h, GENERAL));
    assert!((general - area).abs() < 1e-3 * shoelace(&r).1, "general path {general} vs {area}");
    // Exactness of the box path on a small rect.
    let small = [[3.25f32, 4.5], [7.75, 4.5], [7.75, 9.0], [3.25, 9.0]];
    assert!((sum(&raster::coverage(&small, 32, 32, RasterOpts::default())) - 4.5 * 4.5).abs() < 1e-3);

    // A polygon's coverage sum is its area.
    let mut rng = Rng(9);
    for i in 0..6 {
        let pts = lasso(&mut rng, 200.0 + i as f32, 150.0, 90.0, 40 + 60 * i);
        let (area, perim) = shoelace(&pts);
        let got = sum(&raster::coverage(&pts, w, h, GENERAL));
        assert!((got - area).abs() < 1e-3 * perim, "polygon {i}: {got} vs {area}");
    }

    // Non-zero winding: the centre of a pentagram (wound twice) is selected.
    let star: Vec<Pt> = (0..5)
        .map(|i| {
            let t = (i * 2) as f32 / 5.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            [200.0 + 120.0 * t.cos(), 150.0 + 120.0 * t.sin()]
        })
        .collect();
    let s = polygon_sel(&star, w, h);
    assert_eq!(s.value(200, 150), 255, "winding 2 is inside");
    assert_eq!(s.value(200, 40), 255, "a point of the star");
    assert_eq!(s.value(110, 60), 0, "between the points");
    // The same loop traced twice the other way round.
    let mut twice: Vec<Pt> = disk(200.0, 150.0, 60.0).into_iter().rev().collect();
    twice.extend(disk(200.0, 150.0, 60.0).into_iter().rev());
    assert_eq!(polygon_sel(&twice, w, h).value(200, 150), 255);

    // The rect fast path equals the general path.
    for r in [r, [[-20.5, -3.25], [130.0, -3.25], [130.0, 64.0], [-20.5, 64.0]], [[64.0, 0.0], [128.0, 0.0], [128.0, 256.0], [64.0, 256.0]]] {
        let fast = dense(&raster::rasterize_polygon(&r, w, h, true), w, h);
        let slow = dense(&raster::rasterize_with(&r, w, h, true, GENERAL), w, h);
        let worst = fast.iter().zip(&slow).map(|(&a, &b)| (a as i32 - b as i32).abs()).max().unwrap();
        assert!(worst <= 1, "rect {r:?}: fast and general differ by {worst}");
        let a = raster::coverage(&r, w, h, RasterOpts::default());
        let b = raster::coverage(&r, w, h, GENERAL);
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3));
    }

    // Band-parallel output equals one band on one thread, bit for bit.
    let (bw, bh) = (700, 520);
    let one = RasterOpts { rect_fast_path: false, band_tiles: 0, parallel: false };
    for seed in 1..5 {
        let mut rng = Rng(seed);
        let pts = lasso(&mut rng, 350.0, 260.0, 300.0, 2000);
        let a = raster::rasterize_with(&pts, bw, bh, true, GENERAL);
        let b = raster::rasterize_with(&pts, bw, bh, true, one);
        let c = raster::rasterize_with(&pts, bw, bh, true, RasterOpts { band_tiles: 3, ..GENERAL });
        assert_eq!(dense(&a, bw, bh), dense(&b, bw, bh), "seed {seed}");
        assert_eq!(dense(&a, bw, bh), dense(&c, bw, bh), "seed {seed}, 3-tile bands");
        assert_eq!(a.tile_count(), b.tile_count());
        // Interior tiles of a big lasso are full without per-pixel work.
        assert!(matches!(a.get(TileCoord::new(5, 4)), MaskView::Full));
    }

    // Without antialiasing only 0 and 255 appear.
    let mut rng = Rng(21);
    let pts = lasso(&mut rng, 200.0, 150.0, 100.0, 500);
    let hard = raster::rasterize_polygon(&pts, w, h, false);
    assert!(dense(&hard, w, h).iter().all(|&v| v == 0 || v == 255));
    assert!(dense(&polygon_sel(&pts, w, h), w, h).iter().any(|&v| v != 0 && v != 255));
    assert!(raster::rasterize_polygon(&r, w, h, false).tiles().all(|(_, m)| m.as_flattened().iter().all(|&v| v == 0 || v == 255)));

    // Off-page parts are clipped; a shape covering the page selects it all.
    let page = raster::rasterize_polygon(&[[-50.0, -50.0], [900.0, -40.0], [880.0, 700.0], [-60.0, 650.0]], w, h, true);
    assert!(dense(&page, w, h).iter().all(|&v| v == 255));
}

/// Brute-force max (grow) or min (shrink) over a (2r+1)² square, off-page
/// = 0: a direct scan of each row window, then of each column window.
fn brute_square(d: &[u8], w: u32, h: u32, r: i32, grow: bool) -> Vec<u8> {
    let (w, h) = (w as i32, h as i32);
    let pick = |a: u8, b: u8| if grow { a.max(b) } else { a.min(b) };
    let init = if grow { 0u8 } else { 255u8 };
    let at = |v: &[u8], x: i32, y: i32| if x < 0 || y < 0 || x >= w || y >= h { 0 } else { v[(y * w + x) as usize] };
    let mut rows = vec![0u8; d.len()];
    for y in 0..h {
        for x in 0..w {
            rows[(y * w + x) as usize] = (x - r..=x + r).fold(init, |v, xx| pick(v, at(d, xx, y)));
        }
    }
    let mut out = vec![0u8; d.len()];
    for y in 0..h {
        for x in 0..w {
            out[(y * w + x) as usize] = (y - r..=y + r).fold(init, |v, yy| pick(v, at(&rows, x, yy)));
        }
    }
    out
}

#[test]
fn sc06_morphology() {
    let (w, h) = (330, 270);
    // grow(r) then shrink(r) of a disk gives the disk back within 1 px.
    let (cx, cy, rad) = (160.0f32, 140.0f32, 60.0f32);
    let d0 = polygon_sel(&disk(cx, cy, rad), w, h);
    for r in [3u16, 20, 70] {
        let g = morph::grow(&d0, r, MorphShape::Circle, w, h);
        let back = morph::shrink(&g, r, MorphShape::Circle, w, h);
        let grown = dense(&g, w, h);
        let got = dense(&back, w, h);
        for y in 0..h {
            for x in 0..w {
                let dist = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let (v, gv) = (got[(y * w + x) as usize], grown[(y * w + x) as usize]);
                // The half-coverage edge lies within 1 px of the target
                // circle, and the result is solid 2 px either side of it.
                for (target, v, what) in [(rad, v, "grow + shrink"), (rad + r as f32, gv, "grow")] {
                    let off = dist - target;
                    assert!(off > -1.0 || v >= 128, "r {r} {what}: ({x}, {y}) = {v} inside");
                    assert!(off < 1.0 || v < 128, "r {r} {what}: ({x}, {y}) = {v} outside");
                    assert!(off > -2.0 || v == 255, "r {r} {what}: ({x}, {y}) = {v} well inside");
                    assert!(off < 2.0 || v == 0, "r {r} {what}: ({x}, {y}) = {v} well outside");
                }
            }
        }
    }

    // Square grow / shrink equal brute-force max / min filters.
    let mut rng = Rng(4);
    let (sw, sh) = (200, 150);
    let mut s = random_sel(&mut rng, sw, sh);
    // Leave room for empty tiles so grow has something to do.
    s.remove_tile(TileCoord::new(0, 0));
    s.remove_tile(TileCoord::new(2, 1));
    let d = dense(&s, sw, sh);
    for r in [1u16, 3, 70] {
        let g = dense(&morph::grow(&s, r, MorphShape::Square, sw, sh), sw, sh);
        assert!(g == brute_square(&d, sw, sh, r as i32, true), "square grow r {r}");
        let k = dense(&morph::shrink(&s, r, MorphShape::Square, sw, sh), sw, sh);
        assert!(k == brute_square(&d, sw, sh, r as i32, false), "square shrink r {r}");
    }
    // A huge radius is clamped (to the page diagonal here): it finishes,
    // and growing past the diagonal selects the whole page.
    let all = morph::grow(&s, u16::MAX, MorphShape::Circle, sw, sh);
    assert!(dense(&all, sw, sh).iter().all(|&v| v == 255));
    assert!(morph::shrink(&s, u16::MAX, MorphShape::Square, sw, sh).is_empty());

    // Feather keeps the mass inside the page (shape away from the border).
    let mut rng = Rng(8);
    let blob = polygon_sel(&lasso(&mut rng, 165.0, 135.0, 60.0, 400), w, h);
    let mass = |s: &Selection| dense(s, w, h).iter().map(|&v| v as f64).sum::<f64>();
    for sigma in [2u16, 8, 16] {
        let f = morph::feather(&blob, sigma, w, h);
        let (a, b) = (mass(&blob), mass(&f));
        assert!((a - b).abs() / a < 0.005, "σ {sigma}: mass {a} → {b}");
        assert!(dense(&f, w, h).iter().any(|&v| v != 0 && v != 255));
    }

    // Tiles outside the ROI are shared, not copied.
    let (bw, bh) = (1024u32, 1024u32);
    let mut s = Selection::new();
    for ty in 0..6 {
        for tx in 0..6 {
            s.insert_tile(TileCoord::new(tx, ty), full_mask().clone());
        }
    }
    let faint = Arc::new([[100u8; T]; T]);
    s.insert_tile(TileCoord::new(14, 14), faint.clone());
    // A uniform partial block: blurring it changes only its rim.
    for ty in 9..15 {
        for tx in 0..6 {
            s.insert_tile(TileCoord::new(tx, ty), Arc::new([[100u8; T]; T]));
        }
    }
    let g = morph::grow(&s, 8, MorphShape::Circle, bw, bh);
    let same = |a: &Selection, b: &Selection, c: TileCoord| {
        let p = |s: &Selection| s.tiles().find(|(d, _)| *d == c).map(|(_, m)| Arc::as_ptr(m));
        p(a).is_some() && p(a) == p(b)
    };
    assert!(same(&g, &s, TileCoord::new(14, 14)), "a faint tile far away has nothing to grow from");
    assert!(same(&g, &s, TileCoord::new(2, 11)), "uniform 100 stays below the threshold");
    assert!(matches!(g.get(TileCoord::new(2, 2)), MaskView::Full));
    assert!(matches!(g.get(TileCoord::new(6, 2)), MaskView::Partial(_)), "grew into the next tile");
    assert!(matches!(g.get(TileCoord::new(8, 2)), MaskView::Empty));
    let f = morph::feather(&s, 4, bw, bh);
    assert!(same(&f, &s, TileCoord::new(2, 11)), "inside the uniform block");
    assert!(matches!(f.get(TileCoord::new(2, 2)), MaskView::Full));
    assert!(matches!(f.get(TileCoord::new(0, 0)), MaskView::Partial(_)), "the page border feathers too");
    assert!(matches!(f.get(TileCoord::new(12, 2)), MaskView::Empty));

    // Off the page counts as unselected: shrinking select-all insets it.
    let k = morph::shrink(&Selection::all(w, h), 4, MorphShape::Circle, w, h);
    assert_eq!((k.value(1, 1), k.value(w as i32 - 2, 100), k.value(100, 100)), (0, 0, 255));
    for f in [morph::grow, morph::shrink] {
        assert!(f(&s, 0, MorphShape::Circle, bw, bh).shares_storage(&s), "r = 0 is a no-op");
    }
    assert!(morph::feather(&s, 0, bw, bh).shares_storage(&s));
}

fn tile_at(doc: &Document, c: TileCoord) -> Option<TileRef> {
    doc.active_layer().raster().unwrap().get_ref(c).cloned()
}

#[test]
fn sc07_erase_selected() {
    let mut doc = Document::new(256, 384, 350);
    let id = doc.active();
    let mut rng = Rng(31);
    {
        let (grid, _) = doc.paint_target(id).unwrap();
        for ty in 0..3 {
            for tx in 0..4 {
                let px = grid.get_mut_or_create(TileCoord::new(tx, ty));
                for p in px.as_flattened_mut() {
                    let a = (rng.below(1 << 15) + 1) as u16;
                    *p = [(rng.below(a as u64 + 1)) as u16, (rng.below(a as u64 + 1)) as u16, a / 3, a];
                }
            }
        }
    }
    let half = TileCoord::new(1, 1);
    let full = TileCoord::new(2, 0);
    let untouched = TileCoord::new(3, 2);
    let mut sel = Selection::new();
    let mut m: MaskPixels = [[0; T]; T];
    for (y, row) in m.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = ((x * 7 + y * 3) % 256) as u8;
        }
    }
    sel.insert_tile(half, Arc::new(m));
    sel.insert_tile(full, full_mask().clone());
    // Selected but nothing painted there.
    sel.insert_tile(TileCoord::new(3, 5), full_mask().clone());
    doc.swap_selection(sel);

    let before: Vec<_> = (0..3).flat_map(|ty| (0..4).map(move |tx| TileCoord::new(tx, ty))).map(|c| (c, tile_at(&doc, c))).collect();
    let old_half = tile_at(&doc, half).unwrap();
    let rev = doc.revision();
    let edit = erase_selected(&mut doc, id).expect("pixels were cleared");
    assert_ne!(doc.revision(), rev);
    let Edit::Pixels { layer, tiles } = &edit else { panic!("one Edit::Pixels") };
    assert_eq!(*layer, id);
    let mut changed: Vec<_> = tiles.iter().map(|(c, _)| *c).collect();
    changed.sort();
    assert_eq!(changed, vec![half, full]);

    let new_half = tile_at(&doc, half).unwrap();
    for y in 0..T {
        for x in 0..T {
            let k = 255 - m[y][x] as u32;
            let want = old_half[y][x].map(|v| ((v as u32 * k + 127) / 255) as u16);
            assert_eq!(new_half[y][x], want, "({x}, {y})");
            let p = new_half[y][x];
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "c ≤ a at ({x}, {y})");
        }
    }
    assert!(tile_at(&doc, full).is_none(), "a fully selected tile is removed");
    for (c, t) in &before {
        if *c != half && *c != full {
            assert!(Arc::ptr_eq(t.as_ref().unwrap(), &tile_at(&doc, *c).unwrap()), "{c:?} copied");
        }
    }
    assert!(tile_at(&doc, untouched).is_some());

    let mut history = History::default();
    history.push(edit);
    history.undo(&mut doc);
    for (c, t) in &before {
        assert!(Arc::ptr_eq(t.as_ref().unwrap(), &tile_at(&doc, *c).unwrap()), "undo restores {c:?}");
    }

    let mut p = doc.layer(id).unwrap().props.clone();
    p.locked = true;
    doc.set_props(id, p);
    assert!(erase_selected(&mut doc, id).is_none(), "locked");
    let mut p = doc.layer(id).unwrap().props.clone();
    p.locked = false;
    doc.set_props(id, p);
    // Selected only where nothing is painted: no step.
    let mut far = Selection::new();
    far.insert_tile(TileCoord::new(3, 5), full_mask().clone());
    doc.swap_selection(far);
    assert!(erase_selected(&mut doc, id).is_none());
    doc.swap_selection(Selection::new());
    assert!(erase_selected(&mut doc, id).is_none(), "no selection");
}

/// Edge tiles decide on their page pixels: subtracting or inverting
/// everything leaves no selection, not tiles selected only past the page.
#[test]
fn sc08_edge_tiles_are_canonical_on_the_page() {
    let (w, h) = (300, 130);
    let beyond: [Pt; 4] = [[-10.0, -10.0], [400.0, -10.0], [400.0, 200.0], [-10.0, 200.0]];
    let page = raster::rasterize_polygon(&beyond, w, h, true);
    assert!(page.inverted(w, h).is_empty(), "inverting a page-covering shape selects nothing");

    let mut s = Selection::all(w, h);
    s.combine(&page, SelectOp::Subtract);
    s.clip_to_page(w, h);
    assert!(s.is_empty(), "subtracting the page deselects");

    // A page-covering shape becomes `Selection::all` after the clip, so
    // later ops take the full-tile paths.
    let mut p = page.clone();
    p.clip_to_page(w, h);
    assert!(p.tiles().all(|(_, m)| Arc::ptr_eq(m, full_mask())));
    assert_eq!(p.tile_count(), 5 * 3);

    // Two subtractions that together cover an edge tile's page pixels.
    let mut s = Selection::all(w, h);
    for x in [[250.0, 280.0], [280.0, 400.0]] {
        let r: [Pt; 4] = [[x[0], -10.0], [x[1], -10.0], [x[1], 200.0], [x[0], 200.0]];
        s.combine(&raster::rasterize_polygon(&r, w, h, false), SelectOp::Subtract);
        s.clip_to_page(w, h);
    }
    for ty in 0..3 {
        assert!(matches!(s.get(TileCoord::new(4, ty)), MaskView::Empty), "column 4, row {ty}");
    }
    assert!(matches!(s.get(TileCoord::new(3, 0)), MaskView::Partial(_)));

    // A partial edge tile keeps its page pixels and loses the rest.
    let mut m: MaskPixels = [[255; T]; T];
    m[0][0] = 7;
    let mut s = Selection::new();
    s.insert_tile(TileCoord::new(4, 1), Arc::new(m));
    s.clip_to_page(w, h);
    let MaskView::Partial(m) = s.get(TileCoord::new(4, 1)) else { panic!("kept as partial") };
    assert_eq!((m[0][0], m[1][300 - 256 - 1], m[1][300 - 256]), (7, 255, 0));
    let mut m: MaskPixels = [[255; T]; T];
    m[0][0] = 7;
    let mut s = Selection::new();
    s.insert_tile(TileCoord::new(2, 2), Arc::new(m));
    s.clip_to_page(w, h);
    let MaskView::Partial(m) = s.get(TileCoord::new(2, 2)) else { panic!("kept as partial") };
    assert_eq!((m[0][0], m[1][63], m[130 - 128][0]), (7, 255, 0));
    // Interior tiles are left alone.
    let mut m: MaskPixels = [[0; T]; T];
    m[63][63] = 9;
    let mut s = Selection::new();
    s.insert_tile(TileCoord::new(1, 0), Arc::new(m));
    s.clip_to_page(w, h);
    assert_eq!(s.value(127, 63), 9);
}
