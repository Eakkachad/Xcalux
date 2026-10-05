//! Bucket fill: regions, gap closing, area scaling and writing (spec
//! §4.3, FILL tests 1–13).

use std::sync::Arc;

use arty_core::fill::{FillBlend, FillParams, FillRef, FillScratch, ScaleMode, apply_fill, fill_region, fill_selection};
use arty_core::fix15::ONE_U16;
use arty_core::selection::full_mask;
use arty_core::tile::{TILE_SIZE, new_tile_box};
use arty_core::{CompositeScratch, Document, History, LayerId, Selection, TileCoord};

const O: u16 = ONE_U16;
const INK: [u16; 4] = [0, 0, 0, O];
const RED: [u16; 4] = [O, 0, 0, O];

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
}

fn set(doc: &mut Document, id: LayerId, x: i32, y: i32, v: [u16; 4]) {
    let c = TileCoord::from_pixel(x, y);
    let (ox, oy) = c.origin();
    doc.paint_target(id).unwrap().0.get_mut_or_create(c)[(y - oy) as usize][(x - ox) as usize] = v;
}

fn get(doc: &Document, id: LayerId, x: i32, y: i32) -> [u16; 4] {
    let c = TileCoord::from_pixel(x, y);
    let (ox, oy) = c.origin();
    doc.layer(id).unwrap().raster().unwrap().get(c).map_or([0; 4], |t| t[(y - oy) as usize][(x - ox) as usize])
}

fn rect(doc: &mut Document, id: LayerId, x0: i32, y0: i32, x1: i32, y1: i32, v: [u16; 4]) {
    for y in y0..y1 {
        for x in x0..x1 {
            set(doc, id, x, y, v);
        }
    }
}

/// A `t` px thick square outline over `[x0, x1)×[y0, y1)`.
fn ring(doc: &mut Document, id: LayerId, x0: i32, y0: i32, x1: i32, y1: i32, t: i32) {
    rect(doc, id, x0, y0, x1, y0 + t, INK);
    rect(doc, id, x0, y1 - t, x1, y1, INK);
    rect(doc, id, x0, y0, x0 + t, y1, INK);
    rect(doc, id, x1 - t, y0, x1, y1, INK);
}

fn exact() -> FillParams {
    FillParams { antialias: false, ..FillParams::default() }
}

fn region(doc: &Document, seed: (i32, i32), p: &FillParams) -> Option<Selection> {
    fill_region(doc, seed, p, &mut FillScratch::default())
}

fn pixels(doc: &Document) -> impl Iterator<Item = (i32, i32)> {
    let (w, h) = (doc.width() as i32, doc.height() as i32);
    (0..h).flat_map(move |y| (0..w).map(move |x| (x, y)))
}

/// Reference flood: pixel BFS, one cached tile.
fn naive_flood(doc: &Document, id: LayerId, seed: (i32, i32), tol: u16) -> Vec<bool> {
    let (w, h) = (doc.width() as i32, doc.height() as i32);
    let grid = doc.layer(id).unwrap().raster().unwrap();
    let mut cache: Option<(TileCoord, Option<Box<arty_core::TilePixels>>)> = None;
    let mut px = |x: i32, y: i32| {
        let c = TileCoord::from_pixel(x, y);
        if cache.as_ref().is_none_or(|(cc, _)| *cc != c) {
            cache = Some((c, grid.get(c).map(|t| Box::new(*t))));
        }
        let (ox, oy) = c.origin();
        cache.as_ref().unwrap().1.as_ref().map_or([0; 4], |t| t[(y - oy) as usize][(x - ox) as usize])
    };
    let s = px(seed.0, seed.1);
    let mut passes = |x: i32, y: i32| {
        let p = px(x, y);
        (0..4).all(|c| p[c].abs_diff(s[c]) <= tol)
    };
    let mut out = vec![false; (w * h) as usize];
    let mut stack = vec![seed];
    out[(seed.1 * w + seed.0) as usize] = true;
    while let Some((x, y)) = stack.pop() {
        for (qx, qy) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
            if qx < 0 || qy < 0 || qx >= w || qy >= h || out[(qy * w + qx) as usize] {
                continue;
            }
            if passes(qx, qy) {
                out[(qy * w + qx) as usize] = true;
                stack.push((qx, qy));
            }
        }
    }
    out
}

