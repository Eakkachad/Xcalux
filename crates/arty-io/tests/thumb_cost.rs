//! What the save's page thumbnail costs on the IO pool (plans/bench/B027):
//! `cargo test --release -p arty-io --test thumb_cost -- --ignored --nocapture`.
//! Interleaves the thumbnail with plain saves of the same page and prints
//! median and minimum of several runs.

mod common;

use std::time::Instant;

use arty_core::{Document, TileCoord};
use arty_io::thumb;
use arty_io::{Progress, SaveExtras, SaveOptions, Session, SessionId};
use common::*;

/// An A4 350 dpi page (46 × 64 tiles): a full paper-like first layer and
/// `layers - 1` more with `fill_pct` % of the tiles painted (line art and noise).
fn page(layers: usize, fill_pct: u64) -> Document {
    let mut rng = Rng(0xC0FFEE);
    let mut doc = Document::new(2894, 4093, 350);
    for l in 0..layers {
        let id = if l == 0 { doc.active() } else { doc.add_raster_layer().unwrap() };
        let (grid, _) = doc.paint_target(id).unwrap();
        for ty in 0..64 {
            for tx in 0..46 {
                if l == 0 || rng.chance(fill_pct, 100) {
                    let kind = if l == 0 { 3 } else { 2 + rng.below(3) };
                    grid.insert(TileCoord::new(tx, ty), tile(&mut rng, kind));
                }
            }
        }
    }
    doc
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[0])
}

#[test]
#[ignore = "measurement; run by hand"]
fn thumbnail_cost() {
    let dir = temp_dir("thumb-cost");
    for (layers, fill) in [(1, 0), (5, 20), (15, 20)] {
        let doc = page(layers, fill);
        for threads in [1usize, 2, 4] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let (mut make, mut plain, mut with) = (vec![], vec![], vec![]);
            for run in 0..7 {
                let t = Instant::now();
                let th = thumb::make(&doc, &pool).unwrap();
                make.push(t.elapsed().as_secs_f64() * 1e3);
                let save = |ex: &SaveExtras, name: &str| {
                    let path = dir.join(format!("{name}{run}.arty"));
                    let t = Instant::now();
                    Session::new(SessionId([1; 16]), None)
                        .save_main(&doc, ex, &path, false, &SaveOptions::default(), &pool, &Progress::default())
                        .unwrap();
                    (t.elapsed().as_secs_f64() * 1e3, std::fs::metadata(&path).unwrap().len())
                };
                let (a, _) = save(&SaveExtras::default(), "plain");
                let (b, _) = save(&SaveExtras { thumb: Some(th), ..Default::default() }, "with");
                plain.push(a);
                with.push(b + *make.last().unwrap());
            }
            let (mm, mn) = stats(make);
            let (pm, pn) = stats(plain);
            let (wm, wn) = stats(with);
            println!(
                "{layers:>2} layers {fill:>2}% · {threads} thread(s): thumbnail median {mm:6.1} ms (min {mn:6.1}) · save {pm:7.1} ms (min {pn:7.1}) · save + thumbnail {wm:7.1} ms (min {wn:7.1})"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);

    // From the canvas overview (an eighth of the page, kept by the display sync): the UI thread's share.
    for (w, h) in [(2894u32, 4093u32), (6071, 8598)] {
        let doc = Document::new(w, h, 350);
        let (ow, oh) = (w.div_ceil(64) as usize * 8, h.div_ceil(64) as usize * 8);
        let mut rng = Rng(7);
        let image: Vec<u8> = (0..ow * oh * 4).map(|_| rng.next() as u8).collect();
        let times: Vec<f64> = (0..21)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(thumb::from_overview(&doc, ow, oh, &image, 8));
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        let (m, n) = stats(times);
        println!("from overview {w} x {h} ({ow} x {oh} px): median {m:.2} ms (min {n:.2})");
    }
}
