//! B006: cost of painting through a selection (plans/m3_page_tools.md §7).
//!
//! Paints the same long strokes with no selection, with everything selected
//! (every tile full: the per-tile lookup only), with every tile partly
//! selected (the per-pixel mask lookup on every dab pixel) and through a
//! soft-edged lasso, and reports live time (`begin` + every `feed`) per dab
//! pixel against the unmasked stroke. Minimum of several runs, each on a
//! fresh page.
//!
//! ```text
//! cargo run --release -p arty-brush --example bench_masked
//! ```

use std::sync::Arc;
use std::time::Instant;

use arty_brush::{BrushPreset, InputSample, StrokeEngine, default_presets};
use arty_core::{Document, Selection, TILE_SIZE, TileCoord};

const PAGE: u32 = 4096;
const STEP: f32 = 4.0;
const RUNS: usize = 11;

/// A boustrophedon path of `len` px (rows 90 px apart), as in B004.
fn path(len: f32) -> Vec<InputSample> {
    let (x0, x1) = (200.0f32, PAGE as f32 - 200.0);
    let mut out = Vec::new();
    let (mut s, mut row) = (0.0f32, 0u32);
    let mut i = 0usize;
    while s < len {
        let y = 200.0 + row as f32 * 90.0;
        let mut x = if row % 2 == 0 { x0 } else { x1 };
        let dir = if row % 2 == 0 { 1.0 } else { -1.0 };
        while (x0..=x1).contains(&x) && s < len {
            out.push(InputSample {
                x,
                y: y + 2.0 * (x / 7.0).sin(),
                pressure: 0.8 + 0.2 * (s / 300.0).sin(),
                time: i as f64 * 0.005,
                ..Default::default()
            });
            x += dir * STEP;
            s += STEP;
            i += 1;
        }
        row += 1;
    }
    out
}

fn uniform(v: u8) -> Selection {
    let n = PAGE.div_ceil(TILE_SIZE as u32) as i32;
    let m = Arc::new([[v; TILE_SIZE]; TILE_SIZE]);
    let mut s = Selection::new();
    for ty in 0..n {
        for tx in 0..n {
            s.insert_tile(TileCoord::new(tx, ty), m.clone());
        }
    }
    s
}

fn lasso() -> Selection {
    let pts: Vec<[f32; 2]> = (0..4000)
        .map(|i| {
            let t = i as f32 / 4000.0 * std::f32::consts::TAU;
            let k = 1.0 + 0.1 * (7.0 * t).sin();
            [2048.0 + 1700.0 * k * t.cos(), 2048.0 + 1500.0 * k * t.sin()]
        })
        .collect();
    let s = arty_core::raster::rasterize_polygon(&pts, PAGE, PAGE, true);
    arty_core::morph::feather(&s, 8, PAGE, PAGE)
}

/// Live ms and dab px of one stroke on a fresh page.
fn run(p: &BrushPreset, pts: &[InputSample], sel: &Selection) -> (f64, u64) {
    let mut doc = Document::new(PAGE, PAGE, 350);
    doc.swap_selection(sel.clone());
    let mut engine = StrokeEngine::new();
    engine.configure(p, [0.1, 0.1, 0.1]);
    let t = Instant::now();
    engine.begin(&mut doc, pts[0]).expect("raster layer");
    for s in &pts[1..] {
        engine.feed(&mut doc, *s);
    }
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let px = engine.dab_stats().px;
    drop(engine.end(&mut doc));
    (ms, px)
}

fn main() {
    let presets = default_presets();
    let pick = |name: &str, size: f32| BrushPreset {
        size,
        stabilizer: 0,
        taper_in: 0.0,
        taper_out: 0.0,
        post_correction: 0,
        ..presets.iter().find(|p| p.name == name).unwrap().clone()
    };
    let cases = [("G-Pen", 8.0, 20_000.0), ("G-Pen", 30.0, 10_000.0), ("Brush", 24.0, 10_000.0), ("Airbrush", 120.0, 4_000.0)];
    let sels = [
        ("none", Selection::new()),
        ("all selected (full tiles)", uniform(255)),
        ("all tiles partial (m = 200)", uniform(200)),
        ("soft lasso", lasso()),
    ];
    println!("| brush | selection | live ms | dab px (M) | ns / dab px | vs none |");
    println!("|---|---|---:|---:|---:|---:|");
    for (name, size, len) in cases {
        let p = pick(name, size);
        let pts = path(len);
        // Interleaved, so load from other processes hits every case alike;
        // the fastest run of each is kept.
        let mut best = vec![(f64::MAX, 0u64); sels.len()];
        for _ in 0..RUNS {
            for (b, (_, sel)) in best.iter_mut().zip(&sels) {
                let r = run(&p, &pts, sel);
                if r.0 < b.0 {
                    *b = r;
                }
            }
        }
        let mut base = 0.0;
        for ((label, _), &(ms, px)) in sels.iter().zip(&best) {
            let ns = ms * 1e6 / px.max(1) as f64;
            if label == &"none" {
                base = ns;
            }
            println!(
                "| {name} {size} | {label} | {ms:.1} | {:.2} | {ns:.2} | {:+.1} % |",
                px as f64 / 1e6,
                (ns / base - 1.0) * 100.0
            );
        }
    }
}
