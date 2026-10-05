//! Transform session timings (m3 §7, B009).
//!
//! cargo run -p arty-core --release --example bench_transform
//!
//! Set `RAYON_NUM_THREADS` to fix the pool size (the targets assume 8).
//! Prints a markdown table:
//! 1. resampling throughput per filter (ns per destination px), 1 thread
//!    and the whole pool;
//! 2. a drag preview frame (nearest preview + parallel recomposite of the
//!    dirty tiles, as the canvas sync does) for a 2000² float on 10 layers;
//! 3. the same for a full-page layer on the synthetic 30-layer B4 page
//!    (record only);
//! 4. full-page commits: bicubic rotate + scale, and a 64-px translate.

use std::time::Instant;

use arty_core::tile::{TILE_SIZE, new_tile_box};
use arty_core::transform::{Filter, FloatSession, XfParams};
use arty_core::{CompositeScratch, Document, LayerId, TileCoord};
use arty_testkit::synthetic::{Page, synthetic_manga_page};
use rayon::prelude::*;

const ONE: u32 = 1 << 15;

/// Paint `[x0, x1) × [y0, y1)` of layer `id` with smooth, varied, valid pixels.
fn fill(doc: &mut Document, id: LayerId, (x0, y0, x1, y1): (i32, i32, i32, i32), seed: u32) {
    let (g, _) = doc.paint_target(id).unwrap();
    let t = TILE_SIZE as i32;
    let tiles: Vec<TileCoord> = (y0.div_euclid(t)..=(y1 - 1).div_euclid(t))
        .flat_map(|ty| (x0.div_euclid(t)..=(x1 - 1).div_euclid(t)).map(move |tx| TileCoord::new(tx, ty)))
        .collect();
    for c in tiles {
        let (ox, oy) = c.origin();
        let px = g.get_mut_or_create(c);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let (gx, gy) = (ox + x as i32, oy + y as i32);
                if gx < x0 || gy < y0 || gx >= x1 || gy >= y1 {
                    continue;
                }
                let h = (gx as u32).wrapping_mul(2_654_435_761) ^ (gy as u32).wrapping_mul(40_503) ^ seed;
                let a = ONE / 2 + (h % (ONE / 2));
                let v = |k: u32| (a * ((h >> k) & 255) / 255) as u16;
                px[y][x] = [v(3), v(11), v(19), a as u16];
            }
        }
    }
}

