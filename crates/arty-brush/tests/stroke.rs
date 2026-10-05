use arty_brush::shape::{ShapeSample, seg_len, taper};
use arty_brush::{InputSample, Reshape, StrokeEngine, StrokeRefused, default_presets};
use arty_core::{Document, History, TileCoord};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

fn sample(x: f32, y: f32, p: f32, t: f64) -> InputSample {
    InputSample { x, y, pressure: p, time: t, ..Default::default() }
}

fn alpha_at(doc: &Document, x: i32, y: i32) -> u16 {
    let c = TileCoord::from_pixel(x, y);
    let (ox, oy) = c.origin();
    doc.active_layer()
        .raster()
        .unwrap()
        .get(c)
        .map(|t| t[(y - oy) as usize][(x - ox) as usize][3])
        .unwrap_or(0)
}

fn preset(name: &str) -> arty_brush::BrushPreset {
    default_presets().into_iter().find(|p| p.name == name).unwrap()
}

fn line(engine: &mut StrokeEngine, doc: &mut Document, y: f32) {
    engine.begin(doc, sample(20.0, y, 0.0, 0.0)).unwrap();
    for i in 1..=60 {
        engine.feed(doc, sample(20.0 + i as f32 * 3.0, y, 0.8, i as f64 * 0.005));
    }
}

#[test]
fn pen_stroke_paints_and_undoes() {
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("G-Pen"), [0.0, 0.0, 0.0]);
    line(&mut engine, &mut doc, 100.0);
    let edit = engine.end(&mut doc).expect("stroke painted");
    assert!(alpha_at(&doc, 120, 100) > 30000, "line should be solid ink");
    assert_eq!(alpha_at(&doc, 120, 140), 0);

    let mut h = History::default();
    h.push(edit);
    h.undo(&mut doc);
    assert_eq!(alpha_at(&doc, 120, 100), 0);
}

#[test]
fn eraser_removes_ink() {
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("G-Pen"), [0.0; 3]);
    line(&mut engine, &mut doc, 100.0);
    engine.end(&mut doc);
    engine.configure(&preset("Hard Eraser"), [0.0; 3]);
    line(&mut engine, &mut doc, 100.0);
    engine.end(&mut doc);
    assert!(alpha_at(&doc, 120, 100) < 1000);
}

#[test]
fn locked_layer_refuses() {
    let mut doc = Document::new(64, 64, 72);
    let id = doc.active();
    let mut p = doc.layer(id).unwrap().props.clone();
    p.locked = true;
    doc.set_props(id, p);
    let mut engine = StrokeEngine::new();
    assert_eq!(engine.begin(&mut doc, sample(1.0, 1.0, 1.0, 0.0)), Err(StrokeRefused::Locked));
}

#[test]
fn painting_off_page_is_discarded() {
    let mut doc = Document::new(64, 64, 72);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("G-Pen"), [0.0; 3]);
    engine.begin(&mut doc, sample(-200.0, -200.0, 1.0, 0.0)).unwrap();
    engine.feed(&mut doc, sample(-100.0, -200.0, 1.0, 0.01));
    engine.end(&mut doc);
    assert!(doc.active_layer().raster().unwrap().is_empty());
}

/// G4 gate: feeding samples must not allocate in steady state.
///
/// The first write to a tile in each stroke copies it for undo (one
/// allocation per tile per stroke, by design). Once a stroke has touched
/// its tiles, every further sample must be allocation-free.
#[test]
fn feed_is_allocation_free_in_steady_state() {
    let mut doc = Document::new(512, 512, 350);
    let mut engine = StrokeEngine::new();
    // "Inking Pen" has taper in/out and post correction: the sample log is
    // reserved in `begin`, so recording must not allocate either.
    for name in ["G-Pen", "Inking Pen", "Mapping Pen", "Brush", "Pencil", "Airbrush", "Hard Eraser"] {
        let p = preset(name);
        assert!(name != "Inking Pen" || (p.taper_in > 0.0 && p.taper_out > 0.0 && p.post_correction > 0));
        engine.configure(&p, [0.2, 0.3, 0.4]);
        engine.begin(&mut doc, sample(50.0, 200.0, 0.5, 0.0)).unwrap();
        let mut t = 0.0;
        let xs = |i: i32| 50.0 + i as f32 * 2.0;
        // An out-and-back warm-up touches (and copies) every tile.
        for i in (1..200).chain((1..200).rev()) {
            t += 0.004;
            engine.feed(&mut doc, sample(xs(i), 200.0, 0.7, t));
        }
        let tiles_before = doc.active_layer().raster().unwrap().len();
        // A further pass over the same tiles is measured.
        let n = arty_testkit::count_allocs(|| {
            for i in 1..200 {
                t += 0.004;
                engine.feed(&mut doc, sample(xs(i), 200.0, 0.7, t));
            }
        });
        assert_eq!(doc.active_layer().raster().unwrap().len(), tiles_before, "{name}: measured pass reached new tiles");
        engine.end(&mut doc);
        assert_eq!(n, 0, "{name}: feed allocated {n} times");
    }
}

