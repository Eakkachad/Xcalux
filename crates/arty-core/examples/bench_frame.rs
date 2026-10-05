//! Frame border folders: build time, mask memory and composite cost.
//!
//! cargo run -p arty-core --release --example bench_frame
//!
//! On a B4 600 dpi page (6071 × 8598, 95 × 135 tiles), with the JP
//! commercial inner frame divided into 6 and 30 panels (one angled cut)
//! and a 0.6 mm border:
//! - `Frame::build` time on the rayon pool and on one thread, and
//!   `mask_bytes`;
//! - 20 frame folders built in parallel, as `fram::apply` does on load;
//! - `composite_tile` ns/tile on Full, Partial and Outside tiles of a frame
//!   folder against the same folder without a frame, Normal and
//!   pass-through, over 3 painted children (one thread).
//!
//! Times are `min / median` of the runs: the minimum is the cost on an idle
//! machine, the median shows how noisy it was.

use std::hint::black_box;
use std::time::Instant;

use arty_core::page::{MANGA_PRESETS, PageSetup};
use arty_core::tile::{fill_tile, new_tile_box};
use arty_core::{BlendMode, BorderStyle, CompositeScratch, Cov, Document, Frame, FrameShape, Panel, TILE_SIZE, TileCoord};
use rayon::prelude::*;

const W: u32 = 6071;
const H: u32 = 8598;

/// `(min, median)` of `runs` timings of `f` in ms.
fn time_ms(runs: usize, mut f: impl FnMut()) -> (f64, f64) {
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(f64::total_cmp);
    (t[0], t[t.len() / 2])
}

/// The B4 inner frame cut into `rows × cols` panels with 5 mm / 2 mm
/// gutters; the first horizontal cut is angled.
fn layout(rows: usize, cols: usize) -> FrameShape {
    let (_, _, page) = PageSetup::from_mm(&MANGA_PRESETS[0], 600);
    let px = |mm: f32| mm / 25.4 * 600.0;
    let border = BorderStyle { width: px(0.6), color: [0, 0, 0, 1 << 15] };
    let mut shape = FrameShape { panels: vec![Panel::rect(page.inner).unwrap()], border };
    let r = page.inner;
    let (gap_h, gap_v) = (px(5.0), px(2.0));
    for i in 1..rows {
        let y = r.y + r.h * i as f32 / rows as f32;
        let tilt = if i == 1 { 300.0 } else { 0.0 };
        shape = shape.cut([r.x - 50.0, y], [r.x + r.w + 50.0, y + tilt], gap_h, gap_v).unwrap().0;
    }
    for j in 1..cols {
        let x = r.x + r.w * j as f32 / cols as f32;
        shape = shape.cut([x, r.y - 50.0], [x, r.y + r.h + 50.0], gap_h, gap_v).unwrap().0;
    }
    assert_eq!(shape.panels.len(), rows * cols);
    shape
}

/// A tile of `f` of the given kind, the first found from the page centre.
fn find(f: &Frame, want: &str) -> TileCoord {
    let (tw, th) = f.tiles();
    let mut all: Vec<TileCoord> =
        (0..th as i32).flat_map(|y| (0..tw as i32).map(move |x| TileCoord::new(x, y))).collect();
    let mid = TileCoord::new(tw as i32 / 2, th as i32 / 2);
    all.sort_by_key(|c| (c.x - mid.x).abs() + (c.y - mid.y).abs());
    *all.iter()
        .find(|&&c| matches!(
            (want, f.content(c), f.border(c)),
            ("full", Cov::Full, Cov::None) | ("partial", Cov::Partial(_), Cov::Partial(_)) | ("outside", Cov::None, Cov::None)
        ))
        .expect("tile kind")
}