/// Recomposite the dirty page tiles in parallel, as `CanvasSync` does.
fn sync(doc: &mut Document) -> usize {
    let mut dirty = Vec::new();
    if doc.dirty_mut().drain_into(&mut dirty) {
        dirty = (0..doc.tiles_high() as i32)
            .flat_map(|y| (0..doc.tiles_wide() as i32).map(move |x| TileCoord::new(x, y)))
            .collect();
    }
    dirty.retain(|c| doc.contains_tile(*c));
    let doc: &Document = doc;
    dirty.par_iter().for_each_init(
        || (new_tile_box(), CompositeScratch::new()),
        |(tile, scratch), &c| doc.composite_tile(c, tile, scratch),
    );
    dirty.len()
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// A drag: `frames` nearest previews, each followed by a recomposite.
/// Returns (mean ms, max ms, mean preview-only ms, mean tiles recomposited).
fn drag(doc: &mut Document, s: &mut FloatSession, frames: usize) -> (f64, f64, f64, f64) {
    sync(doc);
    let p0 = s.params();
    let (mut total, mut worst, mut resample, mut tiles) = (0.0, 0.0f64, 0.0, 0.0);
    for i in 1..=frames {
        let f = i as f64;
        let p = XfParams { t: [f * 7.3, f * 3.1], theta: f * 0.01, s: [1.0 + f * 0.004; 2], ..p0 };
        let start = Instant::now();
        s.preview(doc, p, Filter::Nearest);
        let pre = ms(start);
        tiles += sync(doc) as f64;
        let all = ms(start);
        total += all;
        worst = worst.max(all);
        resample += pre;
    }
    let n = frames as f64;
    (total / n, worst, resample / n, tiles / n)
}

fn main() {
    let threads = rayon::current_num_threads();
    println!("rayon threads: {threads}\n");
    println!("| case | value |\n|---|---:|");

    // 1. Filter throughput: a 2048² block rotated and scaled (every dest
    // pixel takes the general sampling path).
    let mut doc = Document::new(4096, 4096, 600);
    let id = doc.active();
    fill(&mut doc, id, (512, 512, 2560, 2560), 1);
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    for f in [Filter::Nearest, Filter::Bilinear, Filter::Bicubic] {
        for (label, pool) in [("1 thread", Some(&one)), ("pool", None)] {
            let mut s = FloatSession::begin(&mut doc, id).unwrap();
            let p = XfParams { theta: 0.3, s: [1.1, 1.1], t: [15.5, 7.25], ..s.params() };
            let run = |s: &mut FloatSession, doc: &mut Document| {
                let start = Instant::now();
                s.preview(doc, p, f);
                ms(start)
            };
            let mut go = || {
                run(&mut s, &mut doc); // warm (pyramid, tiles)
                let t = (0..3).map(|_| {
                    s.preview(&mut doc, XfParams { t: [0.0; 2], ..p }, f);
                    run(&mut s, &mut doc)
                });
                t.fold(f64::INFINITY, f64::min)
            };
            let t = match pool {
                Some(pool) => pool.install(go),
                None => go(),
            };
            let tiles = doc.layer(id).unwrap().raster().unwrap().len();
            let ns_px = t * 1e6 / (tiles * TILE_SIZE * TILE_SIZE) as f64;
            println!("| {f:?} resample, {label}: ns / dest px ({tiles} tiles, {t:.1} ms) | {ns_px:.2} |");
            s.cancel(&mut doc);
        }
    }

    // 2. Preview frame: a 2000² float on 10 full layers (3072² page).
    let mut doc = Document::new(3072, 3072, 600);
    let mut ids = vec![doc.active()];
    for _ in 1..10 {
        ids.push(doc.add_raster_layer().unwrap());
    }
    for (i, &l) in ids.iter().enumerate() {
        if i != 5 {
            fill(&mut doc, l, (0, 0, 3072, 3072), i as u32 * 77);
        }
    }
    fill(&mut doc, ids[5], (300, 300, 2300, 2300), 5);
    doc.set_active(ids[5]);
    let start = Instant::now();
    let mut s = FloatSession::begin(&mut doc, ids[5]).unwrap();
    let begin_ms = ms(start);
    let (mean, worst, pre, tiles) = drag(&mut doc, &mut s, 20);
    println!("| 2000² float on 10 layers: begin | {begin_ms:.1} ms |");
    println!("| 2000² float on 10 layers: drag frame mean (preview + recomposite of {tiles:.0} tiles) | {mean:.1} ms |");
    println!("| 2000² float on 10 layers: drag frame max | {worst:.1} ms |");
    println!("| 2000² float on 10 layers: preview (resample) only, mean | {pre:.1} ms |");
    let start = Instant::now();
    let p = s.params();
    s.preview(&mut doc, p, Filter::Bilinear);
    let n = sync(&mut doc);
    println!("| 2000² float on 10 layers: idle bilinear refine + recomposite ({n} tiles) | {:.1} ms |", ms(start));
    let start = Instant::now();
    let edit = s.commit(&mut doc, Filter::Bicubic);
    println!("| 2000² float on 10 layers: bicubic commit | {:.1} ms |", ms(start));
    drop(edit);
    drop(doc);

    // 3. Full-page layer on the synthetic 30-layer B4 page (record).
    let mut doc = synthetic_manga_page(Page::B4_600);
    let full = doc.add_raster_layer().unwrap();
    fill(&mut doc, full, (0, 0, 6071, 8598), 9);
    let start = Instant::now();
    let mut s = FloatSession::begin(&mut doc, full).unwrap();
    let begin_ms = ms(start);
    let (mean, worst, pre, tiles) = drag(&mut doc, &mut s, 5);
    println!("| full-page layer on 30 layers (B4 600 dpi): begin | {begin_ms:.1} ms |");
    println!("| full-page layer on 30 layers: drag frame mean (recomposite {tiles:.0} tiles) | {mean:.0} ms |");
    println!("| full-page layer on 30 layers: drag frame max | {worst:.0} ms |");
    println!("| full-page layer on 30 layers: preview (resample) only, mean | {pre:.1} ms |");
    s.cancel(&mut doc);
    drop(doc);

    // 4. Full-page commits on a one-layer B4 page.
    let mut doc = Document::new(6071, 8598, 600);
    let id = doc.active();
    fill(&mut doc, id, (0, 0, 6071, 8598), 4);
    for (label, p) in [
        ("bicubic commit, rotate 5° + scale 1.05", XfParams { theta: 5f64.to_radians(), s: [1.05; 2], ..XfParams::identity([0.0; 2]) }),
        ("bicubic commit, scale 0.37 (pyramid level 1)", XfParams { s: [0.37; 2], ..XfParams::identity([0.0; 2]) }),
        ("commit, translate (128, −64)", XfParams { t: [128.0, -64.0], ..XfParams::identity([0.0; 2]) }),
        ("commit, translate (13, 7)", XfParams { t: [13.0, 7.0], ..XfParams::identity([0.0; 2]) }),
    ] {
        for previewed in [false, true] {
            let mut s = FloatSession::begin(&mut doc, id).unwrap();
            let pivot = s.params().pivot;
            let p = XfParams { pivot, ..p };
            if previewed {
                // The usual flow: the user saw a nearest preview first, so
                // the commit reuses its tiles.
                s.preview(&mut doc, p, Filter::Nearest);
                doc.dirty_mut().drain_into(&mut Vec::new());
            }
            s.set_params(p);
            let start = Instant::now();
            let edit = s.commit(&mut doc, Filter::Bicubic);
            let t = ms(start);
            let when = if previewed { "after a preview" } else { "cold" };
            println!("| full-page layer (12 825 tiles): {label}, {when} | {t:.1} ms |");
            // Put the page back for the next case.
            let mut h = arty_core::History::default();
            h.push(edit.unwrap(), &doc);
            h.undo(&mut doc);
            doc.dirty_mut().drain_into(&mut Vec::new());
        }
    }
}