/// Random line art: thick segments and solid blobs, some semi-transparent.
fn line_art(doc: &mut Document, id: LayerId, rng: &mut Rng, segments: usize) {
    let (w, h) = (doc.width() as i64, doc.height() as i64);
    for _ in 0..segments {
        let (x0, y0) = (rng.below(w as u64) as i64, rng.below(h as u64) as i64);
        let (x1, y1) = (rng.below(w as u64) as i64, rng.below(h as u64) as i64);
        let t = 1 + rng.below(3) as i64;
        let a = [O, O, O, O / 2, O / 7][rng.below(5) as usize];
        let v = [a / 4, a / 3, 0, a];
        let n = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for k in 0..=n {
            let (cx, cy) = (x0 + (x1 - x0) * k / n, y0 + (y1 - y0) * k / n);
            for dy in 0..t {
                for dx in 0..t {
                    let (x, y) = (cx + dx, cy + dy);
                    if x < w && y < h {
                        set(doc, id, x as i32, y as i32, v);
                    }
                }
            }
        }
    }
}

#[test]
fn fl01_closed_ring_fills_its_interior_only() {
    let mut doc = Document::new(256, 256, 600);
    let id = doc.active();
    ring(&mut doc, id, 50, 50, 150, 150, 3);
    for p in [exact(), FillParams::default()] {
        let r = region(&doc, (100, 100), &p).unwrap();
        for (x, y) in pixels(&doc) {
            let inside = (53..147).contains(&x) && (53..147).contains(&y);
            assert_eq!(r.value(x, y), if inside { 255 } else { 0 }, "({x}, {y}) aa {}", p.antialias);
        }
    }
}

#[test]
fn fl02_gap_closing_stops_the_leak_at_the_mouth() {
    let mut doc = Document::new(512, 512, 600);
    let id = doc.active();
    ring(&mut doc, id, 150, 150, 350, 350, 4);
    // A 6 px gap in the right wall (x 346..350).
    rect(&mut doc, id, 346, 247, 350, 253, [0; 4]);
    let leak = region(&doc, (250, 250), &exact()).unwrap();
    assert_eq!(leak.value(10, 10), 255, "R = 0 leaks");

    let closed = region(&doc, (250, 250), &FillParams { gap_px: 4, ..exact() }).unwrap();
    assert_eq!(closed.value(10, 10), 0, "R = 4 holds");
    for (x, y) in pixels(&doc) {
        if closed.value(x, y) > 0 {
            assert!(x <= 350 && (150..350).contains(&y), "({x}, {y}) is past the gap mouth");
        }
    }
    // The interior is filled all the way to the walls.
    for y in 154..346 {
        for x in 154..346 {
            assert_eq!(closed.value(x, y), 255, "({x}, {y})");
        }
    }
}

/// A click in the ring of a strand too narrow for a core of its own does
/// not adopt the big region's core across the line: the nearest core is
/// looked for along the region, not in a straight line.
#[test]
fn fl02_gap_fallback_does_not_cross_a_line() {
    let mut doc = Document::new(256, 256, 600);
    let id = doc.active();
    // A 12 px strand (x 103..115, y 20..236) between 3 px lines, open to
    // the background through a 6 px gap in its left line far down.
    rect(&mut doc, id, 100, 17, 103, 239, INK);
    rect(&mut doc, id, 115, 17, 118, 239, INK);
    rect(&mut doc, id, 100, 17, 118, 20, INK);
    rect(&mut doc, id, 100, 236, 118, 239, INK);
    rect(&mut doc, id, 100, 200, 103, 206, [0; 4]);
    let p = FillParams { gap_px: 8, ..exact() };
    // 1 px from the line, the background core (x ≤ 91) is 13 px away in a
    // straight line, within 2R = 16; along the region it is ~140 px.
    for seed in [(104, 60), (109, 60), (113, 60)] {
        let r = region(&doc, seed, &p).unwrap();
        assert_eq!(r.value(30, 30), 0, "seed {seed:?}: the background was filled");
        assert_eq!(r.value(200, 100), 0, "seed {seed:?}");
        assert_eq!(r.value(109, 30), 255, "seed {seed:?}: the strand was not filled");
        assert_eq!(r.value(seed.0, seed.1), 255);
    }
    // A seed in the background's ring still finds the background core.
    let r = region(&doc, (97, 60), &p).unwrap();
    assert_eq!(r.value(30, 30), 255);
}

