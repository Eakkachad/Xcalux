//! Test (b): Bit-identical output equality between the pre-E5 fresh worker path
//! and the persistent thread-local worker path across multiple document configurations.

use arty_core::{
    BlendMode, BorderStyle, Document, Frame, FrameShape, Panel, RectF, TileCoord, fix15,
};
use arty_render::gpu::{CHUNK, MIP_LEVELS, TILES_PER_CHUNK, UploadRect, chunk_slot};
use arty_render::upload::{
    CHAIN_BYTES, RowJob, level_bytes, plan_rects, row_jobs, run_jobs_fresh, run_jobs_reused,
};

fn make_jobs<'a>(r: &UploadRect, buf: &'a mut [u8]) -> Vec<RowJob<'a>> {
    let mut jobs = Vec::new();
    row_jobs(r, buf, &mut jobs);
    jobs
}

fn prepare_both(doc: &Document, rects: &[UploadRect]) -> (Vec<u8>, Vec<u8>) {
    let total_bytes: usize = rects.iter().map(|r| (r.w * r.h) as usize * CHAIN_BYTES).sum();
    let mut staging_fresh = vec![0u8; total_bytes];
    let mut staging_reused = vec![0u8; total_bytes];

    // Fresh worker path (pre-E5 baseline)
    {
        let mut jobs = Vec::new();
        let mut buf = &mut staging_fresh[..];
        for r in rects {
            let (mine, tail) = buf.split_at_mut((r.w * r.h) as usize * CHAIN_BYTES);
            buf = tail;
            row_jobs(r, mine, &mut jobs);
        }
        run_jobs_fresh(doc, jobs);
    }

    // Reused persistent worker path (E5)
    {
        let mut jobs = Vec::new();
        let mut buf = &mut staging_reused[..];
        for r in rects {
            let (mine, tail) = buf.split_at_mut((r.w * r.h) as usize * CHAIN_BYTES);
            buf = tail;
            row_jobs(r, mine, &mut jobs);
        }
        run_jobs_reused(doc, jobs);
    }

    (staging_fresh, staging_reused)
}

