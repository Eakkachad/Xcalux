//! E1 gate: undo memory stays within its budget however many whole-layer
//! steps are pushed.

use std::sync::Arc;

use arty_core::fill::{FillBlend, apply_fill};
use arty_core::{Document, Edit, History, Selection, TileCoord};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

const MIB: usize = 1 << 20;

/// Every page tile partial, so a fill makes a distinct tile per coordinate.
fn partial_region(w: u32, h: u32) -> Selection {
    let mut m = [[255u8; 64]; 64];
    m[0][0] = 128;
    let m = Arc::new(m);
    let mut s = Selection::new();
    for y in 0..h.div_ceil(64) as i32 {
        for x in 0..w.div_ceil(64) as i32 {
            s.insert_tile(TileCoord::new(x, y), m.clone());
        }
    }
    s
}

/// Fill the whole layer, then clear it, each as one step.
fn round(doc: &mut Document, h: &mut History, region: &Selection, v: u16) {
    let id = doc.active();
    let edit = apply_fill(doc, id, region, [v, v, v, 1 << 15], 1.0, FillBlend::Normal).unwrap();
    h.push(edit, doc);
    let grid = doc.layer(id).unwrap().raster().unwrap();
    let tiles: Vec<_> = grid.iter().map(|(c, t)| (c, Some(t.clone()))).collect();
    doc.clear_layer(id);
    h.push(Edit::Pixels { layer: id, tiles }, doc);
}

#[test]
fn whole_layer_steps_stay_within_budget() {
    // 1024² = 256 tiles = 8 MiB per layer.
    let region = partial_region(1024, 1024);
    {
        // Warm the rayon pool and the allocator outside the measurement.
        let mut doc = Document::new(1024, 1024, 350);
        round(&mut doc, &mut History::new(4), &region, 1);
    }
    let mut doc = Document::new(1024, 1024, 350);
    let budget = 16 * MIB;
    let mut h = History::with_budget(200, budget);
    let step = 256 * arty_core::TILE_BYTES + 256 * 16 + 4096;
    let base = arty_testkit::live_bytes();
    for v in 0..40 {
        round(&mut doc, &mut h, &region, 1000 + v);
        let grown = arty_testkit::live_bytes().saturating_sub(base);
        assert!(grown <= doc.pixel_bytes() + budget + step + MIB, "round {v}: {} MiB live", grown >> 20);
        let u = h.usage();
        assert!(u.undo_bytes <= u.budget || u.undo_steps == 1, "round {v}: {u:?}");
    }
    assert!(h.usage().trimmed > 0);
    h.clear();
    let grown = arty_testkit::live_bytes().saturating_sub(base);
    assert!(grown <= doc.pixel_bytes() + MIB, "clear left {} KiB", grown >> 10);
}
