//! Bucket fill timings on a B4 600 dpi page (B008).
//!
//! cargo run -p arty-core --release --example bench_fill
//!
//! Line art: one closed panel border holding ~10 % of the page plus a few
//! thousand antialiased random-walk strokes (seeded, so runs compare).
//! "10 % region" seeds inside the panel; "full leak" seeds outside it, so
//! the region is the rest of the page. Each case reports the median and
//! minimum of 9 runs of `fill_region` (3 on the 31-layer page) on an
//! 8-thread pool, the median of 3 single-thread runs for the plain cases,
//! and the median of 3 `apply_fill` runs. The "+n darkest" rows add area
//! scaling by n px with "To darkest pixel".

use std::time::Instant;

use arty_core::fill::{FillBlend, FillParams, FillRef, FillScratch, ScaleMode, apply_fill, fill_region};
use arty_core::fix15::ONE_U16;
use arty_core::{Document, LayerId, Selection, TileCoord};
use arty_testkit::synthetic::{Page, synthetic_manga_page};

const O: u16 = ONE_U16;
const PAGE: Page = Page::B4_600;
/// The 10 % panel: ~1920 × 2720 px.
const PANEL: (i32, i32, i32, i32) = (400, 600, 2320, 3320);
/// Random-walk strokes (each ~450 px long).
const STROKES: usize = 600;

struct Rng(u64);

impl Rng {
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Antialiased line art on layer `id`: a closed 6 px border around
/// [`PANEL`] and `strokes` random walks, 2.5 px wide.
fn draw_line_art(doc: &mut Document, id: LayerId, strokes: usize) {
    let (w, h) = (doc.width() as f64, doc.height() as f64);
    let (grid, _) = doc.paint_target(id).unwrap();
    let mut put = |x: i32, y: i32, a: f64| {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 || a <= 0.0 {
            return;
        }
        let c = TileCoord::from_pixel(x, y);
        let (ox, oy) = c.origin();
        let p = &mut grid.get_mut_or_create(c)[(y - oy) as usize][(x - ox) as usize];
        let v = (a.min(1.0) * O as f64) as u16;
        if v > p[3] {
            *p = [0, 0, 0, v];
        }
    };
    let seg = |put: &mut dyn FnMut(i32, i32, f64), (ax, ay): (f64, f64), (bx, by): (f64, f64), half: f64| {
        let (x0, x1) = ((ax.min(bx) - half - 1.0) as i32, (ax.max(bx) + half + 2.0) as i32);
        let (y0, y1) = ((ay.min(by) - half - 1.0) as i32, (ay.max(by) + half + 2.0) as i32);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = (dx * dx + dy * dy).max(1e-9);
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
                let d = ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt();
                put(x, y, half + 0.5 - d);
            }
        }
    };
    let (x0, y0, x1, y1) = PANEL;
    let (x0, y0, x1, y1) = (x0 as f64, y0 as f64, x1 as f64, y1 as f64);
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    for k in 0..4 {
        // Short pieces keep the bounding boxes small.
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        let n = 200;
        for i in 0..n {
            let t0 = i as f64 / n as f64;
            let t1 = (i + 1) as f64 / n as f64;
            let p = (a.0 + (b.0 - a.0) * t0, a.1 + (b.1 - a.1) * t0);
            let q = (a.0 + (b.0 - a.0) * t1, a.1 + (b.1 - a.1) * t1);
            seg(&mut put, p, q, 3.0);
        }
    }
    let mut rng = Rng(0x5EED_F111);
    for _ in 0..strokes {
        let (mut x, mut y) = (rng.f() * w, rng.f() * h);
        let mut ang = rng.f() * std::f64::consts::TAU;
        for _ in 0..30 {
            ang += (rng.f() - 0.5) * 0.8;
            let l = 8.0 + rng.f() * 12.0;
            let (nx, ny) = (x + ang.cos() * l, y + ang.sin() * l);
            seg(&mut put, (x, y), (nx, ny), 1.25);
            (x, y) = (nx, ny);
        }
    }
}

/// (median, min).
fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[0])
}

fn share(doc: &Document, r: &Selection) -> f64 {
    let mut px = 0f64;
    for (_, m) in r.tiles() {
        px += m.as_flattened().iter().filter(|&&v| v > 0).count() as f64;
    }
    px * 100.0 / (doc.width() as f64 * doc.height() as f64)
}

/// (median, min) ms of `runs` calls, and the region.
fn time_region(doc: &Document, seed: (i32, i32), p: &FillParams, runs: usize) -> ((f64, f64), Selection) {
    let mut s = FillScratch::default();
    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..runs {
        let t = Instant::now();
        let r = fill_region(doc, seed, p, &mut s).expect("region");
        times.push(t.elapsed().as_secs_f64() * 1e3);
        last = Some(r);
    }
    (stats(times), last.unwrap())
}