#[test]
fn fl03_gap_closing_reaches_acute_corners() {
    let mut doc = Document::new(512, 256, 600);
    let id = doc.active();
    // A closed triangle with a ~16° tip at (460, 100).
    let tri = [(40.0f64, 40.0f64), (460.0, 100.0), (40.0, 160.0)];
    let dist = |px: f64, py: f64, (ax, ay): (f64, f64), (bx, by): (f64, f64)| {
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
    };
    for (x, y) in pixels(&doc).collect::<Vec<_>>() {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        if (0..3).any(|k| dist(px, py, tri[k], tri[(k + 1) % 3]) <= 1.2) {
            set(&mut doc, id, x, y, INK);
        }
    }
    let plain = region(&doc, (100, 100), &exact()).unwrap();
    assert_eq!(plain.value(5, 5), 0, "closed");
    assert!(plain.value(440, 100) == 255, "the tip is open space");
    for r in [4, 8] {
        let closed = region(&doc, (100, 100), &FillParams { gap_px: r, ..exact() }).unwrap();
        for (x, y) in pixels(&doc) {
            assert_eq!(closed.value(x, y), plain.value(x, y), "R {r} at ({x}, {y})");
        }
    }
}

#[test]
fn fl04_area_scaling() {
    let mut doc = Document::new(256, 128, 600);
    let id = doc.active();
    // A vertical 3 px line with antialiased edges.
    let profile = [(99, 11469u16), (100, O), (101, O), (102, O - 300), (103, 11469)];
    for y in 0..128 {
        for (x, a) in profile {
            set(&mut doc, id, x, y, [0, 0, 0, a]);
        }
    }
    let span = |r: &Selection, y: i32| {
        let xs: Vec<i32> = (0..256).filter(|&x| r.value(x, y) > 0).collect();
        (*xs.first().unwrap(), *xs.last().unwrap())
    };
    let base = region(&doc, (20, 64), &exact()).unwrap();
    assert_eq!(span(&base, 64), (0, 98));

    let dark = FillParams { area_scale: 10, scale_mode: ScaleMode::ToDarkest, ..exact() };
    let r = region(&doc, (20, 64), &dark).unwrap();
    for y in 0..128 {
        assert_eq!(span(&r, y), (0, 102), "row {y}: reaches the core, never the far side");
    }
    let plain = FillParams { area_scale: 10, ..exact() };
    let r = region(&doc, (20, 64), &plain).unwrap();
    assert_eq!(span(&r, 64), (0, 103), "plain growth crosses the whole line");
    let small = FillParams { area_scale: 2, ..exact() };
    assert_eq!(span(&region(&doc, (20, 64), &small).unwrap(), 64), (0, 100));

    for n in [1, 3, 7] {
        let r = region(&doc, (20, 64), &FillParams { area_scale: -n, ..exact() }).unwrap();
        for y in 0..128 {
            assert_eq!(span(&r, y), (0, 98 - n as i32), "row {y}: erodes by exactly {n}, not at the page border");
        }
    }
}

