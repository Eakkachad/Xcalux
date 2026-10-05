//! Benchmark of CPU upload preparation throughput and memory allocations:
//! Pre-E5 (Worker::new per rayon split) vs E5 (persistent thread-local Worker reuse).
//!
//! cargo run -p arty-render --release --example bench_upload

use std::time::{Duration, Instant};

use arty_core::{BlendMode, Document, TILE_SIZE, TileCoord, fix15};
use arty_render::gpu::{CHUNK, TILES_PER_CHUNK, UploadRect, chunk_slot};
use arty_render::upload::{
    CHAIN_BYTES, CanvasSync, plan_rects, row_jobs, run_jobs_fresh,
};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

const A4_W: u32 = 2894;
const A4_H: u32 = 4093;

fn build_a4_doc() -> Document {
    let mut doc = Document::new(A4_W, A4_H, 350);
    doc.set_paper(Some([fix15::ONE_U16; 4]));
    let base = doc.active();
    let folder = doc.add_folder().unwrap();
    let clip = doc.add_raster_layer().unwrap();
    doc.move_layer(clip, Some(folder), 0);
    let tone = doc.add_raster_layer().unwrap();
    doc.move_layer(tone, Some(folder), 0);
    let line = doc.add_raster_layer().unwrap();

    let mut p_clip = doc.layer(clip).unwrap().props.clone();
    p_clip.clip = true;
    p_clip.blend = BlendMode::Multiply;
    p_clip.opacity = 0.8;
    doc.set_props(clip, p_clip);

    let mut p_tone = doc.layer(tone).unwrap().props.clone();
    p_tone.blend = BlendMode::Multiply;
    doc.set_props(tone, p_tone);

    let tiles_x = A4_W.div_ceil(TILE_SIZE as u32);
    let tiles_y = A4_H.div_ceil(TILE_SIZE as u32);

    for (layer_i, id) in [base, clip, tone, line].into_iter().enumerate() {
        let (grid, _) = doc.paint_target(id).unwrap();
        for y in 0..tiles_y {
            for x in 0..tiles_x {
                let c = TileCoord::new(x as i32, y as i32);
                let v = ((layer_i as u16 + 1) * 3500 + (x * 7 + y * 13) as u16 * 120) % 32000;
                grid.get_mut_or_create(c).as_flattened_mut().fill([v / 2, v / 3, v / 4, v]);
            }
        }
    }
    doc
}

fn plan_for_tiles(doc: &Document, tiles: &[TileCoord]) -> Vec<UploadRect> {
    let chunks_x = doc.width().div_ceil(CHUNK);
    let mut slots: Vec<_> = tiles
        .iter()
        .filter(|c| doc.contains_tile(**c))
        .map(|c| {
            let (tx, ty) = (c.x as u32, c.y as u32);
            let layer = chunk_slot(tx, ty, TILES_PER_CHUNK, chunks_x).0;
            (layer, ty, tx)
        })
        .collect();
    slots.sort_unstable();
    slots.dedup();
    let mut rects = Vec::new();
    plan_rects(&slots, &mut rects);
    rects
}

fn prepare_staging_fresh(staging: &mut Vec<u8>, doc: &Document, batch: &[UploadRect]) {
    let bytes: usize = batch.iter().map(|r| (r.w * r.h) as usize * CHAIN_BYTES).sum();
    staging.resize(bytes, 0);

    let total_rows: usize = batch.iter().map(|r| r.h as usize).sum();
    let mut jobs = Vec::with_capacity(total_rows);
    let mut buf = &mut staging[..];
    for r in batch {
        let (mine, tail) = buf.split_at_mut((r.w * r.h) as usize * CHAIN_BYTES);
        buf = tail;
        row_jobs(r, mine, &mut jobs);
    }
    run_jobs_fresh(doc, jobs);
}

