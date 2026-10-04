//! G4 gate: steady-state compositing must not touch the heap.

use arty_core::{BlendMode, CompositeScratch, Document, TileCoord, tile::new_tile_box};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

#[test]
fn composite_tile_is_allocation_free() {
    let mut doc = Document::new(256, 256, 350);
    let base = doc.active();
    let folder = doc.add_folder().unwrap();
    let clip = doc.add_raster_layer().unwrap();
    doc.move_layer(clip, Some(folder), 0);
    let inner_base = doc.add_raster_layer().unwrap();
    doc.move_layer(inner_base, Some(folder), 0);
    let mut p = doc.layer(clip).unwrap().props.clone();
    p.clip = true;
    p.blend = BlendMode::Multiply;
    doc.set_props(clip, p);
    for id in [base, clip, inner_base] {
        let (grid, _) = doc.paint_target(id).unwrap();
        grid.get_mut_or_create(TileCoord::new(1, 1)).as_flattened_mut().fill([1000, 2000, 3000, 8000]);
    }

    let mut scratch = CompositeScratch::new();
    let mut out = new_tile_box();
    // Warm-up sizes the scratch stack for this tree.
    doc.composite_tile(TileCoord::new(1, 1), &mut out, &mut scratch);

    let n = arty_testkit::count_allocs(|| {
        for y in 0..4 {
            for x in 0..4 {
                doc.composite_tile(TileCoord::new(x, y), &mut out, &mut scratch);
            }
        }
    });
    assert_eq!(n, 0, "composite_tile allocated {n} times");
}