#[test]
fn fl05_writes_keep_premultiplied_invariants() {
    let mut rng = Rng(0xF111);
    for (blend, lock_alpha, opacity) in [
        (FillBlend::Normal, false, 1.0),
        (FillBlend::Normal, false, 0.37),
        (FillBlend::Behind, false, 1.0),
        (FillBlend::Behind, false, 0.61),
        (FillBlend::Normal, true, 1.0),
        (FillBlend::Normal, true, 0.5),
    ] {
        let mut doc = Document::new(192, 160, 600);
        let id = doc.active();
        for (x, y) in pixels(&doc).collect::<Vec<_>>() {
            if rng.below(4) != 0 {
                let a = rng.below(O as u64 + 1) as u16;
                let ch = |r: &mut Rng| (r.below(a as u64 + 1)) as u16;
                let v = [ch(&mut rng), ch(&mut rng), ch(&mut rng), a];
                set(&mut doc, id, x, y, v);
            }
        }
        let mut p = doc.layer(id).unwrap().props.clone();
        p.lock_alpha = lock_alpha;
        doc.set_props(id, p);
        let mut sel = Selection::new();
        for ty in 0..3 {
            for tx in 0..3 {
                let mut m = [[0u8; TILE_SIZE]; TILE_SIZE];
                for row in m.iter_mut() {
                    for v in row.iter_mut() {
                        *v = rng.below(256) as u8;
                    }
                }
                sel.insert_tile(TileCoord::new(tx, ty), Arc::new(m));
            }
        }
        sel.insert_tile(TileCoord::new(1, 1), full_mask().clone());
        for color in [RED, [9000, 12000, 3000, 20000], [O, O, O, O], [100, 0, 50, 300]] {
            let before = doc.snapshot();
            let edit = apply_fill(&mut doc, id, &sel, color, opacity, blend);
            assert!(edit.is_some());
            for (x, y) in pixels(&doc) {
                let v = get(&doc, id, x, y);
                assert!(v[3] <= O && v[0] <= v[3] && v[1] <= v[3] && v[2] <= v[3], "{v:?} {blend:?} lock {lock_alpha}");
                if lock_alpha {
                    assert_eq!(v[3], get(&before, id, x, y)[3], "alpha locked");
                }
            }
        }
    }
}

#[test]
fn fl06_selection_limits_the_fill() {
    let mut doc = Document::new(256, 256, 600);
    let mut sel = Selection::new();
    let mut m = [[0u8; TILE_SIZE]; TILE_SIZE];
    for (y, row) in m.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = ((x * 4 + y) % 256) as u8;
        }
    }
    sel.insert_tile(TileCoord::new(0, 0), Arc::new(m));
    sel.insert_tile(TileCoord::new(1, 0), full_mask().clone());
    doc.swap_selection(sel.clone());
    let r = region(&doc, (70, 10), &FillParams::default()).unwrap();
    for (x, y) in pixels(&doc) {
        assert_eq!(r.value(x, y), sel.value(x, y), "({x}, {y})");
    }
    assert!(region(&doc, (10, 100), &FillParams::default()).is_none(), "seed outside the selection");
    assert!(region(&doc, (0, 0), &FillParams::default()).is_none(), "seed at coverage 0");
    let wand = FillParams { use_selection: false, ..FillParams::default() };
    assert_eq!(region(&doc, (10, 100), &wand).unwrap().value(200, 200), 255, "the wand ignores it");
}

#[test]
fn fl07_non_contiguous_takes_every_passing_pixel() {
    let mut doc = Document::new(300, 200, 600);
    let id = doc.active();
    line_art(&mut doc, id, &mut Rng(77), 40);
    for (seed, tol) in [((5, 5), 0u16), ((150, 100), 2000), ((299, 199), 20000)] {
        let p = FillParams { contiguous: false, tolerance: tol, ..exact() };
        let s = get(&doc, id, seed.0, seed.1);
        let r = region(&doc, seed, &p).unwrap();
        for (x, y) in pixels(&doc) {
            let v = get(&doc, id, x, y);
            let pass = (0..4).all(|c| v[c].abs_diff(s[c]) <= tol);
            assert_eq!(r.value(x, y) > 0, pass, "({x}, {y})");
        }
    }
}