#[test]
fn every_paint_preset_leaves_ink() {
    for p in default_presets().into_iter().filter(|p| !p.eraser && p.blending < 1.0) {
        let mut doc = Document::new(512, 256, 350);
        let mut engine = StrokeEngine::new();
        engine.configure(&p, [0.0; 3]);
        engine.begin(&mut doc, sample(40.0, 128.0, 0.0, 0.0)).unwrap();
        for i in 1..=100 {
            let t = i as f32 / 100.0;
            engine.feed(&mut doc, sample(40.0 + 430.0 * t, 128.0, (t * std::f32::consts::PI).sin(), i as f64 * 0.006));
        }
        engine.end(&mut doc);
        let a = alpha_at(&doc, 255, 128);
        println!("{:>14}: center alpha {:.3}", p.name, a as f32 / 32768.0);
        assert!(a > 2000, "{} painted almost nothing (alpha {a})", p.name);
    }
}

fn pixel_at(doc: &Document, x: i32, y: i32) -> [u16; 4] {
    let c = TileCoord::from_pixel(x, y);
    let (ox, oy) = c.origin();
    doc.active_layer()
        .raster()
        .unwrap()
        .get(c)
        .map(|t| t[(y - oy) as usize][(x - ox) as usize])
        .unwrap_or([0; 4])
}

/// A horizontal line from x = 20 to 200 at constant `pressure`.
fn line_at(engine: &mut StrokeEngine, doc: &mut Document, y: f32, pressure: f32) {
    engine.begin(doc, sample(20.0, y, pressure, 0.0)).unwrap();
    for i in 1..=60 {
        engine.feed(doc, sample(20.0 + i as f32 * 3.0, y, pressure, i as f64 * 0.005));
    }
    engine.end(doc);
}

/// Strongest alpha across the line's width at `x`.
fn peak_alpha(doc: &Document, x: i32, y: i32) -> u16 {
    (y - 3..=y + 3).map(|y| alpha_at(doc, x, y)).max().unwrap()
}

fn lock_alpha(doc: &mut Document) {
    let id = doc.active();
    let mut p = doc.layer(id).unwrap().props.clone();
    p.lock_alpha = true;
    doc.set_props(id, p);
}

#[test]
fn tap_leaves_a_dot() {
    // Pen: pressure rises and falls in place; touch lift-off reports 0.
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("G-Pen"), [0.0; 3]);
    engine.begin(&mut doc, sample(100.0, 100.0, 0.3, 0.0)).unwrap();
    engine.feed(&mut doc, sample(100.0, 100.0, 0.8, 0.01));
    engine.feed(&mut doc, sample(100.0, 100.0, 0.0, 0.02));
    assert!(engine.end(&mut doc).is_some(), "a tap is an undoable edit");
    assert!(peak_alpha(&doc, 100, 100) > 20000, "pen tap should leave a dot");
    // One dot, not a smear.
    assert_eq!(alpha_at(&doc, 100, 110), 0);
    assert_eq!(alpha_at(&doc, 110, 100), 0);

    // Mouse: press and release without moving.
    engine.configure(&preset("Pencil"), [0.0; 3]);
    engine.begin(&mut doc, sample(50.0, 50.0, 1.0, 0.0)).unwrap();
    assert!(engine.end(&mut doc).is_some());
    assert!(peak_alpha(&doc, 50, 50) > 5000, "click should leave a dot");

    // Zero pressure throughout paints nothing.
    engine.begin(&mut doc, sample(150.0, 150.0, 0.0, 0.0)).unwrap();
    assert!(engine.end(&mut doc).is_none());
}