/// Min ns per `composite_tile` over the tiles `cs` in `a` and in `b`,
/// measured in alternating blocks so machine load hits both alike.
fn composite_ab(a: &Document, b: &Document, cs: &[TileCoord], iters: usize) -> (f64, f64) {
    let mut scratch = CompositeScratch::new();
    let mut out = new_tile_box();
    let rounds = iters.div_ceil(cs.len());
    let mut block = |doc: &Document| {
        let start = Instant::now();
        for _ in 0..rounds {
            for &c in cs {
                doc.composite_tile(black_box(c), &mut out, &mut scratch);
            }
        }
        start.elapsed().as_secs_f64() * 1e9 / (rounds * cs.len()) as f64
    };
    block(a);
    block(b);
    let (mut ta, mut tb) = (f64::INFINITY, f64::INFINITY);
    for _ in 0..25 {
        ta = ta.min(block(a));
        tb = tb.min(block(b));
    }
    (ta, tb)
}

fn main() {
    println!("threads: {} (rayon pool)", rayon::current_num_threads());
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let shapes = [layout(3, 2), layout(6, 5)];
    for shape in &shapes {
        let n = shape.panels.len();
        let bytes = Frame::build(shape.clone(), W, H).mask_bytes();
        let (min, med) = time_ms(31, || drop(black_box(Frame::build(shape.clone(), W, H))));
        let (min1, med1) = one.install(|| time_ms(11, || drop(black_box(Frame::build(shape.clone(), W, H)))));
        println!(
            "Frame::build {n:>2} panels: pool {min:6.2} / {med:6.2} ms, 1 thread {min1:6.2} / {med1:6.2} ms; \
             mask_bytes {:.2} MiB ({} masks)",
            bytes as f64 / (1 << 20) as f64,
            bytes / (TILE_SIZE * TILE_SIZE)
        );
    }

    // 20 folders on load (fram::apply builds them in parallel).
    let twenty: Vec<FrameShape> = (0..20).map(|i| shapes[i % 2].clone()).collect();
    let (min, med) = time_ms(11, || {
        let built: Vec<_> = twenty.par_iter().map(|s| Frame::build(s.clone(), W, H)).collect();
        drop(black_box(built));
    });
    println!("20 frame folders (10 × 6 + 10 × 30 panels), built in parallel: {min:6.1} / {med:6.1} ms");

    // Composite: [paper, folder{3 painted rasters}], with and without a frame.
    // Every 8th partial tile of the 6-panel frame stands for "a partial tile".
    let frame = Frame::build(shapes[0].clone(), W, H);
    let (tw, th) = frame.tiles();
    let partial: Vec<TileCoord> = (0..th as i32)
        .flat_map(|y| (0..tw as i32).map(move |x| TileCoord::new(x, y)))
        .filter(|&c| matches!(frame.content(c), Cov::Partial(_)))
        .step_by(8)
        .collect();
    let one = |kind| vec![find(&frame, kind)];
    let sets = [
        ("Full", one("full")),
        ("Partial (central)", one("partial")),
        ("Partial (sample)", partial.clone()),
        ("Outside", one("outside")),
    ];
    println!("partial tiles sampled: {} of the frame's partial tiles", partial.len());
    for layers in [3, 10] {
        let mut doc = Document::new(W, H, 600);
        let folder = doc.add_folder().unwrap();
        for i in 0..layers {
            let id = doc.add_raster_layer().unwrap();
            doc.move_layer(id, Some(folder), i);
            let (grid, _) = doc.paint_target(id).unwrap();
            for (_, cs) in &sets {
                for &c in cs {
                    fill_tile(grid.get_mut_or_create(c), [3000 + (i as u16 % 5) * 2000, 9000, 4000, 16000]);
                }
            }
        }
        for blend in [BlendMode::Normal, BlendMode::PassThrough] {
            let mut p = doc.layer(folder).unwrap().props.clone();
            p.blend = blend;
            doc.set_props(folder, p);
            let mut framed = doc.snapshot();
            framed.set_frame(folder, Some(frame.clone()));
            for (name, cs) in &sets {
                let (plain, with) = composite_ab(&doc, &framed, cs, 2_000);
                println!(
                    "composite {layers:>2} children {:<11} {name:<17}: plain folder {plain:7.0} ns, frame folder {with:7.0} ns ({:+.1} %)",
                    format!("{blend:?}"),
                    (with / plain - 1.0) * 100.0
                );
            }
        }
    }
}