#[test]
fn fl08_wavefront_matches_a_naive_flood() {
    let mut rng = Rng(0xC0FFEE);
    for round in 0..3 {
        let (w, h) = ([700, 333, 450][round], [500, 640, 129][round]);
        let mut doc = Document::new(w, h, 600);
        let id = doc.active();
        line_art(&mut doc, id, &mut rng, 30 + round * 30);
        let mut scratch = FillScratch::default();
        for _ in 0..8 {
            let seed = (rng.below(w as u64) as i32, rng.below(h as u64) as i32);
            let tol = [0u16, 0, 5000, 17000][rng.below(4) as usize];
            let p = FillParams { tolerance: tol, ..exact() };
            let r = fill_region(&doc, seed, &p, &mut scratch).unwrap();
            let want = naive_flood(&doc, id, seed, tol);
            for (x, y) in pixels(&doc) {
                assert_eq!(r.value(x, y) > 0, want[(y * w as i32 + x) as usize], "seed {seed:?} at ({x}, {y})");
                assert!(matches!(r.value(x, y), 0 | 255));
            }
        }
    }
}

#[test]
fn fl09_all_visible_matches_the_flattened_page() {
    let mut doc = Document::new(320, 200, 600);
    let a = doc.active();
    line_art(&mut doc, a, &mut Rng(3), 20);
    let b = doc.add_raster_layer().unwrap();
    rect(&mut doc, b, 60, 40, 200, 120, [O / 3, O / 4, O / 5, O / 2]);
    let c = doc.add_raster_layer().unwrap();
    line_art(&mut doc, c, &mut Rng(9), 15);
    let mut p = doc.layer(c).unwrap().props.clone();
    p.blend = arty_core::BlendMode::Multiply;
    doc.set_props(c, p);

    let mut flat = Document::new(320, 200, 600);
    flat.set_paper(None);
    let f = flat.active();
    let mut cs = CompositeScratch::new();
    let mut tile = new_tile_box();
    for ty in 0..doc.tiles_high() as i32 {
        for tx in 0..doc.tiles_wide() as i32 {
            let t = TileCoord::new(tx, ty);
            doc.composite_tile(t, &mut tile, &mut cs);
            *flat.paint_target(f).unwrap().0.get_mut_or_create(t) = *tile;
        }
    }
    for (seed, tol) in [((5, 5), 0u16), ((100, 80), 3000), ((250, 150), 12000)] {
        let all = region(&doc, seed, &FillParams { reference: FillRef::AllVisible, tolerance: tol, ..exact() }).unwrap();
        let want = region(&flat, seed, &FillParams { tolerance: tol, ..exact() }).unwrap();
        for (x, y) in pixels(&doc) {
            assert_eq!(all.value(x, y), want.value(x, y), "seed {seed:?} at ({x}, {y})");
        }
    }
}

#[test]
fn fl10_reference_mode_uses_only_reference_layers() {
    let mut doc = Document::new(256, 256, 600);
    let lines = doc.active();
    ring(&mut doc, lines, 40, 40, 200, 200, 3);
    let other = doc.add_raster_layer().unwrap();
    rect(&mut doc, other, 120, 43, 123, 197, INK); // splits the ring, but is no reference
    let target = doc.add_raster_layer().unwrap();
    let mut p = doc.layer(lines).unwrap().props.clone();
    p.reference = true;
    doc.set_props(lines, p);
    doc.set_active(target);

    let refp = FillParams { reference: FillRef::Reference, ..exact() };
    let r = region(&doc, (60, 100), &refp).unwrap();
    assert_eq!(r.value(150, 100), 255, "the non-reference wall is ignored");
    assert_eq!(r.value(121, 100), 255);
    assert_eq!(r.value(10, 10), 0, "the reference ring holds");

    let active = region(&doc, (60, 100), &exact()).unwrap();
    assert_eq!(active.value(10, 10), 255, "Active reads the empty target");
    let all = region(&doc, (60, 100), &FillParams { reference: FillRef::AllVisible, ..exact() }).unwrap();
    assert_eq!((all.value(150, 100), all.value(10, 10)), (0, 0), "AllVisible sees both");

    // A hidden reference layer does not count; with none, Reference falls
    // back to the active layer.
    let mut p = doc.layer(lines).unwrap().props.clone();
    p.visible = false;
    doc.set_props(lines, p);
    assert_eq!(region(&doc, (60, 100), &refp).unwrap().value(10, 10), 255);
}

