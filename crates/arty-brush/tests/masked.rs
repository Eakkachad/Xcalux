//! SEL-CORE (plans/m3_page_tools.md §9.1, tests 8–10): painting is limited
//! to the selection.

use std::sync::Arc;

use arty_brush::shape::DabStats;
use arty_brush::{InputSample, LayerSurface, MaskCur, Reshape, StrokeEngine, default_presets};
use arty_core::selection::full_mask;
use arty_core::tile::new_tile_box;
use arty_core::{Document, Edit, MaskPixels, PixelRecorder, Selection, TILE_SIZE, TileCoord};
use hokusai::TiledSurface;

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

const T: usize = TILE_SIZE;

fn sample(x: f32, y: f32, p: f32, t: f64) -> InputSample {
    InputSample { x, y, pressure: p, time: t, ..Default::default() }
}

fn preset(name: &str) -> arty_brush::BrushPreset {
    default_presets().into_iter().find(|p| p.name == name).unwrap()
}

fn pixel_at(doc: &Document, x: i32, y: i32) -> [u16; 4] {
    let c = TileCoord::from_pixel(x, y);
    let (ox, oy) = c.origin();
    doc.active_layer().raster().unwrap().get(c).map(|t| t[(y - oy) as usize][(x - ox) as usize]).unwrap_or([0; 4])
}

/// A tile whose left `split` columns are `left` and the rest `right`.
fn split_tile(split: usize, left: u8, right: u8) -> Arc<MaskPixels> {
    let mut m: MaskPixels = [[right; T]; T];
    for row in &mut m {
        row[..split].fill(left);
    }
    Arc::new(m)
}

/// Tiles (0..8, 1..3) of a 512 × 256 page: row 1 fully selected except
/// tiles 3 (unselected) and 4 (left half selected); row 2 uniform `m`.
fn test_selection(m: u8) -> Selection {
    let mut s = Selection::new();
    for tx in 0..8 {
        match tx {
            3 => {}
            4 => {
                s.insert_tile(TileCoord::new(tx, 1), split_tile(32, 255, 0));
            }
            _ => {
                s.insert_tile(TileCoord::new(tx, 1), full_mask().clone());
            }
        }
        s.insert_tile(TileCoord::new(tx, 2), Arc::new([[m; T]; T]));
    }
    s
}

fn stroke(engine: &mut StrokeEngine, doc: &mut Document, y: f32) -> Option<Edit> {
    engine.begin(doc, sample(20.0, y, 0.8, 0.0)).unwrap();
    for i in 1..=150 {
        engine.feed(doc, sample(20.0 + i as f32 * 3.0, y, 0.8, i as f64 * 0.005));
    }
    engine.end(doc)
}

#[test]
fn sc08_masked_stroke_stays_inside_the_selection() {
    let mut doc = Document::new(512, 256, 350);
    let id = doc.active();
    // Existing ink everywhere on row 1, so "unchanged" is checked against
    // real pixels, not just transparency.
    {
        let (grid, _) = doc.paint_target(id).unwrap();
        for tx in 0..8 {
            if tx != 3 {
                grid.get_mut_or_create(TileCoord::new(tx, 1)).as_flattened_mut().fill([1000, 2000, 3000, 9000]);
            }
        }
    }
    let before = doc.snapshot();
    doc.swap_selection(test_selection(0));
    let mut engine = StrokeEngine::new();
    engine.configure(&arty_brush::BrushPreset { stabilizer: 0, ..preset("G-Pen") }, [0.0; 3]);
    let edit = stroke(&mut engine, &mut doc, 96.0).expect("painted inside the selection");

    // Selected: painted. Mask 0 (the unselected tile, the right half of
    // tile 4): every pixel unchanged.
    assert!(pixel_at(&doc, 100, 96)[3] > 30000);
    assert!(pixel_at(&doc, 4 * 64 + 10, 96)[3] > 30000);
    for x in 0..512 {
        let c = TileCoord::from_pixel(x, 96);
        let unselected = c.x == 3 || (c.x == 4 && x % 64 >= 32);
        for y in 80..112 {
            if unselected {
                assert_eq!(pixel_at(&doc, x, y), pixel_at(&before, x, y), "({x}, {y}) is not selected");
            }
        }
    }
    // The unselected tile was never created, recorded or dirtied.
    let unselected = TileCoord::new(3, 1);
    assert!(doc.active_layer().raster().unwrap().get(unselected).is_none());
    let Edit::Pixels { tiles, .. } = &edit else { panic!("a pixel edit") };
    assert!(tiles.iter().all(|(c, _)| *c != unselected), "no undo record for an unselected tile");
    assert!(tiles.iter().any(|(c, _)| *c == TileCoord::new(4, 1)));
}

