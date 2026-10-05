//! B004: cost of pen-up stroke reshaping (exit taper / post correction).
//!
//! Paints long strokes with several presets and measures the live feed time
//! against the `end()` time in Tail and Full modes, plus ns per dab pixel
//! and µs per dab, which calibrate `shape::MAX_FULL_REPLAY_PX`.
//!
//! ```text
//! cargo run --release -p arty-brush --example bench_replay
//! ```

use std::time::Instant;

use arty_brush::{BrushPreset, InputSample, Reshape, StrokeEngine, default_presets};
use arty_core::Document;

const PAGE: u32 = 4096;
/// Sample spacing along the path (px); 200 Hz.
const STEP: f32 = 4.0;

/// A boustrophedon path of `len` px over the page with a small wobble, so
/// post correction has something to smooth.
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

struct Row {
    label: String,
    mode: &'static str,
    len: f32,
    samples: usize,
    live_ms: f64,
    end_ms: f64,
    dabs: u64,
    px: u64,
    reshape: Reshape,
}

fn run(label: &str, p: &BrushPreset, mode: &'static str, len: f32) -> Row {
    let pts = path(len);
    let mut doc = Document::new(PAGE, PAGE, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(p, [0.1, 0.1, 0.1]);
    let t = Instant::now();
    engine.begin(&mut doc, pts[0]).expect("raster layer");
    for s in &pts[1..] {
        engine.feed(&mut doc, *s);
    }
    let live_ms = t.elapsed().as_secs_f64() * 1e3;
    let live = engine.dab_stats();
    let t = Instant::now();
    let edit = engine.end(&mut doc);
    let end_ms = t.elapsed().as_secs_f64() * 1e3;
    drop(edit);
    Row {
        label: label.into(),
        mode,
        len,
        samples: pts.len(),
        live_ms,
        end_ms,
        dabs: live.dabs,
        px: live.px,
        reshape: engine.last_reshape(),
    }
}

fn preset(name: &str) -> BrushPreset {
    default_presets().into_iter().find(|p| p.name == name).expect("default preset")
}

fn main() {
    let sized = |name: &str, size: f32| BrushPreset { size, stabilizer: 0, ..preset(name) };
    let brushes = [
        ("G-Pen 8", sized("G-Pen", 8.0)),
        ("G-Pen 30", sized("G-Pen", 30.0)),
        ("Brush 24", sized("Brush", 24.0)),
        ("Airbrush 120", sized("Airbrush", 120.0)),
        ("G-Pen 500", sized("G-Pen", 500.0)),
        ("G-Pen 2000", sized("G-Pen", 2000.0)),
    ];
    let mut rows = Vec::new();
    for (label, p) in &brushes {
        for len in [2_000.0, 10_000.0, 40_000.0] {
            // Tail: exit taper only (Brush smudges, so it always gets Full).
            let tail = BrushPreset { taper_out: 80.0, post_correction: 0, ..p.clone() };
            rows.push(run(label, &tail, "taper", len));
            // Full: post correction repaints everything.
            let full = BrushPreset { taper_out: 80.0, post_correction: 3, ..p.clone() };
            rows.push(run(label, &full, "taper+corr", len));
        }
    }

    println!("| brush | shaping | length px | samples | live ms | end() ms | reshape | dabs | dab px (M) | live ns/px | live µs/dab |");
    println!("|---|---|---:|---:|---:|---:|---|---:|---:|---:|---:|");
    for r in &rows {
        println!(
            "| {} | {} | {} | {} | {:.1} | {:.1} | {:?} | {} | {:.1} | {:.2} | {:.2} |",
            r.label,
            r.mode,
            r.len,
            r.samples,
            r.live_ms,
            r.end_ms,
            r.reshape,
            r.dabs,
            r.px as f64 / 1e6,
            r.live_ms * 1e6 / r.px.max(1) as f64,
            r.live_ms * 1e3 / r.dabs.max(1) as f64,
        );
    }
    // Calibration: Full replays repaint the whole stroke, so their end()
    // time over the live dab pixels is the replay cost per pixel.
    let full: Vec<_> = rows.iter().filter(|r| r.reshape == Reshape::Full && r.px > 1_000_000).collect();
    let worst = full.iter().map(|r| r.end_ms * 1e6 / r.px as f64).fold(0.0f64, f64::max);
    let worst_live = rows.iter().filter(|r| r.px > 1_000_000).map(|r| r.live_ms * 1e6 / r.px as f64).fold(0.0f64, f64::max);
    println!();
    println!("worst Full end() ns per live dab px: {worst:.3}");
    println!("worst live ns per dab px: {worst_live:.3}");
    let ns = worst.max(worst_live);
    if ns > 0.0 {
        println!("MAX_FULL_REPLAY_PX for 100 ms: {:.0}", 100e6 / ns);
    }
}