#[test]
fn thin_pens_still_paint() {
    // G-Pen at 1 px: optical radius below the anti-aliasing floor.
    let mut tiny = preset("G-Pen");
    tiny.size = 1.0;
    // Mapping Pen's light-pressure taper, and a 2 px all-soft brush (soft
    // brushes are faint by nature).
    let mut soft = preset("Flat Color");
    soft.size = 2.0;
    soft.hardness = 0.0;
    for (p, pressure, min) in [(tiny, 0.8, 6000), (preset("Mapping Pen"), 0.1, 4000), (soft, 1.0, 400)] {
        let mut doc = Document::new(256, 256, 350);
        let mut engine = StrokeEngine::new();
        engine.configure(&p, [0.0; 3]);
        line_at(&mut engine, &mut doc, 100.0, pressure);
        let a = peak_alpha(&doc, 120, 100);
        assert!(a > min, "{} at pressure {pressure} painted almost nothing (alpha {a})", p.name);
        assert!(a < 32768, "{}: a sub-pixel line should be faint, not solid", p.name);
    }

    // The taper thins out smoothly instead of cutting off.
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("Mapping Pen"), [0.0; 3]);
    let mut prev = u16::MAX;
    for (i, pressure) in [0.6, 0.3, 0.15, 0.05].into_iter().enumerate() {
        let y = 40.0 + i as f32 * 40.0;
        line_at(&mut engine, &mut doc, y, pressure);
        let a = peak_alpha(&doc, 120, y as i32);
        assert!(a > 0 && a <= prev, "pressure {pressure}: alpha {a} (previous {prev})");
        prev = a;
    }
}

#[test]
fn full_persistence_does_not_erase() {
    for name in ["Blender", "Brush"] {
        let mut doc = Document::new(256, 256, 350);
        let mut engine = StrokeEngine::new();
        engine.configure(&preset("Flat Color"), [0.2, 0.4, 0.8]);
        for y in [85.0, 100.0, 115.0] {
            line_at(&mut engine, &mut doc, y, 1.0);
        }
        assert!(alpha_at(&doc, 120, 100) > 32000);

        let mut p = preset(name);
        p.persistence = 1.0;
        engine.configure(&p, [0.2, 0.4, 0.8]);
        line_at(&mut engine, &mut doc, 100.0, 0.8);
        let a = alpha_at(&doc, 120, 100);
        assert!(a > 30000, "{name} at 100% persistence erased solid paint (alpha {a})");
    }
}

#[test]
fn eraser_is_refused_on_alpha_locked_layer() {
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("G-Pen"), [1.0, 0.0, 0.0]);
    line_at(&mut engine, &mut doc, 100.0, 1.0);
    let before = pixel_at(&doc, 120, 100);
    lock_alpha(&mut doc);
    for name in ["Hard Eraser", "Soft Eraser"] {
        engine.configure(&preset(name), [0.0; 3]);
        assert_eq!(engine.begin(&mut doc, sample(20.0, 100.0, 1.0, 0.0)), Err(StrokeRefused::AlphaLocked));
        engine.feed(&mut doc, sample(200.0, 100.0, 1.0, 0.1));
        assert!(engine.end(&mut doc).is_none());
    }
    assert_eq!(pixel_at(&doc, 120, 100), before);
}

#[test]
fn blending_on_alpha_locked_layer_keeps_alpha_and_color() {
    // A soft red shape with plenty of partially transparent edge pixels.
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    let shape = arty_brush::BrushPreset { size: 40.0, min_size: 1.0, hardness: 0.1, ..preset("Flat Color") };
    engine.configure(&shape, [1.0, 0.0, 0.0]);
    line_at(&mut engine, &mut doc, 128.0, 1.0);
    let before: Vec<[u16; 4]> = (0..256 * 256).map(|i| pixel_at(&doc, i % 256, i / 256)).collect();
    lock_alpha(&mut doc);

    // Start in empty space so the smudge bucket begins transparent, then
    // drag across the shape's edge. The Blender's own color (black) must
    // never show; the others paint red too, so any change of hue or
    // darkening is the lock-alpha blend going wrong.
    for (name, color) in [("Blender", [0.0; 3]), ("Brush", [1.0, 0.0, 0.0]), ("Watercolor", [1.0, 0.0, 0.0])] {
        engine.configure(&preset(name), color);
        engine.begin(&mut doc, sample(60.0, 60.0, 0.8, 0.0)).unwrap();
        for i in 1..=60 {
            let t = i as f32 / 60.0;
            engine.feed(&mut doc, sample(60.0 + 140.0 * t, 60.0 + 136.0 * t, 0.8, i as f64 * 0.006));
        }
        engine.end(&mut doc);
    }

    for (i, b) in before.iter().enumerate() {
        let (x, y) = ((i % 256) as i32, (i / 256) as i32);
        let p = pixel_at(&doc, x, y);
        assert_eq!(p[3], b[3], "alpha changed at ({x}, {y})");
        if p[3] > 1600 {
            let red = p[0] as f32 / p[3] as f32;
            assert!(red > 0.9, "({x}, {y}) darkened: {p:?} (was {b:?})");
        }
    }
}

