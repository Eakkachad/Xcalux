//! Selection core timings on a B4 600 dpi page (plans/m3_page_tools.md §7,
//! write-up plans/bench/B006_selection_core.md).
//!
//! cargo run -p arty-core --release --example bench_select
//!
//! Each case runs on rayon pools of 1, 8 and all threads and reports the
//! fastest and the median of several runs.

use std::sync::Arc;
use std::time::Instant;

use arty_core::morph::{self, MorphShape};
use arty_core::raster;
use arty_core::{MaskPixels, Pt, SelectOp, Selection, TILE_SIZE, TileCoord};

const W: u32 = 6071;
const H: u32 = 8598;

/// Fastest and median of `runs` calls of `f`, each returning its own time
/// in ms. The minimum is the figure to quote on a busy machine.
fn min_median_ms(runs: usize, f: impl Fn() -> f64) -> (f64, f64) {
    let mut t: Vec<f64> = (0..runs).map(|_| f()).collect();
    t.sort_by(f64::total_cmp);
    (t[0], t[t.len() / 2])
}

/// Time of `f`, in ms; its result is dropped outside the timing.
fn time<R>(f: impl FnOnce() -> R) -> f64 {
    let s = Instant::now();
    let r = std::hint::black_box(f());
    let ms = s.elapsed().as_secs_f64() * 1e3;
    drop(r);
    ms
}

/// A hand-drawn-looking closed lasso: a wobbly loop, one point every ~3 px.
fn lasso(cx: f32, cy: f32, rx: f32, ry: f32, wobble: f32) -> Vec<Pt> {
    let n = ((rx + ry) * std::f32::consts::PI / 3.0) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            let k = 1.0 + wobble * (0.6 * (5.0 * t).sin() + 0.3 * (13.0 * t + 1.0).cos() + 0.1 * (41.0 * t).sin());
            [cx + rx * k * t.cos(), cy + ry * k * t.sin()]
        })
        .collect()
}

fn perimeter(p: &[Pt]) -> f64 {
    (0..p.len()).map(|i| ((p[(i + 1) % p.len()][0] - p[i][0]) as f64).hypot((p[(i + 1) % p.len()][1] - p[i][1]) as f64)).sum()
}

/// Screentone-like dots everywhere: every tile partial, so the ROI of any
/// morphology op is the whole page (the "full-page boundary" case).
fn tone() -> Selection {
    let mut s = Selection::new();
    let (tw, th) = (W.div_ceil(64) as i32, H.div_ceil(64) as i32);
    let mut m: MaskPixels = [[0; TILE_SIZE]; TILE_SIZE];
    for (y, row) in m.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            // 16 px dot pitch, radius 5, 1 px soft edge.
            let (dx, dy) = ((x % 16) as f32 - 7.5, (y % 16) as f32 - 7.5);
            *v = ((5.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    let m = Arc::new(m);
    for ty in 0..th {
        for tx in 0..tw {
            s.insert_tile(TileCoord::new(tx, ty), m.clone());
        }
    }
    s
}

fn main() {
    let threads = [1usize, 8, std::thread::available_parallelism().map_or(8, |n| n.get())];
    println!("B4 600 dpi: {W} × {H} px, {} tiles; logical cores {}", W.div_ceil(64) * H.div_ceil(64), threads[2]);

    let typical = lasso(3000.0, 4300.0, 2200.0, 3000.0, 0.15);
    let other = lasso(3600.0, 3600.0, 1800.0, 2000.0, 0.2);
    let page = lasso(W as f32 / 2.0, H as f32 / 2.0, W as f32 / 2.0 - 40.0, H as f32 / 2.0 - 40.0, 0.004);
    println!(
        "typical lasso: {} pts, perimeter {:.0} px; full-page lasso: {} pts, perimeter {:.0} px",
        typical.len(),
        perimeter(&typical),
        page.len(),
        perimeter(&page)
    );
    let sel = raster::rasterize_polygon(&typical, W, H, true);
    let sel2 = raster::rasterize_polygon(&other, W, H, true);
    let screen = tone();
    println!(
        "typical lasso selection: {} tiles ({} partial); tone: {} tiles, all partial",
        sel.tile_count(),
        sel.byte_size() / 4096,
        screen.tile_count()
    );
    println!();
    println!("| operation | {} |", threads.map(|t| format!("{t} thr: min (median) ms")).join(" | "));
    println!("|---|{}|", threads.map(|_| "---:").join("|"));

    type Case<'a> = (&'a str, usize, Box<dyn Fn() -> f64 + Sync + 'a>);
    let (sel_ref, sel2_ref) = (&sel, &sel2);
    let combine = |op| {
        let (sel, sel2) = (sel_ref, sel2_ref);
        move || {
            let mut s = sel.clone();
            time(|| s.combine(sel2, op))
        }
    };
    let cases: Vec<Case<'_>> = vec![
        ("select all", 41, Box::new(|| time(|| Selection::all(W, H)))),
        (
            "deselect (drop a select-all)",
            21,
            Box::new(|| {
                let s = Selection::all(W, H);
                time(move || drop(s))
            }),
        ),
        ("invert, typical lasso", 21, Box::new(|| time(|| sel.inverted(W, H)))),
        ("add, two lassos", 21, Box::new(combine(SelectOp::Add))),
        ("subtract, two lassos", 21, Box::new(combine(SelectOp::Subtract))),
        ("intersect, two lassos", 21, Box::new(combine(SelectOp::Intersect))),
        ("rasterize typical lasso", 11, Box::new(|| time(|| raster::rasterize_polygon(&typical, W, H, true)))),
        ("rasterize full-page lasso", 11, Box::new(|| time(|| raster::rasterize_polygon(&page, W, H, true)))),
        ("grow r=64 circle, typical lasso", 9, Box::new(|| time(|| morph::grow(&sel, 64, MorphShape::Circle, W, H)))),
        ("grow r=64 circle, full-page boundary", 9, Box::new(|| time(|| morph::grow(&screen, 64, MorphShape::Circle, W, H)))),
        ("shrink r=64 circle, full-page boundary", 9, Box::new(|| time(|| morph::shrink(&screen, 64, MorphShape::Circle, W, H)))),
        ("grow r=64 square, full-page boundary", 9, Box::new(|| time(|| morph::grow(&screen, 64, MorphShape::Square, W, H)))),
        ("feather σ=16, typical lasso", 9, Box::new(|| time(|| morph::feather(&sel, 16, W, H)))),
        ("feather σ=16, full-page boundary", 9, Box::new(|| time(|| morph::feather(&screen, 16, W, H)))),
    ];
    for (name, runs, f) in &cases {
        let cols: Vec<String> = threads
            .iter()
            .map(|&n| {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
                pool.install(|| {
                    f(); // warm-up
                    let (min, med) = min_median_ms(*runs, f);
                    format!("{min:.2} ({med:.2})")
                })
            })
            .collect();
        println!("| {name} | {} |", cols.join(" | "));
    }
}
