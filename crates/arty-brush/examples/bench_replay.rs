//! B004, B022 & B023: cost of pen-up stroke reshaping (exit taper / post correction).
//!
//! Measures pen-up reshape cost for typical strokes (including the in-app
//! bench stroke: Inking Pen 8 px, 2 s contacts on an A4 350 dpi page)
//! comparing:
//! 1. 100 ms baseline (synchronous full replay)
//! 2. 16 ms adaptive budget (synchronous tail replay fallback)
//! 3. Zero-Wait Pen-Up (speculative prefix replay on worker thread)
//!
//! Reports min, median (p50), p99 of end() ms, worker CPU time per stroke,
//! and detailed timing breakdown.
//!
//! ```text
//! cargo run --release -p arty-brush --example bench_replay [inapp] [survey] [paced]
//! ```
//!
//! With no argument every section runs. `paced` feeds samples on a 240 Hz
//! schedule (deadline pacing, as a pen does), so the worker runs as in the app.

use std::f64::consts::TAU;
use std::time::{Duration, Instant};

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

/// How samples are fed between pen-down and pen-up.
#[derive(Clone, Copy)]
enum Pace {
    /// Fixed sleep before every sample (B023 tables).
    Sleep(u64),
    /// Real pen rate: sample i is fed at i / Hz after pen-down.
    Hz(f64),
}

#[allow(dead_code)]
#[derive(Clone)]
struct StrokeStat {
    live_ms: f64,
    end_ms: f64,
    worker_cpu_ms: f64,
    dabs: u64,
    px: u64,
    reshape: Reshape,
    breakdown: EndBreakdown,
}

#[allow(dead_code)]
struct AggregatedRow {
    label: String,
    mode: &'static str,
    budget_label: &'static str,
    len: f32,
    samples: usize,
    end_min_ms: f64,
    end_p50_ms: f64,
    end_p99_ms: f64,
    worker_cpu_ms: f64,
    dabs: u64,
    px: u64,
    reshape: Reshape,
    breakdown: EndBreakdown,
}

fn measure_stroke_once(
    p: &BrushPreset,
    speculative: bool,
    budget_ms: Option<f64>,
    pts: &[InputSample],
    doc_w: u32,
    doc_h: u32,
    pace: Pace,
) -> StrokeStat {
    let mut doc = Document::new(doc_w, doc_h, 350);
    let mut engine = StrokeEngine::new();
    engine.configure(p, [0.1, 0.1, 0.1]);
    engine.set_speculative_replay(speculative);
    if let Some(b) = budget_ms {
        engine.set_replay_budget(b);
    }
    let t = Instant::now();
    engine.begin(&mut doc, pts[0]).expect("raster layer");
    for (i, s) in pts[1..].iter().enumerate() {
        match pace {
            Pace::Sleep(us) => std::thread::sleep(Duration::from_micros(us)),
            Pace::Hz(hz) => {
                // Deadline pacing: sample i+1 is due (i+1)/hz after pen-down.
                let due = t + Duration::from_secs_f64((i + 1) as f64 / hz);
                let now = Instant::now();
                if due > now {
                    std::thread::sleep(due - now);
                }
            }
        }
        engine.feed(&mut doc, *s);
    }
    let live_ms = t.elapsed().as_secs_f64() * 1e3;
    let live = engine.dab_stats();
    let t = Instant::now();
    let edit = engine.end(&mut doc);
    let end_ms = t.elapsed().as_secs_f64() * 1e3;
    let worker_cpu_ms = engine.worker_cpu_time_us() as f64 / 1000.0;
    drop(edit);

    StrokeStat {
        live_ms,
        end_ms,
        worker_cpu_ms,
        dabs: live.dabs,
        px: live.px,
        reshape: engine.last_reshape(),
        breakdown: engine.last_breakdown(),
    }
}

fn run_stroke_multi(
    label: &str,
    p: &BrushPreset,
    mode: &'static str,
    speculative: bool,
    budget_ms: Option<f64>,
    budget_label: &'static str,
    pts: &[InputSample],
    doc_w: u32,
    doc_h: u32,
    pace: Pace,
    repeats: usize,
) -> AggregatedRow {
    let mut runs = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        runs.push(measure_stroke_once(p, speculative, budget_ms, pts, doc_w, doc_h, pace));
    }
    runs.sort_by(|a, b| a.end_ms.partial_cmp(&b.end_ms).unwrap());
    let end_min_ms = runs[0].end_ms;
    let end_p50_ms = runs[runs.len() / 2].end_ms;
    let end_p99_ms = runs[((runs.len() as f64 * 0.99).ceil() as usize).min(runs.len()) - 1].end_ms;

    let med = &runs[runs.len() / 2];
    let mut len = 0.0f32;
    for w in pts.windows(2) {
        len += (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
    }

    AggregatedRow {
        label: label.into(),
        mode,
        budget_label,
        len,
        samples: pts.len(),
        end_min_ms,
        end_p50_ms,
        end_p99_ms,
        worker_cpu_ms: med.worker_cpu_ms,
        dabs: med.dabs,
        px: med.px,
        reshape: med.reshape,
        breakdown: med.breakdown,
    }
}

fn preset(name: &str) -> BrushPreset {
    default_presets().into_iter().find(|p| p.name == name).expect("default preset")
}