#[test]
fn blender_preview_shows_a_smear() {
    let blender = preset("Blender");
    let (w, h) = (192, 48);
    let color = [0.9, 0.2, 0.1];
    let preview = arty_brush::render_preview(&blender, w, h, color);
    // Same backdrop under a blender that cannot touch the canvas.
    let still = arty_brush::BrushPreset { opacity: 0.0, ..blender };
    let backdrop = arty_brush::render_preview(&still, w, h, color);
    let covered = preview.chunks(4).filter(|p| p[3] > 0).count();
    let changed = preview.chunks(4).zip(backdrop.chunks(4)).filter(|(a, b)| a != b).count();
    assert!(covered > (w * h) as usize / 10, "blender preview is (nearly) empty: {covered} px");
    assert!(changed > (w * h) as usize / 50, "blender preview shows no smear: {changed} px differ");
}

// ----- stroke shaping (taper, post correction, replay) ----------------------

/// `name` with the stabilizer off and the given taper in / out and post correction.
fn shaped(name: &str, taper_in: f32, taper_out: f32, post_correction: u8) -> arty_brush::BrushPreset {
    arty_brush::BrushPreset { stabilizer: 0, taper_in, taper_out, post_correction, ..preset(name) }
}

/// Paint `path` as (x, y, pressure) at 200 Hz and end the stroke.
fn draw(engine: &mut StrokeEngine, doc: &mut Document, path: &[(f32, f32, f32)]) -> Option<arty_core::Edit> {
    let (x, y, p) = path[0];
    engine.begin(doc, sample(x, y, p, 0.0)).unwrap();
    for (i, &(x, y, p)) in path.iter().enumerate().skip(1) {
        engine.feed(doc, sample(x, y, p, i as f64 * 0.005));
    }
    engine.end(doc)
}

