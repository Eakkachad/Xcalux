//! Marching-ants outline extraction (SEL-UI, B007).
//!
//! cargo run -p arty-core --release --example bench_contour
//!
//! Times `contour::extract` on a B4 600 dpi page (6071×8598) for select-all,
//! a typical lasso (AA and hard-edged), and wand-on-screentone selections,
//! on 8 threads, 1 thread and every core.

use std::sync::Arc;
use std::time::Instant;

use arty_core::contour::{Contours, extract};
use arty_core::selection::full_mask;
use arty_core::{MaskPixels, Selection, TILE_SIZE, TileCoord};
use rayon::prelude::*;

const W: u32 = 6071;
const H: u32 = 8598;
const T: i32 = TILE_SIZE as i32;

/// Selection with coverage `f(x, y)`; `tile_hint(tile)` may answer a whole
/// tile (Some(0) empty, Some(255) full) without per-pixel work.
fn build(f: impl Fn(f32, f32) -> f32 + Sync, tile_hint: impl Fn(TileCoord) -> Option<u8> + Sync) -> Selection {
    let coords: Vec<TileCoord> =
        (0..H.div_ceil(64) as i32).flat_map(|y| (0..W.div_ceil(64) as i32).map(move |x| TileCoord::new(x, y))).collect();
    let tiles: Vec<(TileCoord, Arc<MaskPixels>)> = coords
        .par_iter()
        .filter_map(|&c| {
            match tile_hint(c) {
                Some(0) => return None,
                Some(_) => return Some((c, full_mask().clone())),
                None => {}
            }
            let mut m: MaskPixels = [[0; TILE_SIZE]; TILE_SIZE];
            for (y, row) in m.iter_mut().enumerate() {
                for (x, px) in row.iter_mut().enumerate() {
                    let (gx, gy) = (c.x * T + x as i32, c.y * T + y as i32);
                    if gx < W as i32 && gy < H as i32 {
                        *px = (f(gx as f32 + 0.5, gy as f32 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            }
            Some((c, Arc::new(m)))
        })
        .collect();
    let mut s = Selection::new();
    for (c, m) in tiles {
        s.insert_tile(c, m);
    }
    s
}

/// A wobbly closed lasso around the page centre: signed distance (px,
/// approximately) to its outline, positive inside.
fn lasso_sd(x: f32, y: f32) -> f32 {
    let (dx, dy) = (x - W as f32 / 2.0, y - H as f32 / 2.0);
    let a = dy.atan2(dx);
    let r = 2600.0 * (1.0 + 0.12 * (7.0 * a).sin() + 0.04 * (23.0 * a).sin() + 0.015 * (61.0 * a).sin());
    r - (dx * dx + dy * dy).sqrt()
}

fn lasso_hint(c: TileCoord) -> Option<u8> {
    // The tile's centre is more than a tile diagonal (with slack for the
    // distance approximation) from the outline.
    let sd = lasso_sd((c.x * T + 32) as f32, (c.y * T + 32) as f32);
    if sd > 200.0 {
        Some(255)
    } else if sd < -200.0 {
        Some(0)
    } else {
        None
    }
}

/// AA halftone dots, `pitch` px apart, radius `r`, inside `region`.
fn tone(pitch: f32, r: f32, region: [f32; 4]) -> impl Fn(f32, f32) -> f32 + Sync {
    move |x, y| {
        if x < region[0] || y < region[1] || x >= region[2] || y >= region[3] {
            return 0.0;
        }
        let (fx, fy) = ((x / pitch).fract() - 0.5, (y / pitch).fract() - 0.5);
        let d = (fx * fx + fy * fy).sqrt() * pitch;
        r - d + 0.5
    }
}

fn perimeter(c: &Contours) -> f64 {
    c.lods[0]
        .iter()
        .map(|p| {
            let n = p.pts.len();
            let m = if p.closed { n } else { n.saturating_sub(1) };
            (0..m)
                .map(|i| {
                    let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
                    (((b[0] - a[0]) as f64).powi(2) + ((b[1] - a[1]) as f64).powi(2)).sqrt()
                })
                .sum::<f64>()
        })
        .sum()
}

/// Median wall time of `runs` extractions, in ms, and the last result.
fn time(sel: &Selection, runs: usize) -> (f64, Contours) {
    let mut times = Vec::with_capacity(runs);
    let mut last = Contours::default();
    for _ in 0..runs {
        // Freeing the previous outline is not part of an extraction.
        drop(std::mem::take(&mut last));
        let t = Instant::now();
        last = extract(sel, W, H);
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[runs / 2], last)
}

fn main() {
    let all_threads = rayon::current_num_threads();
    println!("B4 600 dpi page {W}×{H}; rayon has {all_threads} threads");
    let cases: Vec<(&str, Selection)> = vec![
        ("select all", {
            let mut s = Selection::new();
            for ty in 0..H.div_ceil(64) as i32 {
                for tx in 0..W.div_ceil(64) as i32 {
                    s.insert_tile(TileCoord::new(tx, ty), full_mask().clone());
                }
            }
            s
        }),
        ("lasso, antialiased", build(|x, y| lasso_sd(x, y) + 0.5, lasso_hint)),
        ("lasso, hard edge", build(|x, y| if lasso_sd(x, y) >= 0.0 { 1.0 } else { 0.0 }, lasso_hint)),
        (
            "wand on tone, 1/4 page",
            build(tone(7.0, 2.2, [0.0, 0.0, W as f32 / 2.0, H as f32 / 2.0]), |c| {
                ((c.x * T) as u32 >= W / 2 || (c.y * T) as u32 >= H / 2).then_some(0)
            }),
        ),
        ("wand on tone, full page", build(tone(7.0, 2.2, [0.0, 0.0, W as f32, H as f32]), |_| None)),
    ];
    println!(
        "| case | tiles | 8 threads (ms) | 1 thread (ms) | {all_threads} threads (ms) | pool | truncated | LOD0 / LOD1 / LOD2 segments | outlines | perimeter (px) |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (name, sel) in &cases {
        let pool = |n: usize| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
        let runs = 9;
        let (t8, c) = pool(8).install(|| time(sel, runs));
        let (t1, _) = pool(1).install(|| time(sel, 3));
        let (tall, _) = time(sel, runs);
        let segs = |k: usize| c.lods[k].iter().map(|p| p.segments()).sum::<usize>();
        println!(
            "| {name} | {} | {t8:.2} | {t1:.2} | {tall:.2} | {} | {} | {} / {} / {} | {} | {:.0} |",
            sel.tile_count(),
            c.pool,
            c.truncated,
            segs(0),
            segs(1),
            segs(2),
            c.lods[0].len(),
            perimeter(&c)
        );
    }
}