fn main() {
    println!("=== ARTY Pen-Up Reshape Bench (B004 / B022 / B023) ===");
    println!("Calibrated startup rate: {:.3} ns/px", arty_brush::engine::startup_rate());
    println!();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let run = |section: &str| args.is_empty() || args.iter().any(|a| a == section);

    let inking = preset("Inking Pen");
    let doc_side = 1009.0 * (A4_H as f32 / 929.0);
    let in_app_pts = in_app_bench_path(A4_W as f32 / 2.0, A4_H as f32 / 2.0, doc_side);

    // Pace simulating real 2-second in-app pen drawing (480 samples over 2000 ms = ~4.16 ms per sample)
    let in_app_pace_us = 4166;

    // 1. In-app bench stroke profile (A4 350 dpi, Inking Pen 8 px, 2 s contact):
    if run("inapp") {
        println!("### Typical in-app bench stroke: Inking Pen 8 px, 2 s contact on A4 350 dpi (5 repeats interleaved)");
        println!("| Mode | Budget | Reshape | end() p50 (ms) | end() p99 (ms) | end() min (ms) | Worker CPU (ms) | px (M) | rep ms | rest ms |");
        println!("|---|---|---|---:|---:|---:|---:|---:|---:|---:|");

        let modes = [
            ("Baseline (sync)", false, Some(100.0), "100 ms baseline"),
            ("Adaptive E21", false, Some(16.0), "16 ms adaptive"),
            ("Zero-Wait B023", true, None, "Zero-Wait"),
        ];

        for (name, spec, bud, lbl) in modes {
            let r = run_stroke_multi("Inking Pen 8 (A4)", &inking, name, spec, bud, lbl, &in_app_pts, A4_W, A4_H, Pace::Sleep(in_app_pace_us), 5);
            let b = &r.breakdown;
            println!(
                "| {} | {} | {:?} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |",
                r.mode,
                r.budget_label,
                r.reshape,
                r.end_p50_ms,
                r.end_p99_ms,
                r.end_min_ms,
                r.worker_cpu_ms,
                r.px as f64 / 1e6,
                b.replay_us as f64 / 1e3,
                b.restore_us as f64 / 1e3,
            );
        }
        println!();
    }

    let sized = |name: &str, size: f32| BrushPreset { size, stabilizer: 0, ..preset(name) };
    // 2. Real pen rate: samples on a 240 Hz schedule, so the worker keeps up as in the app.
    if run("paced") {
        println!("### 240 Hz paced strokes (deadline pacing; 5 repeats in-app, 3 otherwise)");
        println!("| Stroke | Samples | Mode | Reshape | end() p50 ms | end() p99 ms | end() min ms | Worker CPU ms | rep ms |");
        println!("|---|---:|---|---|---:|---:|---:|---:|---:|");
        let g8 = sized("G-Pen", 8.0);
        let corrected = BrushPreset { taper_out: 80.0, post_correction: 3, ..g8.clone() };
        let taper_only = BrushPreset { taper_out: 80.0, post_correction: 0, ..g8 };
        let (p2k, p10k) = (path(2_000.0), path(10_000.0));
        let cases = [
            ("Inking Pen 8 (A4, in-app)", &inking, in_app_pts.as_slice(), A4_W, A4_H, 5),
            ("G-Pen 8 taper+corr 2k px", &corrected, p2k.as_slice(), PAGE, PAGE, 3),
            ("G-Pen 8 taper+corr 10k px", &corrected, p10k.as_slice(), PAGE, PAGE, 3),
            ("G-Pen 8 taper only 2k px", &taper_only, p2k.as_slice(), PAGE, PAGE, 3),
        ];
        for (label, p, pts, w, h, repeats) in cases {
            for (m_name, spec) in [("Baseline (sync)", false), ("Zero-Wait", true)] {
                let r = run_stroke_multi(label, p, m_name, spec, Some(100.0), "100ms", pts, w, h, Pace::Hz(STROKE_HZ), repeats);
                println!(
                    "| {} | {} | {} | {:?} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |",
                    r.label,
                    r.samples,
                    r.mode,
                    r.reshape,
                    r.end_p50_ms,
                    r.end_p99_ms,
                    r.end_min_ms,
                    r.worker_cpu_ms,
                    r.breakdown.replay_us as f64 / 1e3,
                );
            }
        }
        println!();
    }

    // 3. Full survey across presets and lengths (4096x4096 page):
    if !run("survey") {
        return;
    }
    let brushes = [
        ("G-Pen 8", sized("G-Pen", 8.0)),
        ("G-Pen 30", sized("G-Pen", 30.0)),
        ("Brush 24", sized("Brush", 24.0)),
        ("Airbrush 120", sized("Airbrush", 120.0)),
        ("G-Pen 500", sized("G-Pen", 500.0)),
    ];

    println!("### Suite survey across stroke lengths (4096×4096 page, interleaved repeats)");
    println!("| Preset | Length px | Mode | end() p50 ms | end() p99 ms | Worker CPU ms | Reshape | px (M) |");
    println!("|---|---:|---|---:|---:|---:|---|---:|");

    for (label, p) in &brushes {
        for len in [2_000.0, 10_000.0, 40_000.0] {
            let pts = path(len);
            let full = BrushPreset { taper_out: 80.0, post_correction: 3, ..p.clone() };

            for (m_name, spec, bud, lbl) in [
                ("Baseline", false, Some(100.0), "100ms"),
                ("Adaptive E21", false, Some(16.0), "16ms"),
                ("Zero-Wait", true, None, "ZeroWait"),
            ] {
                // Short pacing (200 us) to test concurrent worker pipeline
                let r = run_stroke_multi(label, &full, m_name, spec, bud, lbl, &pts, PAGE, PAGE, Pace::Sleep(200), 3);
                println!(
                    "| {} | {:.0} | {} | {:.2} | {:.2} | {:.2} | {:?} | {:.2} |",
                    r.label,
                    r.len,
                    r.mode,
                    r.end_p50_ms,
                    r.end_p99_ms,
                    r.worker_cpu_ms,
                    r.reshape,
                    r.px as f64 / 1e6,
                );
            }
        }
    }
}