/// Fresh engine and page, `path` painted with `p`.
fn paint(p: &arty_brush::BrushPreset, path: &[(f32, f32, f32)]) -> (Document, StrokeEngine) {
    let mut doc = Document::new(512, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(p, [0.1, 0.1, 0.1]);
    draw(&mut engine, &mut doc, path);
    (doc, engine)
}

/// Pixels differing between the two documents' active layers (missing tiles are transparent).
fn differing_pixels(a: &Document, b: &Document) -> usize {
    let (ga, gb) = (a.active_layer().raster().unwrap(), b.active_layer().raster().unwrap());
    let blank = arty_core::tile::new_tile();
    let empty: &arty_core::TilePixels = &blank;
    let mut coords: Vec<TileCoord> = ga.coords().chain(gb.coords()).collect();
    coords.sort();
    coords.dedup();
    let mut n = 0;
    for c in coords {
        let (ta, tb) = (ga.get(c).unwrap_or(empty), gb.get(c).unwrap_or(empty));
        n += ta.iter().flatten().zip(tb.iter().flatten()).filter(|(p, q)| p != q).count();
    }
    n
}

/// Rows within ±24 px of `y` where the line at column `x` is at least half opaque.
fn width_at(doc: &Document, x: i32, y: i32) -> usize {
    (y - 24..=y + 24).filter(|&y| alpha_at(doc, x, y) > 16384).count()
}

/// Wavy line across the page with varying pressure.
fn wavy() -> Vec<(f32, f32, f32)> {
    (0..=120)
        .map(|i| {
            let t = i as f32 / 120.0;
            (30.0 + 440.0 * t, 128.0 + 30.0 * (t * 9.0).sin(), 0.4 + 0.5 * (t * 5.0).sin().abs())
        })
        .collect()
}

/// A loop of 1.4 turns: its tail runs back across its start.
fn looped() -> Vec<(f32, f32, f32)> {
    (0..=140)
        .map(|i| {
            let a = std::f32::consts::PI + i as f32 / 100.0 * std::f32::consts::TAU;
            (256.0 + 70.0 * a.cos(), 128.0 + 70.0 * a.sin(), 0.7 + 0.2 * (i as f32 * 0.1).sin())
        })
        .collect()
}

/// Straight horizontal line at y = 100, 3 px steps from x = 20 to 419, constant pressure.
fn straight(pressure: f32) -> Vec<(f32, f32, f32)> {
    (0..=133).map(|i| (20.0 + i as f32 * 3.0, 100.0, pressure)).collect()
}

/// Horizontal line at y = 128 zigzagging ±1.5 px every 3 px (hand tremor).
fn jittery() -> Vec<(f32, f32, f32)> {
    (0..=150).map(|i| (30.0 + i as f32 * 3.0, if i % 2 == 0 { 129.5 } else { 126.5 }, 0.8)).collect()
}

#[test]
fn shaped_stroke_equals_drawing_pre_tapered_path() {
    let (tin, tout) = (30.0, 60.0);
    for (name, tail) in [("G-Pen", true), ("Pencil", true), ("Brush", false)] {
        for (label, path) in [("wavy", wavy()), ("loop", looped())] {
            let (doc, engine) = paint(&shaped(name, tin, tout, 0), &path);
            let r = engine.last_reshape();
            assert!(if tail { matches!(r, Reshape::Tail { tiles } if tiles > 0) } else { r == Reshape::Full }, "{name}/{label}: {r:?}");

            // Reference: a plain engine fed the final tapered pressures.
            let pts: Vec<ShapeSample> = path.iter().map(|&(x, y, _)| ShapeSample { x, y, ..Default::default() }).collect();
            let total: f32 = pts.windows(2).map(|w| seg_len(&w[0], &w[1])).sum();
            let mut s = 0.0f32;
            let tapered: Vec<_> = path
                .iter()
                .enumerate()
                .map(|(i, &(x, y, p))| {
                    if i > 0 {
                        s += seg_len(&pts[i - 1], &pts[i]);
                    }
                    (x, y, p * taper(s, Some(total), tin, tout))
                })
                .collect();
            let (reference, _) = paint(&shaped(name, 0.0, 0.0, 0), &tapered);
            let n = differing_pixels(&doc, &reference);
            assert_eq!(n, 0, "{name}/{label}: {n} pixels differ from the pre-tapered stroke");
        }
    }
}

#[test]
fn replay_of_unchanged_pressures_is_pixel_identical_to_live() {
    // The last 100 px are drawn at zero pressure, so the exit taper changes
    // no input: the replay must reproduce the live stroke exactly.
    let path: Vec<_> = wavy().into_iter().map(|(x, y, p)| (x, y, if x > 340.0 { 0.0 } else { p })).collect();
    for (name, tail) in [("G-Pen", true), ("Pencil", true), ("Brush", false), ("Watercolor", false)] {
        let (replayed, engine) = paint(&shaped(name, 0.0, 60.0, 0), &path);
        let r = engine.last_reshape();
        assert!(if tail { matches!(r, Reshape::Tail { .. }) } else { r == Reshape::Full }, "{name}: {r:?}");
        let (live, engine) = paint(&shaped(name, 0.0, 0.0, 0), &path);
        assert_eq!(engine.last_reshape(), Reshape::Skipped);
        assert_eq!(differing_pixels(&replayed, &live), 0, "{name}: replay differs from the live stroke");
    }
}

#[test]
fn exit_taper_thins_the_end() {
    let pen = arty_brush::BrushPreset { size: 16.0, ..shaped("G-Pen", 0.0, 120.0, 0) };
    let (doc, engine) = paint(&pen, &straight(1.0));
    assert!(matches!(engine.last_reshape(), Reshape::Tail { .. }));
    let mid = width_at(&doc, 200, 100);
    assert!(mid >= 12, "full width {mid}");
    // Linear in arc length over the last 120 px (x 299 → 419).
    let widths: Vec<_> = [300, 330, 360, 390, 410].iter().map(|&x| width_at(&doc, x, 100)).collect();
    assert!(widths[0] + 1 >= mid, "taper starts at the taper length: {widths:?} vs {mid}");
    assert!(widths.windows(2).all(|w| w[1] < w[0]), "width falls along the taper: {widths:?}");
    assert!(widths[4] * 3 < mid, "the end is thin: {widths:?} vs {mid}");

    let plain = arty_brush::BrushPreset { taper_out: 0.0, ..pen };
    let (doc, _) = paint(&plain, &straight(1.0));
    assert!(width_at(&doc, 410, 100) + 1 >= mid, "without taper the end keeps its width");
}

#[test]
fn entry_taper_is_live() {
    let pen = arty_brush::BrushPreset { size: 16.0, ..shaped("G-Pen", 120.0, 60.0, 0) };
    let path = straight(1.0);
    let mut doc = Document::new(512, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&pen, [0.0; 3]);
    let (x, y, p) = path[0];
    engine.begin(&mut doc, sample(x, y, p, 0.0)).unwrap();
    for (i, &(x, y, p)) in path.iter().enumerate().skip(1) {
        engine.feed(&mut doc, sample(x, y, p, i as f64 * 0.005));
    }
    // Before pen-up the start is already narrow.
    let widths: Vec<_> = [30, 60, 90, 120].iter().map(|&x| width_at(&doc, x, 100)).collect();
    let mid = width_at(&doc, 250, 100);
    assert!(widths.windows(2).all(|w| w[0] < w[1]), "width grows along the entry: {widths:?}");
    assert!(widths[0] * 3 < mid, "the start is thin: {widths:?} vs {mid}");
    let start: Vec<_> = (0..200).flat_map(|x| (70..130).map(move |y| (x, y))).map(|(x, y)| pixel_at(&doc, x, y)).collect();
    engine.end(&mut doc);
    assert!(matches!(engine.last_reshape(), Reshape::Tail { .. }));
    let after: Vec<_> = (0..200).flat_map(|x| (70..130).map(move |y| (x, y))).map(|(x, y)| pixel_at(&doc, x, y)).collect();
    assert!(start == after, "pen-up must not touch the entry");
}

#[test]
fn shaped_stroke_is_one_undo_step() {
    for (tout, corr) in [(80.0, 0), (0.0, 3), (80.0, 3)] {
        let p = arty_brush::BrushPreset { taper_out: tout, post_correction: corr, ..preset("Inking Pen") };
        // Empty page: undo leaves nothing.
        let mut doc = Document::new(512, 256, 350);
        let mut engine = StrokeEngine::new();
        engine.configure(&p, [0.0; 3]);
        let mut h = History::default();
        h.push(draw(&mut engine, &mut doc, &jittery()).expect("painted"));
        assert_ne!(engine.last_reshape(), Reshape::Skipped, "({tout}, {corr})");
        h.undo(&mut doc);
        assert!(doc.active_layer().raster().unwrap().is_empty(), "({tout}, {corr}): undo left ink");

        // Over existing ink: undo restores the page exactly, in one step.
        engine.configure(&preset("Flat Color"), [0.8, 0.1, 0.1]);
        h.push(draw(&mut engine, &mut doc, &straight(1.0)).unwrap());
        let before = doc.snapshot();
        engine.configure(&p, [0.0; 3]);
        h.push(draw(&mut engine, &mut doc, &wavy()).unwrap());
        assert!(differing_pixels(&doc, &before) > 0);
        let steps = h.undo_len();
        h.undo(&mut doc);
        assert_eq!(h.undo_len(), steps - 1);
        assert_eq!(differing_pixels(&doc, &before), 0, "({tout}, {corr}): undo did not restore the page");
    }
}

/// Alpha-weighted ink centroid (y) of each column in `xs` around y = 128.
fn centroids(doc: &Document, xs: std::ops::Range<i32>) -> Vec<f32> {
    xs.map(|x| {
        let (mut m, mut w) = (0.0f32, 0.0f32);
        for y in 110..=146 {
            let a = alpha_at(doc, x, y) as f32;
            m += a * y as f32;
            w += a;
        }
        m / w.max(1.0)
    })
    .collect()
}

fn variance(v: &[f32]) -> f32 {
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32
}

#[test]
fn post_correction_smooths_jitter() {
    let pen = arty_brush::BrushPreset { size: 6.0, ..shaped("G-Pen", 0.0, 0.0, 0) };
    let (raw, _) = paint(&pen, &jittery());
    let (smooth, engine) = paint(&arty_brush::BrushPreset { post_correction: 6, ..pen.clone() }, &jittery());
    assert_eq!(engine.last_reshape(), Reshape::Full);
    let (vr, vs) = (variance(&centroids(&raw, 60..450)), variance(&centroids(&smooth, 60..450)));
    assert!(vr > 0.05, "the raw line shows the tremor: {vr}");
    assert!(vs < vr * 0.2, "post correction left variance {vs} (raw {vr})");

    // The same level at 4× zoom smooths a quarter as far in document px.
    let mut doc = Document::new(512, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&arty_brush::BrushPreset { post_correction: 6, ..pen }, [0.1; 3]);
    engine.set_view_zoom(4.0);
    draw(&mut engine, &mut doc, &jittery());
    let vz = variance(&centroids(&doc, 60..450));
    assert!(vz > vs, "zoomed-in correction is gentler: {vz} vs {vs}");
}

#[test]
fn post_correction_keeps_endpoints() {
    let path = jittery();
    let (first, last) = (path[0], path[path.len() - 1]);
    for level in [3, 6, 10] {
        let pen = arty_brush::BrushPreset { size: 6.0, ..shaped("G-Pen", 0.0, 0.0, level) };
        let (doc, engine) = paint(&pen, &path);
        assert_eq!(engine.last_reshape(), Reshape::Full);
        for (x, y, _) in [first, last] {
            let a = alpha_at(&doc, x.floor() as i32, y.floor() as i32);
            assert!(a > 16384, "level {level}: endpoint ({x}, {y}) lost its ink ({a})");
        }
        // Nothing reaches past the ends.
        assert_eq!(peak_alpha(&doc, first.0 as i32 - 6, 128), 0);
        assert_eq!(peak_alpha(&doc, last.0 as i32 + 6, 128), 0);
    }
}

#[test]
fn reshape_skips_when_nothing_changes() {
    // Shaping off: the pre-shaping path.
    let (_, engine) = paint(&preset("G-Pen"), &wavy());
    assert_eq!(engine.last_reshape(), Reshape::Skipped);
    // Entry taper only is applied live; nothing to redo.
    let (_, engine) = paint(&shaped("G-Pen", 40.0, 0.0, 0), &wavy());
    assert_eq!(engine.last_reshape(), Reshape::Skipped);
    // Post correction of a line that is already straight changes nothing.
    let (_, engine) = paint(&shaped("G-Pen", 0.0, 0.0, 6), &straight(0.8));
    assert_eq!(engine.last_reshape(), Reshape::Skipped);

    // Taps keep their dot with every shaping option on.
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("Inking Pen"), [0.0; 3]);
    engine.begin(&mut doc, sample(100.0, 100.0, 0.3, 0.0)).unwrap();
    engine.feed(&mut doc, sample(100.0, 100.0, 0.8, 0.01));
    engine.feed(&mut doc, sample(100.0, 100.0, 0.0, 0.02));
    assert!(engine.end(&mut doc).is_some(), "a tap is an undoable edit");
    assert_eq!(engine.last_reshape(), Reshape::Skipped);
    assert!(peak_alpha(&doc, 100, 100) > 20000, "pen tap should leave a dot");
    assert_eq!(alpha_at(&doc, 100, 110), 0);
    // Mouse click.
    engine.begin(&mut doc, sample(50.0, 50.0, 1.0, 0.0)).unwrap();
    assert!(engine.end(&mut doc).is_some());
    assert_eq!(engine.last_reshape(), Reshape::Skipped);
    assert!(peak_alpha(&doc, 50, 50) > 5000, "click should leave a dot");
}

