//! Composite throughput baseline.
//!
//! cargo run -p arty-core --release --example bench_composite
//!
//! Builds a manga-page-sized stack of fully painted layers and times
//! `composite_tile` per tile (single thread) for Normal-only and mixed
//! blend-mode stacks.

use std::time::Instant;

use arty_core::{BlendMode, CompositeScratch, Document, TileCoord, tile::new_tile_box};

fn build(layers: usize, mixed: bool) -> Document {
    // A 1024² region is enough to measure per-tile cost.
    let mut doc = Document::new(1024, 1024, 600);
    for i in 0..layers {
        let id = if i == 0 { doc.active() } else { doc.add_raster_layer() };
        let (grid, _) = doc.paint_target(id).unwrap();
        for ty in 0..16 {
            for tx in 0..16 {
                let v = 4000 + (i as u16 * 997) % 20000;
                grid.get_mut_or_create(TileCoord::new(tx, ty)).as_flattened_mut().fill([v / 2, v / 3, v / 4, v]);
            }
        }
        if mixed && i % 3 == 1 {
            let mut p = doc.layer(id).unwrap().props.clone();
            p.blend = [BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay][i % 3];
            doc.set_props(id, p);
        }
    }
    doc
}

fn main() {
    let mut scratch = CompositeScratch::new();
    let mut out = new_tile_box();
    for (layers, mixed) in [(1, false), (10, false), (30, false), (10, true), (30, true)] {
        let doc = build(layers, mixed);
        let tiles = 256;
        let rounds = 8;
        let start = Instant::now();
        for _ in 0..rounds {
            for ty in 0..16 {
                for tx in 0..16 {
                    doc.composite_tile(TileCoord::new(tx, ty), &mut out, &mut scratch);
                }
            }
        }
        let per_tile = start.elapsed().as_secs_f64() * 1e6 / (tiles * rounds) as f64;
        // A B4 600 dpi page is 95 × 135 = 12 825 tiles.
        println!(
            "{layers:>3} layers {:<6}: {per_tile:7.1} µs/tile  → full B4@600dpi page {:7.1} ms (1 thread)",
            if mixed { "mixed" } else { "normal" },
            per_tile * 12_825.0 / 1000.0
        );
    }
}