fn case(doc: &mut Document, layer: LayerId, name: &str, seed: (i32, i32), p: FillParams, one: &rayon::ThreadPool) {
    let runs = if p.reference == FillRef::AllVisible && doc.layer_count() > 3 { 3 } else { 9 };
    let ((med, min), r) = time_region(doc, seed, &p, runs);
    let single = if p.gap_px == 0 && p.area_scale == 0 && p.reference != FillRef::AllVisible {
        let ((m, _), _) = one.install(|| time_region(doc, seed, &p, 3));
        format!("{m:8.1}")
    } else {
        "       —".into()
    };
    let mut writes = Vec::new();
    for _ in 0..3 {
        let snap = doc.snapshot();
        let t = Instant::now();
        let edit = apply_fill(doc, layer, &r, [O / 2, 0, 0, O], 1.0, FillBlend::Normal);
        writes.push(t.elapsed().as_secs_f64() * 1e3);
        drop(edit);
        *doc = snap;
    }
    let (write, _) = stats(writes);
    println!(
        "| {name:<34} | {:>5.1} % | {med:8.1} | {min:8.1} | {single} | {write:8.1} | {:>5} |",
        share(doc, &r),
        r.tile_count()
    );
}

fn main() {
    // 8 as in B008; RAYON_NUM_THREADS sets it (B013 T1-emulated runs give the core count).
    let threads = std::env::var("RAYON_NUM_THREADS").ok().and_then(|n| n.parse().ok()).filter(|&n: &usize| n > 0).unwrap_or(8);
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global().unwrap();
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    println!(
        "B4 {}×{} px, {} cores available, {threads} worker threads",
        PAGE.width,
        PAGE.height,
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );

    // Line art on its own page: Active and Reference modes.
    let t = Instant::now();
    let mut doc = Document::new(PAGE.width, PAGE.height, PAGE.dpi);
    let lines = doc.active();
    draw_line_art(&mut doc, lines, STROKES);
    let target = doc.add_raster_layer().unwrap();
    let mut props = doc.layer(lines).unwrap().props.clone();
    props.reference = true;
    doc.set_props(lines, props);
    let tiles = doc.layer(lines).unwrap().raster().unwrap().len();
    println!("line art: {tiles} tiles, built in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    println!();
    println!("| Case | Region | fill_region median ms | min ms | 1 thread median | apply_fill ms | Tiles |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    let inside = (1300, 1900);
    let outside = (5000, 7000);
    for (mode, label) in [(FillRef::Active, "Active"), (FillRef::Reference, "Reference"), (FillRef::AllVisible, "AllVisible (1 layer)")]
    {
        doc.set_active(if mode == FillRef::Active { lines } else { target });
        let write_to = doc.active();
        for (seed, where_) in [(inside, "10 %"), (outside, "full leak")] {
            for r in [0u8, 8, 16] {
                let p = FillParams { reference: mode, gap_px: r, ..FillParams::default() };
                case(&mut doc, write_to, &format!("{label}, {where_}, R {r}"), seed, p, &one);
            }
        }
    }
    doc.set_active(lines);
    for n in [2i8, 10] {
        let p = FillParams { area_scale: n, scale_mode: ScaleMode::ToDarkest, ..FillParams::default() };
        case(&mut doc, lines, &format!("Active, full leak, +{n} darkest"), outside, p, &one);
    }

    // AllVisible on the 30-layer reference page with the line art on top.
    let t = Instant::now();
    let mut page = synthetic_manga_page(PAGE);
    let top = page.add_raster_layer().unwrap();
    draw_line_art(&mut page, top, STROKES);
    println!();
    println!(
        "30-layer page + line art built in {:.0} ms ({} layers incl. folders)",
        t.elapsed().as_secs_f64() * 1e3,
        page.layer_count()
    );
    println!();
    println!("| Case | Region | fill_region median ms | min ms | 1 thread median | apply_fill ms | Tiles |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    // Flats, tones and gradients cut the page up; the largest region is
    // the one around `outside`.
    for r in [0u8, 8, 16] {
        let p = FillParams { reference: FillRef::AllVisible, gap_px: r, ..FillParams::default() };
        case(&mut page, top, &format!("AllVisible 35 layers, largest, R {r}"), outside, p, &one);
    }
    for n in [2i8, 10] {
        let p = FillParams { reference: FillRef::AllVisible, area_scale: n, scale_mode: ScaleMode::ToDarkest, ..FillParams::default() };
        case(&mut page, top, &format!("AllVisible 35 layers, largest, +{n} darkest"), outside, p, &one);
    }
}