/// S19, skidding: a tap whose pen drifts a little between down and up still
/// leaves its dot with the Inking Pen (entry taper, exit taper, correction,
/// stabilizer at its default), instead of a faint tapered speck.
#[test]
fn skidding_tap_with_taper_leaves_a_dot() {
    let mut reference = None;
    for drift in [0.0f32, 0.3, 1.0, 2.0] {
        for zoom in [1.0, 0.5] {
            let mut doc = Document::new(256, 256, 350);
            let mut engine = StrokeEngine::new();
            engine.configure(&preset("Inking Pen"), [0.0; 3]);
            engine.set_view_zoom(zoom);
            engine.begin(&mut doc, sample(100.0, 100.0, 0.3, 0.0)).unwrap();
            engine.feed(&mut doc, sample(100.0 + drift * 0.5, 100.0, 0.8, 0.01));
            engine.feed(&mut doc, sample(100.0 + drift, 100.0 + drift * 0.3, 0.6, 0.02));
            engine.feed(&mut doc, sample(100.0 + drift, 100.0 + drift * 0.3, 0.0, 0.03));
            let edit = engine.end(&mut doc);
            assert!(edit.is_some(), "drift {drift}: a tap is an undoable edit");
            assert_eq!(engine.last_reshape(), Reshape::Skipped, "drift {drift}");
            let peak = peak_alpha(&doc, 100, 100);
            assert!(peak > 20000, "drift {drift} zoom {zoom}: the tap left a speck ({peak})");
            assert_eq!(alpha_at(&doc, 100, 110), 0);
            assert_eq!(alpha_at(&doc, 110, 100), 0);
            // The same dot wherever the pen skidded to: drawn at touch-down.
            let tiles: Vec<_> = doc.active_layer().raster().unwrap().iter().map(|(c, t)| (c, **t)).collect();
            match &reference {
                None => reference = Some(tiles),
                Some(r) => assert!(*r == tiles, "drift {drift} zoom {zoom}: a different dot"),
            }
            // One undo step removes it.
            let mut history = History::new(8);
            history.push(edit.unwrap());
            history.undo(&mut doc);
            assert!(doc.active_layer().raster().unwrap().is_empty(), "drift {drift}");
        }
    }

    // A real (short) stroke longer than the dot keeps its taper.
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(&preset("Inking Pen"), [0.0; 3]);
    engine.begin(&mut doc, sample(100.0, 100.0, 0.8, 0.0)).unwrap();
    for i in 1..=10 {
        engine.feed(&mut doc, sample(100.0 + i as f32 * 2.0, 100.0, 0.8, i as f64 * 0.01));
    }
    engine.feed(&mut doc, sample(120.0, 100.0, 0.0, 0.11));
    engine.end(&mut doc);
    assert_ne!(engine.last_reshape(), Reshape::Skipped);
    assert!(peak_alpha(&doc, 100, 100) < 20000, "a 20 px stroke became a tap");
}