fn plan_for_dirty(doc: &Document, dirty: &[TileCoord]) -> Vec<UploadRect> {
    let chunks_x = doc.width().div_ceil(CHUNK);
    let mut slots: Vec<_> = dirty
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

fn assert_mips_identical(rects: &[UploadRect], fresh: &[u8], reused: &[u8], doc_name: &str) {
    assert_eq!(fresh.len(), reused.len());
    let mut offset = 0;
    for r in rects {
        let tiles = (r.w * r.h) as usize;
        for k in 0..MIP_LEVELS as usize {
            let n = level_bytes(k) * tiles;
            let fresh_level = &fresh[offset..offset + n];
            let reused_level = &reused[offset..offset + n];
            assert_eq!(
                fresh_level, reused_level,
                "document '{doc_name}': mip level {k} mismatch for rect {:?}",
                r
            );
            offset += n;
        }
    }
}

#[test]
fn equality_single_layer_partial_page() {
    // 150 × 200 px = 3 × 4 tiles, crossing tile bounds.
    let mut doc = Document::new(150, 200, 300);
    doc.set_paper(Some([fix15::ONE_U16; 4]));
    let id = doc.active();
    let (grid, _) = doc.paint_target(id).unwrap();
    for ty in 0..4 {
        for tx in 0..3 {
            let c = TileCoord::new(tx, ty);
            let v = ((tx * 5000 + ty * 7000) % 25000) as u16;
            grid.get_mut_or_create(c).as_flattened_mut().fill([v, v / 2, v / 3, fix15::ONE_U16]);
        }
    }
    let all_tiles: Vec<_> = (0..4).flat_map(|y| (0..3).map(move |x| TileCoord::new(x, y))).collect();
    let rects = plan_for_dirty(&doc, &all_tiles);
    let (fresh, reused) = prepare_both(&doc, &rects);
    assert_mips_identical(&rects, &fresh, &reused, "single_layer_partial_page");
}

#[test]
fn equality_blend_modes_clip_and_folders() {
    let mut doc = Document::new(256, 256, 350);
    doc.set_paper(Some([fix15::ONE_U16; 4]));
    let base = doc.active();
    let folder = doc.add_folder().unwrap();
    let clip1 = doc.add_raster_layer().unwrap();
    let clip2 = doc.add_raster_layer().unwrap();
    let norm = doc.add_raster_layer().unwrap();
    doc.move_layer(clip1, Some(folder), 0);
    doc.move_layer(clip2, Some(folder), 1);
    doc.move_layer(norm, Some(folder), 2);

    let top = doc.add_raster_layer().unwrap();

    let mut p1 = doc.layer(clip1).unwrap().props.clone();
    p1.clip = true;
    p1.blend = BlendMode::Multiply;
    p1.opacity = 0.8;
    doc.set_props(clip1, p1);

    let mut p2 = doc.layer(clip2).unwrap().props.clone();
    p2.clip = true;
    p2.blend = BlendMode::Screen;
    p2.opacity = 0.6;
    doc.set_props(clip2, p2);

    let mut p_top = doc.layer(top).unwrap().props.clone();
    p_top.blend = BlendMode::Overlay;
    p_top.opacity = 0.75;
    doc.set_props(top, p_top);

    for (layer_i, id) in [base, clip1, clip2, norm, top].into_iter().enumerate() {
        let (grid, _) = doc.paint_target(id).unwrap();
        for y in 0..4 {
            for x in 0..4 {
                let c = TileCoord::new(x, y);
                let val = ((layer_i as u16 + 1) * 4500 + (x * 11 + y * 17) as u16 * 200) % 32000;
                grid.get_mut_or_create(c).as_flattened_mut().fill([val / 2, val / 3, val / 4, val]);
            }
        }
    }

    let all_tiles: Vec<_> = (0..4).flat_map(|y| (0..4).map(move |x| TileCoord::new(x, y))).collect();
    let rects = plan_for_dirty(&doc, &all_tiles);
    let (fresh, reused) = prepare_both(&doc, &rects);
    assert_mips_identical(&rects, &fresh, &reused, "blend_modes_clip_and_folders");
}

#[test]
fn equality_frame_mask_and_partial_tiles() {
    let mut doc = Document::new(320, 320, 350);
    let folder = doc.add_folder().unwrap();
    let art = doc.add_raster_layer().unwrap();
    doc.move_layer(art, Some(folder), 0);

    let (grid, _) = doc.paint_target(art).unwrap();
    for y in 0..5 {
        for x in 0..5 {
            let c = TileCoord::new(x, y);
            grid.get_mut_or_create(c).as_flattened_mut().fill([12000, 15000, 18000, fix15::ONE_U16]);
        }
    }

    let shape = FrameShape {
        panels: vec![Panel::rect(RectF { x: 20.0, y: 30.0, w: 200.0, h: 180.0 }).unwrap()],
        border: BorderStyle { width: 4.0, color: [0, 0, 0, fix15::ONE_U16] },
    };
    doc.set_frame(folder, Some(Frame::build(shape, 320, 320)));

    let all_tiles: Vec<_> = (0..5).flat_map(|y| (0..5).map(move |x| TileCoord::new(x, y))).collect();
    let rects = plan_for_dirty(&doc, &all_tiles);
    let (fresh, reused) = prepare_both(&doc, &rects);
    assert_mips_identical(&rects, &fresh, &reused, "frame_mask_and_partial_tiles");
}

#[test]
fn equality_scattered_dirty_tiles() {
    let mut doc = Document::new(1024, 1024, 350);
    doc.set_paper(Some([fix15::ONE_U16; 4]));
    let id = doc.active();
    let (grid, _) = doc.paint_target(id).unwrap();

    let scattered = [
        TileCoord::new(1, 1),
        TileCoord::new(3, 5),
        TileCoord::new(7, 2),
        TileCoord::new(14, 14),
        TileCoord::new(0, 15),
        TileCoord::new(15, 0),
    ];
    for &c in &scattered {
        let v = ((c.x * 3000 + c.y * 5000) % 30000) as u16;
        grid.get_mut_or_create(c).as_flattened_mut().fill([v, v / 2, v / 3, fix15::ONE_U16]);
    }

    let rects = plan_for_dirty(&doc, &scattered);
    let (fresh, reused) = prepare_both(&doc, &rects);
    assert_mips_identical(&rects, &fresh, &reused, "scattered_dirty_tiles");
}

#[test]
fn equality_persistent_scratch_isolation_across_runs() {
    // Run doc1, then doc2, then doc1 again with reused workers.
    // Ensure that doc1's output is not affected by doc2 running on the same workers.
    let mut doc1 = Document::new(128, 128, 300);
    doc1.set_paper(Some([fix15::ONE_U16; 4]));
    let id1 = doc1.active();
    let (grid1, _) = doc1.paint_target(id1).unwrap();
    grid1.get_mut_or_create(TileCoord::new(0, 0)).as_flattened_mut().fill([1000, 2000, 3000, 4000]);

    let mut doc2 = Document::new(128, 128, 300);
    doc2.set_paper(Some([0; 4]));
    let id2 = doc2.active();
    let (grid2, _) = doc2.paint_target(id2).unwrap();
    grid2.get_mut_or_create(TileCoord::new(0, 0)).as_flattened_mut().fill([20000, 25000, 30000, 32000]);

    let rects1 = plan_for_dirty(&doc1, &[TileCoord::new(0, 0)]);
    let rects2 = plan_for_dirty(&doc2, &[TileCoord::new(0, 0)]);

    let (fresh1_initial, _) = prepare_both(&doc1, &rects1);
    let (fresh2, _) = prepare_both(&doc2, &rects2);

    // Now run doc1 with reused workers:
    let total_bytes1 = rects1.iter().map(|r| (r.w * r.h) as usize * CHAIN_BYTES).sum();
    let mut reused1 = vec![0u8; total_bytes1];
    let jobs1 = make_jobs(&rects1[0], &mut reused1);
    run_jobs_reused(&doc1, jobs1);
    assert_eq!(fresh1_initial, reused1, "first run of doc1");

    // Run doc2 with reused workers:
    let total_bytes2 = rects2.iter().map(|r| (r.w * r.h) as usize * CHAIN_BYTES).sum();
    let mut reused2 = vec![0u8; total_bytes2];
    let jobs2 = make_jobs(&rects2[0], &mut reused2);
    run_jobs_reused(&doc2, jobs2);
    assert_eq!(fresh2, reused2, "interleaved run of doc2");

    // Run doc1 again with reused workers:
    let mut reused1_again = vec![0u8; total_bytes1];
    let jobs1_again = make_jobs(&rects1[0], &mut reused1_again);
    run_jobs_reused(&doc1, jobs1_again);
    assert_eq!(fresh1_initial, reused1_again, "subsequent run of doc1 after doc2");
}
