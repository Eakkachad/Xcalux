use arty_brush::{InputSample, StrokeEngine, StrokeRefused, default_presets};
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
    for name in ["G-Pen", "Mapping Pen", "Brush", "Pencil", "Airbrush", "Hard Eraser"] {
        engine.configure(&preset(name), [0.2, 0.3, 0.4]);
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
