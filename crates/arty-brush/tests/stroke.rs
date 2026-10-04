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
