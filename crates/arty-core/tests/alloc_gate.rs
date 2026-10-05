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

#[test]
fn frame_composite_is_allocation_free() {
    use arty_core::{BorderStyle, Cov, Frame, FrameShape, Panel, RectF};
    let mut doc = Document::new(320, 320, 350);
    let base = doc.active();
    // [base, frame folder {art}, clip], plus a pass-through frame folder
    // that is itself a clip.
    let folder = doc.add_folder().unwrap();
    let art = doc.add_raster_layer().unwrap();
    doc.move_layer(art, Some(folder), 0);
    doc.set_active(folder);
    let clip = doc.add_raster_layer().unwrap();
    let clip_folder = doc.add_folder().unwrap();
    let inner = doc.add_raster_layer().unwrap();
    doc.move_layer(inner, Some(clip_folder), 0);
    for id in [clip, clip_folder] {
        let mut p = doc.layer(id).unwrap().props.clone();
        p.clip = true;
        doc.set_props(id, p);
    }
    for id in [base, art, clip, inner] {
        let (grid, _) = doc.paint_target(id).unwrap();
        for y in 0..5 {
            for x in 0..5 {
                grid.get_mut_or_create(TileCoord::new(x, y)).as_flattened_mut().fill([1000, 2000, 3000, 8000]);
            }
        }
    }
    let shape = |r: RectF| FrameShape {
        panels: vec![Panel::rect(r).unwrap()],
        border: BorderStyle { width: 5.0, color: [0, 0, 0, 1 << 15] },
    };
    let f = Frame::build(shape(RectF { x: 10.5, y: 10.0, w: 230.0, h: 200.0 }), 320, 320);
    // Partial, Full and Outside tiles are all composited below.
    assert!(matches!(f.content(TileCoord::new(0, 0)), Cov::Partial(_)));
    assert_eq!(f.content(TileCoord::new(1, 1)), Cov::Full);
    assert_eq!(f.content(TileCoord::new(4, 4)), Cov::None);
    doc.set_frame(folder, Some(f));
    doc.set_frame(clip_folder, Some(Frame::build(shape(RectF { x: 30.0, y: 40.0, w: 100.0, h: 100.0 }), 320, 320)));

    let mut scratch = CompositeScratch::new();
    let mut out = new_tile_box();
    doc.composite_tile(TileCoord::new(1, 1), &mut out, &mut scratch);

    let n = arty_testkit::count_allocs(|| {
        for y in 0..5 {
            for x in 0..5 {
                doc.composite_tile(TileCoord::new(x, y), &mut out, &mut scratch);
            }
        }
    });
    assert_eq!(n, 0, "frame compositing allocated {n} times");
}

#[test]
fn history_push_is_allocation_free_when_warm() {
    use arty_core::{Edit, History, tile::new_tile};
    let mut doc = Document::new(512, 512, 350);
    for i in 0..15 {
        let id = if i == 0 { doc.active() } else { doc.add_raster_layer().unwrap() };
        let (grid, _) = doc.paint_target(id).unwrap();
        for c in 0..16 {
            grid.get_mut_or_create(TileCoord::new(c % 8, c / 8))[0][0] = [i + 1; 4];
        }
    }
    let layer = doc.active();
    // 20 tiles the document does not hold, so every one is a candidate.
    let edit = || Edit::Pixels { layer, tiles: (0..20).map(|x| (TileCoord::new(x, 4), Some(new_tile()))).collect() };
    // Under budget (no scan), and over it (the document scan on every push).
    for mut h in [History::new(4), History::with_budget(4, 1)] {
        for _ in 0..6 {
            h.push(edit(), &doc);
        }
        let prebuilt = edit();
        let n = arty_testkit::count_allocs(|| h.push(prebuilt, &doc));
        assert_eq!(n, 0, "History::push allocated {n} times (budget {})", h.budget());
    }
    // Three steps fit without the scan; the fourth re-costs them, then trims.
    let mut h = History::new(8);
    h.push(edit(), &doc);
    h.set_budget(h.usage().undo_bytes * 7 / 2);
    for round in 0..3 {
        h.clear();
        for _ in 0..3 {
            h.push(edit(), &doc);
        }
        let prebuilt = edit();
        let n = arty_testkit::count_allocs(|| h.push(prebuilt, &doc));
        assert_eq!(h.usage().trimmed, 1);
        if round > 0 {
            assert_eq!(n, 0, "History::push re-costing allocated {n} times");
        }
    }
}