#[test]
fn huge_brush_degrades() {
    // A 2000 px brush: each dab covers ~4 M px, so repainting the stroke
    // whole (or even its end, which every dab reaches) would stall pen-up.
    let path: Vec<_> = (0..=40).map(|i| (50.0 + i as f32 * 10.0, 128.0 + 30.0 * (i as f32 * 0.7).sin(), 1.0)).collect();
    let huge = |tout, corr| arty_brush::BrushPreset { size: 2000.0, density: 12.0, ..shaped("G-Pen", 0.0, tout, corr) };
    // With an exit taper it still gets one, clipped to the end.
    let (_, engine) = paint(&huge(80.0, 3), &path);
    let r = engine.last_reshape();
    assert!(matches!(r, Reshape::Tail { .. } | Reshape::TooLong), "{r:?}");
    // Correction alone cannot be clipped: kept as drawn.
    let (_, engine) = paint(&huge(0.0, 3), &path);
    assert_eq!(engine.last_reshape(), Reshape::TooLong);
}

#[test]
fn shaped_lock_alpha_keeps_alpha() {
    let mut doc = Document::new(256, 256, 350);
    let mut engine = StrokeEngine::new();
    let shape = arty_brush::BrushPreset { size: 40.0, min_size: 1.0, hardness: 0.1, ..preset("Flat Color") };
    engine.configure(&shape, [1.0, 0.0, 0.0]);
    line_at(&mut engine, &mut doc, 128.0, 1.0);
    let before: Vec<[u16; 4]> = (0..256 * 256).map(|i| pixel_at(&doc, i % 256, i / 256)).collect();
    lock_alpha(&mut doc);

    let path: Vec<_> = (0..=60).map(|i| (60.0 + 140.0 * i as f32 / 60.0, 60.0 + 136.0 * i as f32 / 60.0, 0.9)).collect();
    for (tout, corr) in [(80.0, 0), (80.0, 4)] {
        let pen = arty_brush::BrushPreset { size: 12.0, ..shaped("G-Pen", 30.0, tout, corr) };
        engine.configure(&pen, [0.0, 0.0, 1.0]);
        draw(&mut engine, &mut doc, &path);
        assert_ne!(engine.last_reshape(), Reshape::Skipped);
    }
    let mut blue = 0;
    for (i, b) in before.iter().enumerate() {
        let (x, y) = ((i % 256) as i32, (i / 256) as i32);
        let p = pixel_at(&doc, x, y);
        assert_eq!(p[3], b[3], "alpha changed at ({x}, {y})");
        blue += (p[2] > b[2]) as usize;
    }
    assert!(blue > 100, "the pen painted onto the locked shape");
}