#[test]
fn fl11_transparent_seed_on_empty_layers_fills_the_page() {
    let doc = Document::new(300, 200, 600);
    for p in [exact(), FillParams::default(), FillParams { gap_px: 8, ..exact() }, FillParams { gap_px: 32, ..FillParams::default() }]
    {
        let r = region(&doc, (150, 100), &p).unwrap();
        for (x, y) in pixels(&doc) {
            assert_eq!(r.value(x, y), 255, "({x}, {y}) with {p:?}");
        }
        let interior: Vec<_> = r.tiles().filter(|(c, _)| c.x < 4 && c.y < 3).collect();
        assert!(interior.iter().all(|(_, m)| Arc::ptr_eq(m, full_mask())), "interior tiles share the full mask");
    }
    assert!(region(&doc, (-1, 5), &exact()).is_none());
    assert!(region(&doc, (300, 5), &exact()).is_none());
}

#[test]
fn fl12_one_undo_step_restores_the_tiles() {
    let mut doc = Document::new(256, 256, 600);
    let id = doc.active();
    ring(&mut doc, id, 30, 30, 220, 220, 3);
    let before: Vec<(TileCoord, arty_core::TileRef)> =
        doc.layer(id).unwrap().raster().unwrap().iter().map(|(c, t)| (c, t.clone())).collect();
    let r = region(&doc, (100, 100), &FillParams::default()).unwrap();
    let rev = doc.revision();
    let edit = apply_fill(&mut doc, id, &r, RED, 1.0, FillBlend::Normal).unwrap();
    assert!(doc.revision() != rev);
    assert_eq!(get(&doc, id, 100, 100), RED);
    let mut h = History::default();
    h.push(edit);
    h.undo(&mut doc);
    let grid = doc.layer(id).unwrap().raster().unwrap();
    assert_eq!(grid.len(), before.len(), "created tiles are removed");
    for (c, t) in &before {
        assert!(Arc::ptr_eq(grid.get_ref(*c).unwrap(), t), "tile {c:?}");
    }
    h.redo(&mut doc);
    assert_eq!(get(&doc, id, 100, 100), RED);

    let mut p = doc.layer(id).unwrap().props.clone();
    p.locked = true;
    doc.set_props(id, p);
    assert!(apply_fill(&mut doc, id, &r, RED, 1.0, FillBlend::Normal).is_none(), "locked");
    let folder = doc.add_folder().unwrap();
    assert!(apply_fill(&mut doc, folder, &r, RED, 1.0, FillBlend::Normal).is_none(), "folder");
    assert!(fill_selection(&mut doc, folder, RED).is_none());
}

#[test]
fn fl13_fill_selection_covers_exactly_the_selection() {
    let mut doc = Document::new(200, 150, 600);
    let id = doc.active();
    assert!(fill_selection(&mut doc, id, INK).is_none(), "no selection");
    let mut sel = Selection::new();
    let mut m = [[0u8; TILE_SIZE]; TILE_SIZE];
    for (y, row) in m.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = if (x + y) % 3 == 0 { 0 } else { (x * 3 + y * 2) as u8 };
        }
    }
    sel.insert_tile(TileCoord::new(1, 1), Arc::new(m));
    sel.insert_tile(TileCoord::new(0, 0), full_mask().clone());
    sel.insert_tile(TileCoord::new(3, 2), full_mask().clone()); // the page's corner tile
    doc.swap_selection(sel.clone());
    let edit = fill_selection(&mut doc, id, INK).unwrap();
    let arty_core::Edit::Pixels { tiles, .. } = edit else { panic!("pixel edit") };
    assert_eq!(tiles.len(), 3);
    for (x, y) in pixels(&doc) {
        let k = (sel.value(x, y) as u32 * O as u32 + 127) / 255;
        assert_eq!(get(&doc, id, x, y), [0, 0, 0, k as u16], "({x}, {y})");
    }
    let grid = doc.layer(id).unwrap().raster().unwrap();
    assert_eq!(grid.len(), 3, "nothing written outside the selection");
}