/// A plain round dab (straight-alpha color, fully opaque).
fn dab(x: f32, y: f32, radius: f32, opaque: f32) -> hokusai::Dab {
    hokusai::Dab {
        x,
        y,
        radius,
        color: hokusai::color::RgbaF32 { r: 0.8, g: 0.3, b: 0.1, a: 1.0 },
        opaque,
        hardness: 0.7,
        alpha_eraser: 1.0,
        aspect_ratio: 1.0,
        angle: 0.0,
        lock_alpha: 0.0,
        colorize: 0.0,
        posterize: 0.0,
        posterize_num: 0.0,
        paint: 0.0,
        anti_aliasing: 1.0,
    }
}

/// Draw one dab straight through a `LayerSurface` on `doc`.
fn draw_one(doc: &mut Document, d: &hokusai::Dab) {
    let (tw, th) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
    let mut recorder = PixelRecorder::default();
    let mut discard = new_tile_box();
    let mut stats = DabStats::default();
    let (grid, dirty, mask) = doc.paint_target_masked(doc.active()).unwrap();
    let mut s = LayerSurface {
        grid,
        dirty,
        recorder: &mut recorder,
        discard: &mut discard,
        tiles_wide: tw,
        tiles_high: th,
        clip: None,
        stats: &mut stats,
        mask,
        mask_cur: MaskCur::default(),
    };
    s.draw_dab(d);
}

#[test]
fn sc08_partial_mask_scales_the_change() {
    for (m, opaque, base) in [(128u8, 1.0f32, None), (40, 0.6, None), (200, 0.9, Some([4000u16, 3000, 2000, 12000]))] {
        let mut plain = Document::new(256, 256, 350);
        if let Some(px) = base {
            let id = plain.active();
            let (grid, _) = plain.paint_target(id).unwrap();
            grid.get_mut_or_create(TileCoord::new(1, 1)).as_flattened_mut().fill(px);
        }
        let mut masked = plain.snapshot();
        let mut sel = Selection::new();
        sel.insert_tile(TileCoord::new(1, 1), Arc::new([[m; T]; T]));
        masked.swap_selection(sel);
        let d = dab(96.0, 96.0, 20.0, opaque);
        let orig = plain.snapshot();
        draw_one(&mut plain, &d);
        draw_one(&mut masked, &d);
        let k = m as f32 / 255.0;
        let mut seen = 0;
        for y in 70..122 {
            for x in 70..122 {
                let (o, p, q) = (pixel_at(&orig, x, y), pixel_at(&plain, x, y), pixel_at(&masked, x, y));
                for ch in 0..4 {
                    let full = p[ch] as f32 - o[ch] as f32;
                    let got = q[ch] as f32 - o[ch] as f32;
                    // fix15 rounding: the mask is truncated to 1/32768 steps
                    // before blending, so allow a few units.
                    assert!((got - full * k).abs() <= 3.0 + full.abs() * 2e-4, "m {m} ({x}, {y}) ch {ch}: {got} vs {full}·{k}");
                }
                seen += usize::from(p != o);
                assert!(q[0] <= q[3] && q[1] <= q[3] && q[2] <= q[3]);
            }
        }
        assert!(seen > 500, "the dab painted");
    }
}

/// G4 gate with a selection: feeding samples must not allocate once every
/// stroke tile has been copied for undo, for every default preset.
#[test]
fn sc09_masked_feed_is_allocation_free_in_steady_state() {
    let mut doc = Document::new(512, 512, 350);
    // Full, partial, uniform-partial and unselected tiles along y = 224
    // (mid tile row, so jitter never reaches another row).
    let mut s = Selection::new();
    for tx in 0..8 {
        for ty in 1..6 {
            let c = TileCoord::new(tx, ty);
            match (tx + ty) % 4 {
                0 => {}
                1 => {
                    s.insert_tile(c, full_mask().clone());
                }
                2 => {
                    s.insert_tile(c, split_tile(20, 255, 60));
                }
                _ => {
                    s.insert_tile(c, Arc::new([[150; T]; T]));
                }
            }
        }
    }
    doc.swap_selection(s);
    let mut engine = StrokeEngine::new();
    for p in default_presets() {
        let name = p.name.clone();
        engine.configure(&p, [0.2, 0.3, 0.4]);
        engine.begin(&mut doc, sample(50.0, 224.0, 0.5, 0.0)).unwrap();
        let mut t = 0.0;
        let xs = |i: i32| 50.0 + i as f32 * 2.0;
        for i in (1..200).chain((1..200).rev()) {
            t += 0.004;
            engine.feed(&mut doc, sample(xs(i), 224.0, 0.7, t));
        }
        let tiles_before = doc.active_layer().raster().unwrap().len();
        let n = arty_testkit::count_allocs(|| {
            for i in 1..200 {
                t += 0.004;
                engine.feed(&mut doc, sample(xs(i), 224.0, 0.7, t));
            }
        });
        assert_eq!(doc.active_layer().raster().unwrap().len(), tiles_before, "{name}: measured pass reached new tiles");
        engine.end(&mut doc);
        assert_eq!(n, 0, "{name}: masked feed allocated {n} times");
    }
}

