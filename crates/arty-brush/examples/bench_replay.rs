//! B004 & B022: cost of pen-up stroke reshaping (exit taper / post correction).
//!
//! Measures pen-up reshape cost for typical strokes (including the in-app
//! bench stroke: Inking Pen 8 px, 2 s contacts on an A4 350 dpi page)
//! comparing 100 ms baseline against 16 ms adaptive budget, with detailed
//! timing breakdown of end() (drain, post correction, clip/tail cost, restore,
//! dab replay, recording finish).
//!
//! ```text
//! cargo run --release -p arty-brush --example bench_replay
//! ```

use std::f64::consts::TAU;
use std::time::Instant;

use arty_brush::{BrushPreset, EndBreakdown, InputSample, Reshape, StrokeEngine, default_presets};
use arty_core::Document;

const PAGE: u32 = 4096;
const A4_W: u32 = 2976;
const A4_H: u32 = 4175;

/// Sample spacing along the path (px); 200 Hz.
const STEP: f32 = 4.0;

/// Boustrophedon path of `len` px over the page with a small wobble.
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

const STROKE_HZ: f64 = 240.0;
const CONTACT: u64 = 480;
const RAMP: u64 = 12;
const LOOP_SECS: f64 = 1.7;
const RADIUS: f32 = 0.18;

/// In-app bench path: 2 s contact (480 samples at 240 Hz) on an A4 page.
fn in_app_bench_path(cx: f32, cy: f32, side: f32) -> Vec<InputSample> {
    let radius = RADIUS * side;
    let mut out = Vec::with_capacity(CONTACT as usize);
    for k in 0..CONTACT {
        let t = k as f64 / STROKE_HZ;
        let theta = TAU * t / LOOP_SECS;
        let r = 0.8 + 0.2 * (3.0 * theta + 0.7 * t).sin();
        let drift = [0.3 * (0.21 * t).sin(), 0.3 * (0.17 * t).cos()];
        let offset = [(r * theta.cos() + drift[0]) as f32, (0.8 * r * theta.sin() + drift[1]) as f32];
        let ramp = ((k + 1).min(CONTACT.saturating_sub(k)) as f64 / RAMP as f64).min(1.0);
        let pressure = ((0.55 + 0.35 * (TAU * 0.9 * t).sin()) * ramp).clamp(0.02, 1.0) as f32;
        out.push(InputSample {
            x: cx + radius * offset[0],
            y: cy + radius * offset[1],
            pressure,
            time: t,
            ..Default::default()
        });
    }
    out
}

#[allow(dead_code)]
struct Row {
    label: String,
    mode: &'static str,
    budget_label: &'static str,
    len: f32,
    samples: usize,
    live_ms: f64,
    end_ms: f64,
    dabs: u64,
    px: u64,
    reshape: Reshape,
    breakdown: EndBreakdown,
}

fn run_stroke(label: &str, p: &BrushPreset, mode: &'static str, budget_ms: f64, budget_label: &'static str, pts: &[InputSample], doc_w: u32, doc_h: u32) -> Row {
    let mut doc = Document::new(doc_w, doc_h, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(p, [0.1, 0.1, 0.1]);
    engine.set_replay_budget(budget_ms);
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
    let mut len = 0.0f32;
    for w in pts.windows(2) {
        len += (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
    }
    Row {
        label: label.into(),
        mode,
        budget_label,
        len,
        samples: pts.len(),
        live_ms,
        end_ms,
        dabs: live.dabs,
        px: live.px,
        reshape: engine.last_reshape(),
        breakdown: engine.last_breakdown(),
    }
}

fn preset(name: &str) -> BrushPreset {
    default_presets().into_iter().find(|p| p.name == name).expect("default preset")
}

fn main() {
    println!("=== ARTY Pen-Up Reshape Bench (B004 / B022) ===");
    println!("Calibrated startup rate: {:.3} ns/px", arty_brush::engine::startup_rate());
    println!();

    let inking = preset("Inking Pen");
    // In-app bench window 1920x1009 has ~929 px canvas viewport height fitting the 4175 px A4 page (zoom ~0.222).
    let doc_side = 1009.0 * (A4_H as f32 / 929.0);
    let in_app_pts = in_app_bench_path(A4_W as f32 / 2.0, A4_H as f32 / 2.0, doc_side);

    // 1. In-app bench stroke profile (A4 350 dpi, Inking Pen 8 px, 2 s contact):
    println!("### Typical in-app bench stroke: Inking Pen 8 px, 2 s contact on A4 350 dpi");
    let in_app_100ms = run_stroke("Inking Pen 8 (A4)", &inking, "shipped (taper+corr)", 100.0, "100 ms baseline", &in_app_pts, A4_W, A4_H);
    let in_app_16ms = run_stroke("Inking Pen 8 (A4)", &inking, "shipped (taper+corr)", 16.0, "16 ms adaptive", &in_app_pts, A4_W, A4_H);

    println!("| budget | live ms | end() ms | reshape | dabs | px (M) | drain ms | corr ms | clip ms | rest ms | rep ms | fin ms |");
    println!("|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for r in [&in_app_100ms, &in_app_16ms] {
        let b = &r.breakdown;
        println!(
            "| {} | {:.1} | {:.2} | {:?} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |",
            r.budget_label,
            r.live_ms,
            r.end_ms,
            r.reshape,
            r.dabs,
            r.px as f64 / 1e6,
            b.drain_us as f64 / 1e3,
            b.correct_us as f64 / 1e3,
            b.clip_cost_us as f64 / 1e3,
            b.restore_us as f64 / 1e3,
            b.replay_us as f64 / 1e3,
            b.finish_us as f64 / 1e3,
        );
    }
    println!();

    // 2. Full survey across presets and lengths (B004 paths):
    let sized = |name: &str, size: f32| BrushPreset { size, stabilizer: 0, ..preset(name) };
    let brushes = [
        ("G-Pen 8", sized("G-Pen", 8.0)),
        ("G-Pen 30", sized("G-Pen", 30.0)),
        ("Brush 24", sized("Brush", 24.0)),
        ("Airbrush 120", sized("Airbrush", 120.0)),
        ("G-Pen 500", sized("G-Pen", 500.0)),
        ("G-Pen 2000", sized("G-Pen", 2000.0)),
    ];

    println!("### Suite survey across stroke lengths (4096×4096 page)");
    println!("| brush | shaping | budget | length px | live ms | end() ms | reshape | dab px (M) | rep ms | rest ms |");
    println!("|---|---|---|---:|---:|---:|---|---:|---:|---:|");
    for (label, p) in &brushes {
        for len in [2_000.0, 10_000.0, 40_000.0] {
            let pts = path(len);
            let full = BrushPreset { taper_out: 80.0, post_correction: 3, ..p.clone() };
            // Run baseline 100 ms and adaptive 16 ms:
            for (b_ms, b_lbl) in [(100.0, "100ms"), (16.0, "16ms")] {
                let r = run_stroke(label, &full, "taper+corr", b_ms, b_lbl, &pts, PAGE, PAGE);
                let b = &r.breakdown;
                println!(
                    "| {} | {} | {} | {:.0} | {:.1} | {:.1} | {:?} | {:.2} | {:.1} | {:.2} |",
                    r.label,
                    r.mode,
                    r.budget_label,
                    r.len,
                    r.live_ms,
                    r.end_ms,
                    r.reshape,
                    r.px as f64 / 1e6,
                    b.replay_us as f64 / 1e3,
                    b.restore_us as f64 / 1e3,
                );
            }
        }
    }
}