#[test]
fn preview_shows_taper() {
    let inking = preset("Inking Pen");
    let flat = arty_brush::BrushPreset { taper_in: 0.0, taper_out: 0.0, post_correction: 0, ..inking.clone() };
    let (w, h) = (192, 48);
    let a = arty_brush::render_preview(&inking, w, h, [0.0; 3]);
    let b = arty_brush::render_preview(&flat, w, h, [0.0; 3]);
    let ink = |v: &[u8]| v.chunks(4).map(|p| p[3] as u32).sum::<u32>();
    assert!(ink(&a) > 0 && ink(&a) < ink(&b), "tapered preview should hold less ink: {} vs {}", ink(&a), ink(&b));
}

/// A scribble that stays within a tap's reach of its start but travels a
/// long path (building up coverage in place) is a stroke, not a tap: it
/// must not collapse into the single touch-down dot.
#[test]
fn scribble_in_place_is_not_a_tap() {
    let mut p = shaped("G-Pen", 40.0, 80.0, 0);
    p.size = 60.0;
    p.opacity = 0.3;
    let run = |scribble: bool| {
        let mut doc = Document::new(256, 256, 350);
        let mut engine = StrokeEngine::new();
        engine.configure(&p, [0.0; 3]);
        engine.begin(&mut doc, sample(128.0, 128.0, 0.8, 0.0)).unwrap();
        let n = if scribble { 120 } else { 2 };
        for i in 1..=n {
            let a = i as f32 * 0.3;
            let (dx, dy) = if scribble { (4.0 * a.cos() - 4.0, 4.0 * a.sin()) } else { (0.0, 0.0) };
            engine.feed(&mut doc, sample(128.0 + dx, 128.0 + dy, 0.8, i as f64 * 0.005));
        }
        engine.feed(&mut doc, sample(128.0, 128.0, 0.0, (n + 1) as f64 * 0.005));
        engine.end(&mut doc);
        // Total ink (alpha) on the layer.
        doc.active_layer().raster().unwrap().iter().flat_map(|(_, t)| t.as_flattened().iter().map(|p| p[3] as u64)).sum::<u64>()
    };
    let dot = run(false);
    assert!(dot > 0);
    let scribble = run(true);
    // 30% dabs looping in place build up far more ink than one dot.
    assert!(scribble > dot * 2, "the scribble collapsed into the tap dot ({scribble} vs {dot})");
}