/// Wavy line across the page; the last 100 px are at zero pressure, so an
/// exit taper changes no input there.
fn wavy() -> Vec<(f32, f32, f32)> {
    (0..=120)
        .map(|i| {
            let t = i as f32 / 120.0;
            let (x, y) = (30.0 + 440.0 * t, 128.0 + 30.0 * (t * 9.0).sin());
            (x, y, if x > 340.0 { 0.0 } else { 0.4 + 0.5 * (t * 5.0).sin().abs() })
        })
        .collect()
}

fn paint_masked(p: &arty_brush::BrushPreset, path: &[(f32, f32, f32)]) -> (Document, StrokeEngine) {
    let mut doc = Document::new(512, 256, 350);
    // A lasso-like selection crossing the path, with soft edges.
    let pts: Vec<[f32; 2]> = (0..200)
        .map(|i| {
            let a = i as f32 / 200.0 * std::f32::consts::TAU;
            [250.0 + 190.0 * a.cos(), 128.0 + 70.0 * (a.sin() + 0.2 * (5.0 * a).sin())]
        })
        .collect();
    let mut sel = arty_core::raster::rasterize_polygon(&pts, 512, 256, true);
    sel = arty_core::morph::feather(&sel, 6, 512, 256);
    doc.swap_selection(sel);
    let mut engine = StrokeEngine::new();
    engine.configure(p, [0.1, 0.1, 0.1]);
    let (x, y, pr) = path[0];
    engine.begin(&mut doc, sample(x, y, pr, 0.0)).unwrap();
    for (i, &(x, y, pr)) in path.iter().enumerate().skip(1) {
        engine.feed(&mut doc, sample(x, y, pr, i as f64 * 0.005));
    }
    engine.end(&mut doc);
    (doc, engine)
}

fn differing_pixels(a: &Document, b: &Document) -> usize {
    let (ga, gb) = (a.active_layer().raster().unwrap(), b.active_layer().raster().unwrap());
    let blank = arty_core::tile::new_tile();
    let mut coords: Vec<TileCoord> = ga.coords().chain(gb.coords()).collect();
    coords.sort();
    coords.dedup();
    coords
        .into_iter()
        .map(|c| {
            let (ta, tb) = (ga.get(c).unwrap_or(&blank), gb.get(c).unwrap_or(&blank));
            ta.iter().flatten().zip(tb.iter().flatten()).filter(|(p, q)| p != q).count()
        })
        .sum()
}

#[test]
fn sc10_replay_with_a_selection_matches_the_live_stroke() {
    let shaped = |name: &str, taper_out: f32| arty_brush::BrushPreset {
        stabilizer: 0,
        taper_in: 0.0,
        taper_out,
        post_correction: 0,
        ..preset(name)
    };
    for (name, tail) in [("G-Pen", true), ("Pencil", true), ("Brush", false), ("Watercolor", false)] {
        let (replayed, engine) = paint_masked(&shaped(name, 60.0), &wavy());
        let r = engine.last_reshape();
        assert!(if tail { matches!(r, Reshape::Tail { .. }) } else { r == Reshape::Full }, "{name}: {r:?}");
        let (live, engine) = paint_masked(&shaped(name, 0.0), &wavy());
        assert_eq!(engine.last_reshape(), Reshape::Skipped);
        assert!(!live.active_layer().raster().unwrap().is_empty(), "{name}: painted something");
        assert_eq!(differing_pixels(&replayed, &live), 0, "{name}: the replay differs from the live stroke");
    }
}