fn prepare_staging_reused(staging: &mut Vec<u8>, doc: &Document, batch: &[UploadRect]) {
    CanvasSync::prepare_staging(staging, doc, batch);
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn min(times: &[Duration]) -> Duration {
    *times.iter().min().unwrap()
}

struct Case {
    name: &'static str,
    tiles: Vec<TileCoord>,
}

fn run_suite_in_pool(doc: &Document, cases: &[Case], pool_name: &str, rounds: usize, warmup: usize) {
    println!("### {} ({} interleaved rounds)", pool_name, rounds);
    println!("| Case | Dirty Tiles | Before: min (median) ms | After: min (median) ms | Speedup (median) | Steady-state heap |");
    println!("|---|---:|---:|---:|---:|---:|");

    let mut staging_before = Vec::new();
    let mut staging_after = Vec::new();

    for case in cases {
        let rects = plan_for_tiles(doc, &case.tiles);

        // Warmup rounds
        for _ in 0..warmup {
            prepare_staging_fresh(&mut staging_before, doc, &rects);
            prepare_staging_reused(&mut staging_after, doc, &rects);
        }

        // Measure steady-state heap growth during a reused run vs fresh run
        let (_, before_heap) = arty_testkit::peak_bytes_during(|| {
            prepare_staging_fresh(&mut staging_before, doc, &rects);
        });

        // Ensure workers on all pool threads are warm
        prepare_staging_reused(&mut staging_after, doc, &rects);
        let (_, after_heap) = arty_testkit::peak_bytes_during(|| {
            prepare_staging_reused(&mut staging_after, doc, &rects);
        });

        let mut before_times = Vec::with_capacity(rounds);
        let mut after_times = Vec::with_capacity(rounds);

        for i in 0..rounds {
            if i % 2 == 0 {
                let t0 = Instant::now();
                prepare_staging_fresh(&mut staging_before, doc, &rects);
                before_times.push(t0.elapsed());

                let t1 = Instant::now();
                prepare_staging_reused(&mut staging_after, doc, &rects);
                after_times.push(t1.elapsed());
            } else {
                let t1 = Instant::now();
                prepare_staging_reused(&mut staging_after, doc, &rects);
                after_times.push(t1.elapsed());

                let t0 = Instant::now();
                prepare_staging_fresh(&mut staging_before, doc, &rects);
                before_times.push(t0.elapsed());
            }
        }

        assert_eq!(staging_before, staging_after, "output mismatch in case {}", case.name);

        let b_min = min(&before_times).as_secs_f64() * 1000.0;
        let b_med = median(before_times).as_secs_f64() * 1000.0;
        let a_min = min(&after_times).as_secs_f64() * 1000.0;
        let a_med = median(after_times).as_secs_f64() * 1000.0;
        let speedup = (b_med - a_med) / b_med * 100.0;

        let heap_str = if after_heap == 0 {
            format!("0 B (vs {:.1} KiB)", before_heap as f64 / 1024.0)
        } else {
            format!("{:.1} KiB (vs {:.1} KiB)", after_heap as f64 / 1024.0, before_heap as f64 / 1024.0)
        };

        println!(
            "| {} | {} | {:.3} ({:.3}) | {:.3} ({:.3}) | {:+.1} % | {} |",
            case.name,
            case.tiles.len(),
            b_min,
            b_med,
            a_min,
            a_med,
            speedup,
            heap_str,
        );
    }
    println!();
}

fn main() {
    println!("# B017 Upload Preparation Benchmark");
    println!("Machine: Windows 11, rustc release profile");
    println!("Building document (A4 350 dpi, 5 layers: paper, base, tone, clip, line)...");
    let doc = build_a4_doc();

    let tiles_x = A4_W.div_ceil(TILE_SIZE as u32);
    let tiles_y = A4_H.div_ceil(TILE_SIZE as u32);

    let cases = [
        Case {
            name: "1 dirty tile",
            tiles: vec![TileCoord::new(10, 10)],
        },
        Case {
            name: "16 dirty tiles (4x4)",
            tiles: (0..4).flat_map(|y| (0..4).map(move |x| TileCoord::new(x + 10, y + 10))).collect(),
        },
        Case {
            name: "256 dirty tiles (16x16 chunk)",
            tiles: (0..16).flat_map(|y| (0..16).map(move |x| TileCoord::new(x, y))).collect(),
        },
        Case {
            name: "Full A4 @ 350 dpi (2944 tiles)",
            tiles: (0..tiles_y).flat_map(|y| (0..tiles_x).map(move |x| TileCoord::new(x as i32, y as i32))).collect(),
        },
    ];

    let rounds = 21;
    let warmup = 5;

    // 1. T1 Target: 4 threads (N100 class 4-core)
    {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        pool.install(|| {
            run_suite_in_pool(&doc, &cases, "Target Tier T1 (4 threads, N100 class)", rounds, warmup);
        });
    }

    // 2. Workstation Tier: 20 threads (default global pool)
    {
        run_suite_in_pool(&doc, &cases, "Workstation (20 threads, default pool)", rounds, warmup);
    }

    // 3. Single thread baseline
    {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        pool.install(|| {
            run_suite_in_pool(&doc, &cases, "Single Thread (1 thread baseline)", rounds, warmup);
        });
    }
}
