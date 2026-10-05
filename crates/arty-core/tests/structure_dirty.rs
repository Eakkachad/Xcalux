//! Integration tests for fine-grained structure dirty marking (TASK E2).

use std::sync::Arc;

use ahash::AHashMap;
use arty_core::blend::BlendMode;
use arty_core::composite::CompositeScratch;
use arty_core::document::Document;
use arty_core::fix15::ONE;
use arty_core::frame::{BorderStyle, Frame, FrameShape, Panel};
use arty_core::history::{Edit, History};
use arty_core::layer::LayerId;
use arty_core::tile::{TileCoord, TilePixels, new_tile_box};

// ----- Oracle Helper --------------------------------------------------------

/// An oracle that caches composited tiles, updates only the tiles reported
/// as dirty by `Document`, and verifies that every tile in the cache matches
/// a fresh full recomposite from scratch, pixel-for-pixel.
struct CompositeOracle {
    cache: AHashMap<TileCoord, Box<TilePixels>>,
    scratch: CompositeScratch,
    /// The tree as of the last verify, for failure messages.
    prev: String,
}

/// The layer tree bottom → top, one layer per line.
fn dump(doc: &Document) -> String {
    fn walk(doc: &Document, ids: &[LayerId], depth: usize, out: &mut String) {
        for &id in ids {
            let l = doc.layer(id).unwrap();
            let p = &l.props;
            let what = match l.raster() {
                Some(g) => format!("raster {:?}", g.coords().map(|c| (c.x, c.y)).collect::<Vec<_>>()),
                None => format!("folder frame={}", doc.frame(id).is_some()),
            };
            out.push_str(&format!(
                "{:w$}{id:?} {what} vis={} op={} {:?} clip={}
",
                "",
                p.visible,
                p.opacity,
                p.blend,
                p.clip,
                w = depth * 2
            ));
            if let Some(c) = l.children() {
                walk(doc, c, depth + 1, out);
            }
        }
    }
    let mut out = String::new();
    walk(doc, doc.root(), 0, &mut out);
    out
}

impl CompositeOracle {
    fn new(doc: &Document) -> Self {
        let mut oracle = Self {
            cache: AHashMap::default(),
            scratch: CompositeScratch::new(),
            prev: dump(doc),
        };
        oracle.rebuild(doc);
        oracle
    }

    fn rebuild(&mut self, doc: &Document) {
        self.cache.clear();
        let (tw, th) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
        for ty in 0..th {
            for tx in 0..tw {
                let c = TileCoord::new(tx, ty);
                let mut tile = new_tile_box();
                doc.composite_tile(c, &mut tile, &mut self.scratch);
                self.cache.insert(c, tile);
            }
        }
    }

    fn verify(&mut self, doc: &mut Document, op_name: &str) {
        let (tw, th) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
        let mut dirty = Vec::new();
        let all = doc.dirty_mut().drain_into(&mut dirty);

        if all {
            for ty in 0..th {
                for tx in 0..tw {
                    let c = TileCoord::new(tx, ty);
                    let mut tile = new_tile_box();
                    doc.composite_tile(c, &mut tile, &mut self.scratch);
                    self.cache.insert(c, tile);
                }
            }
        } else {
            for &c in &dirty {
                if c.x >= 0 && c.x < tw && c.y >= 0 && c.y < th {
                    let mut tile = new_tile_box();
                    doc.composite_tile(c, &mut tile, &mut self.scratch);
                    self.cache.insert(c, tile);
                }
            }
        }

        // Verify pixel-exact equality across all document tiles
        for ty in 0..th {
            for tx in 0..tw {
                let c = TileCoord::new(tx, ty);
                let mut full = new_tile_box();
                doc.composite_tile(c, &mut full, &mut self.scratch);
                let cached = self.cache.get(&c).expect("tile present in cache");
                assert!(
                    **cached == *full,
                    "{op_name}: tile {c:?} in cache differed from full recomposite (dirty={dirty:?}, all={all})
before:
{}after:
{}",
                    self.prev,
                    dump(doc)
                );
            }
        }
        self.prev = dump(doc);
    }
}

// ----- Deterministic PRNG ---------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pixel(&mut self) -> [u16; 4] {
        if self.below(6) == 0 {
            return [0; 4];
        }
        let a = 1 + self.below(ONE);
        [self.below(a + 1) as u16, self.below(a + 1) as u16, self.below(a + 1) as u16, a as u16]
    }
}

fn paint_tile(doc: &mut Document, id: LayerId, c: TileCoord, color: [u16; 4]) {
    if let Some((grid, dirty)) = doc.paint_target(id) {
        let tile = grid.get_mut_or_create(c);
        tile.as_flattened_mut().fill(color);
        dirty.mark(c);
    }
}

/// Paint a tile with a few random colours in a coarse pattern (transparent
/// cells included), so rounding differences show up per pixel.
fn paint_pattern(doc: &mut Document, rng: &mut Rng, id: LayerId, c: TileCoord) {
    let colors = [rng.pixel(), rng.pixel(), rng.pixel(), [0; 4]];
    let shift = rng.below(4);
    if let Some((grid, dirty)) = doc.paint_target(id) {
        let tile = grid.get_mut_or_create(c);
        for (y, row) in tile.iter_mut().enumerate() {
            for (x, px) in row.iter_mut().enumerate() {
                *px = colors[((x >> 3) + (y >> 2) + shift as usize) % 4];
            }
        }
        dirty.mark(c);
    }
}

fn make_test_frame(w: u32, h: u32, panels: Vec<[[f32; 2]; 4]>) -> Arc<Frame> {
    let panel_objs: Vec<Panel> = panels.into_iter().filter_map(|pts| Panel::new(pts.to_vec())).collect();
    let shape = FrameShape {
        panels: panel_objs,
        border: BorderStyle {
            width: 2.0,
            color: [ONE as u16, ONE as u16, ONE as u16, ONE as u16],
        },
    };
    Frame::build(shape, w, h)
}

fn all_layer_ids(doc: &Document) -> Vec<LayerId> {
    let mut out = Vec::new();
    fn walk(doc: &Document, ids: &[LayerId], out: &mut Vec<LayerId>) {
        for &id in ids {
            out.push(id);
            if let Some(l) = doc.layer(id)
                && let Some(children) = l.children()
            {
                walk(doc, children, out);
            }
        }
    }
    walk(doc, doc.root(), &mut out);
    out
}

// ----- Random sequence test -------------------------------------------------

#[test]
fn oracle_random_structure_ops() {
    for k in 0..16u64 {
        let seed = 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(k + 1) | 1;
        let mut rng = Rng(seed);
        // 192x192 = 3x3 tiles
        let mut doc = Document::new(192, 192, 72);
        let first = doc.root()[0];
        paint_pattern(&mut doc, &mut rng, first, TileCoord::new(1, 1));
        let mut history = History::default();
        let mut oracle = CompositeOracle::new(&doc);
        doc.dirty_mut().drain_into(&mut Vec::new());

        let blend_modes: Vec<BlendMode> =
            BlendMode::LAYER_MODES.iter().copied().chain([BlendMode::PassThrough, BlendMode::PassThrough]).collect();

        for step in 0..300 {
            let op = rng.below(14);
            let context = format!("seed {seed:#x} step {step} op {op}");
            let undo_before = history.undo_len();

            match op {
                0 => {
                    // Add raster layer
                    let snap = doc.snapshot_structure();
                    if let Some(id) = doc.add_raster_layer() {
                        history.push(Edit::Structure(Box::new(snap)), &doc);
                        // Optionally paint immediately
                        for _ in 0..rng.below(4) {
                            let c = TileCoord::new(rng.below(3) as i32, rng.below(3) as i32);
                            paint_pattern(&mut doc, &mut rng, id, c);
                        }
                        if rng.below(3) == 0 {
                            let mut p = doc.layer(id).unwrap().props.clone();
                            p.clip = true;
                            doc.set_props(id, p);
                        }
                    }
                    oracle.verify(&mut doc, &format!("{context} add_raster"));
                }
                1 => {
                    // Add folder
                    let snap = doc.snapshot_structure();
                    if doc.add_folder().is_some() {
                        history.push(Edit::Structure(Box::new(snap)), &doc);
                    }
                    oracle.verify(&mut doc, &format!("{context} add_folder"));
                }
                2 => {
                    // Paint a tile on a random raster layer
                    let rasters: Vec<LayerId> = all_layer_ids(&doc).into_iter().filter(|&id| doc.layer(id).is_some_and(|l| !l.is_folder())).collect();
                    if !rasters.is_empty() {
                        let id = rasters[rng.below(rasters.len() as u32) as usize];
                        let c = TileCoord::new(rng.below(3) as i32, rng.below(3) as i32);
                        paint_pattern(&mut doc, &mut rng, id, c);
                    }
                    oracle.verify(&mut doc, &format!("{context} paint"));
                }
                3 => {
                    // Change layer props
                    let all_ids = all_layer_ids(&doc);
                    if !all_ids.is_empty() {
                        let id = all_ids[rng.below(all_ids.len() as u32) as usize];
                        let mut p = doc.layer(id).unwrap().props.clone();
                        match rng.below(4) {
                            0 => p.clip = !p.clip,
                            1 => p.opacity = [0.0, 0.5, 1.0, rng.unit() as f32][rng.below(4) as usize],
                            2 => p.blend = blend_modes[rng.below(blend_modes.len() as u32) as usize],
                            _ => p.visible = !p.visible,
                        }
                        doc.set_props(id, p);
                    }
                    oracle.verify(&mut doc, &format!("{context} set_props"));
                }
                4 => {
                    // Move layer
                    let all_ids = all_layer_ids(&doc);
                    if all_ids.len() > 1 {
                        let id = all_ids[rng.below(all_ids.len() as u32) as usize];
                        let folders: Vec<Option<LayerId>> = std::iter::once(None)
                            .chain(all_ids.iter().copied().filter(|&l| doc.layer(l).is_some_and(|ly| ly.is_folder())).map(Some))
                            .collect();
                        let target_parent = folders[rng.below(folders.len() as u32) as usize];
                        let target_index = rng.below(10) as usize;
                        let snap = doc.snapshot_structure();
                        if doc.move_layer(id, target_parent, target_index) {
                            history.push(Edit::Structure(Box::new(snap)), &doc);
                        }
                    }
                    oracle.verify(&mut doc, &format!("{context} move_layer"));
                }
                5 => {
                    // Shift layer
                    let all_ids = all_layer_ids(&doc);
                    if !all_ids.is_empty() {
                        let id = all_ids[rng.below(all_ids.len() as u32) as usize];
                        let delta = if rng.below(2) == 0 { -1 } else { 1 };
                        let snap = doc.snapshot_structure();
                        if doc.shift_layer(id, delta) {
                            history.push(Edit::Structure(Box::new(snap)), &doc);
                        }
                    }
                    oracle.verify(&mut doc, &format!("{context} shift_layer"));
                }
                6 => {
                    // Duplicate layer
                    let all_ids = all_layer_ids(&doc);
                    if !all_ids.is_empty() && all_ids.len() < 15 {
                        let id = all_ids[rng.below(all_ids.len() as u32) as usize];
                        let snap = doc.snapshot_structure();
                        if doc.duplicate_layer(id).is_some() {
                            history.push(Edit::Structure(Box::new(snap)), &doc);
                        }
                    }
                    oracle.verify(&mut doc, &format!("{context} duplicate_layer"));
                }
                7 => {
                    // Merge down
                    let all_ids = all_layer_ids(&doc);
                    if !all_ids.is_empty() {
                        let id = all_ids[rng.below(all_ids.len() as u32) as usize];
                        let snap = doc.snapshot_structure();
                        if doc.merge_down(id) {
                            history.push(Edit::Structure(Box::new(snap)), &doc);
                        }
                    }
                    oracle.verify(&mut doc, &format!("{context} merge_down"));
                }
                8 => {
                    // Delete layer
                    let all_ids = all_layer_ids(&doc);
                    if all_ids.len() > 1 {
                        let id = all_ids[rng.below(all_ids.len() as u32) as usize];
                        let snap = doc.snapshot_structure();
                        if doc.delete_layer(id) {
                            history.push(Edit::Structure(Box::new(snap)), &doc);
                        }
                    }
                    oracle.verify(&mut doc, &format!("{context} delete_layer"));
                }
                9 => {
                    // Set / clear frame on folder
                    let folders: Vec<LayerId> = all_layer_ids(&doc).into_iter().filter(|&id| doc.layer(id).is_some_and(|l| l.is_folder())).collect();
                    if !folders.is_empty() {
                        let id = folders[rng.below(folders.len() as u32) as usize];
                        let frame = match rng.below(3) {
                            0 => Some(make_test_frame(192, 192, vec![[[10.0, 10.0], [100.0, 10.0], [100.0, 100.0], [10.0, 100.0]]])),
                            1 => Some(make_test_frame(192, 192, vec![[[70.0, 20.0], [180.0, 20.0], [180.0, 60.0], [70.0, 60.0]]])),
                            _ => None,
                        };
                        let snap = doc.snapshot_structure();
                        let _ = doc.set_frame(id, frame);
                        history.push(Edit::Structure(Box::new(snap)), &doc);
                    }
                    oracle.verify(&mut doc, &format!("{context} set_frame"));
                }
                10 => {
                    // History undo
                    if history.undo_len() > 0 {
                        history.undo(&mut doc);
                    }
                    oracle.verify(&mut doc, &format!("{context} undo"));
                }
                12 | 13 => {
                    // Toggle clip (clip groups are where structure edits regroup)
                    let all_ids = all_layer_ids(&doc);
                    let id = all_ids[rng.below(all_ids.len() as u32) as usize];
                    let mut p = doc.layer(id).unwrap().props.clone();
                    p.clip = !p.clip;
                    doc.set_props(id, p);
                    oracle.verify(&mut doc, &format!("{context} toggle_clip"));
                }
                _ => {
                    // History redo
                    if history.redo_len() > 0 {
                        history.redo(&mut doc);
                    }
                    oracle.verify(&mut doc, &format!("{context} redo"));
                }
            }
            // Undo and redo the structure edit just made, so every kind of
            // edit is also diffed by `swap_structure`.
            if history.undo_len() > undo_before && rng.below(2) == 0 {
                history.undo(&mut doc);
                oracle.verify(&mut doc, &format!("{context} undo-now"));
                history.redo(&mut doc);
                oracle.verify(&mut doc, &format!("{context} redo-now"));
            }
        }
    }
}

// ----- Targeted Tests -------------------------------------------------------

#[test]
fn targeted_add_empty_layer_marks_zero_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let active = doc.active();
    paint_tile(&mut doc, active, TileCoord::new(0, 0), [1000, 1000, 1000, 1000]);

    // Drain dirty completely
    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);
    assert!(doc.dirty().is_clean());

    // 1. Adding an empty raster layer marks 0 tiles
    let new_raster = doc.add_raster_layer().unwrap();
    assert!(doc.dirty().is_clean(), "adding empty raster layer marked dirty tiles");
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all);
    assert!(drained.is_empty());

    // 2. Adding an empty folder marks 0 tiles
    let new_folder = doc.add_folder().unwrap();
    assert!(doc.dirty().is_clean(), "adding empty folder marked dirty tiles");
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all);
    assert!(drained.is_empty());

    let _ = (new_raster, new_folder);
}

#[test]
fn targeted_delete_4_tile_layer_marks_at_most_4_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let id = doc.add_raster_layer().unwrap();
    let coords = [
        TileCoord::new(0, 0),
        TileCoord::new(1, 0),
        TileCoord::new(0, 1),
        TileCoord::new(1, 1),
    ];
    for &c in &coords {
        paint_tile(&mut doc, id, c, [2000, 2000, 2000, 2000]);
    }

    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);
    assert!(doc.dirty().is_clean());

    assert!(doc.delete_layer(id));
    drained.clear();
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all, "delete_layer marked all tiles");
    assert_eq!(drained.len(), 4, "delete_layer marked {} tiles instead of 4", drained.len());
    for c in &coords {
        assert!(drained.contains(c));
    }
}

#[test]
fn targeted_delete_layer_with_clip_partners() {
    let mut doc = Document::new(256, 256, 72);
    let base = doc.active();
    let base_coords = [TileCoord::new(0, 0), TileCoord::new(1, 0), TileCoord::new(2, 0)];
    for &c in &base_coords {
        paint_tile(&mut doc, base, c, [1000, 1000, 1000, 1000]);
    }

    let clip = doc.add_raster_layer().unwrap();
    let mut p = doc.layer(clip).unwrap().props.clone();
    p.clip = true;
    doc.set_props(clip, p);

    let clip_coords = [TileCoord::new(0, 0), TileCoord::new(1, 0)];
    for &c in &clip_coords {
        paint_tile(&mut doc, clip, c, [500, 500, 500, 500]);
    }

    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);

    // Deleting clip layer: marks only clip tiles and base tiles (union <= 3 tiles, never page)
    assert!(doc.delete_layer(clip));
    drained.clear();
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all);
    assert!(drained.len() <= 3, "marked {} tiles", drained.len());
}

#[test]
fn targeted_move_layer_marks_only_layer_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let l1 = doc.active();
    let l2 = doc.add_raster_layer().unwrap();
    let l3 = doc.add_raster_layer().unwrap();

    let coords = [TileCoord::new(0, 0), TileCoord::new(1, 1), TileCoord::new(2, 2)];
    for &c in &coords {
        paint_tile(&mut doc, l2, c, [1500, 1500, 1500, 1500]);
    }

    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);

    // Move l2 to index 0 (below l1)
    assert!(doc.move_layer(l2, None, 0));
    drained.clear();
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all, "move_layer marked all tiles");
    assert_eq!(drained.len(), 3, "move_layer marked {} tiles instead of 3", drained.len());
    for c in &coords {
        assert!(drained.contains(c));
    }

    let _ = (l1, l3);
}

#[test]
fn targeted_duplicate_empty_layer_marks_zero_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let id = doc.add_raster_layer().unwrap();
    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);

    let copy = doc.duplicate_layer(id).unwrap();
    assert!(doc.dirty().is_clean(), "duplicating empty layer marked dirty tiles");
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all);
    assert!(drained.is_empty());

    let _ = copy;
}

#[test]
fn targeted_duplicate_4_tile_layer_marks_only_4_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let id = doc.add_raster_layer().unwrap();
    let coords = [
        TileCoord::new(0, 0),
        TileCoord::new(1, 0),
        TileCoord::new(0, 1),
        TileCoord::new(1, 1),
    ];
    for &c in &coords {
        paint_tile(&mut doc, id, c, [2000, 2000, 2000, 2000]);
    }

    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);

    let copy = doc.duplicate_layer(id).unwrap();
    drained.clear();
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all, "duplicate_layer marked all tiles");
    assert_eq!(drained.len(), 4, "duplicate_layer marked {} tiles instead of 4", drained.len());
    for c in &coords {
        assert!(drained.contains(c));
    }

    let _ = copy;
}

#[test]
fn targeted_merge_down_marks_only_merged_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let lower = doc.active();
    for x in 0..4 {
        paint_tile(&mut doc, lower, TileCoord::new(x, 0), [1000, 1000, 1000, 1000]);
    }

    let upper = doc.add_raster_layer().unwrap();
    let upper_coords = [TileCoord::new(1, 0), TileCoord::new(2, 0)];
    for &c in &upper_coords {
        paint_tile(&mut doc, upper, c, [2000, 2000, 2000, 2000]);
    }

    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);

    assert!(doc.merge_down(upper));
    drained.clear();
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all, "merge_down marked all tiles");
    assert_eq!(drained.len(), 2, "merge_down marked {} tiles instead of 2", drained.len());
    for c in &upper_coords {
        assert!(drained.contains(c));
    }
}

#[test]
fn targeted_structure_undo_add_empty_marks_zero_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let mut h = History::default();

    let snap = doc.snapshot_structure();
    let _new_id = doc.add_raster_layer().unwrap();
    h.push(Edit::Structure(Box::new(snap)), &doc);

    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);

    // Undo adding empty layer -> marks 0 tiles
    h.undo(&mut doc);
    assert!(doc.dirty().is_clean(), "undoing add_empty_layer marked tiles");
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all);
    assert!(drained.is_empty());

    // Redo adding empty layer -> marks 0 tiles
    h.redo(&mut doc);
    assert!(doc.dirty().is_clean(), "redoing add_empty_layer marked tiles");
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all);
    assert!(drained.is_empty());
}

#[test]
fn targeted_structure_undo_delete_layer_marks_only_layer_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let mut h = History::default();
    let id = doc.add_raster_layer().unwrap();
    let coords = [
        TileCoord::new(0, 0),
        TileCoord::new(1, 0),
        TileCoord::new(0, 1),
        TileCoord::new(1, 1),
    ];
    for &c in &coords {
        paint_tile(&mut doc, id, c, [2000, 2000, 2000, 2000]);
    }

    let snap = doc.snapshot_structure();
    assert!(doc.delete_layer(id));
    h.push(Edit::Structure(Box::new(snap)), &doc);

    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);

    // Undo delete -> marks the 4 restored tiles
    h.undo(&mut doc);
    drained.clear();
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all, "undoing delete_layer marked all tiles");
    assert_eq!(drained.len(), 4, "undoing delete_layer marked {} tiles instead of 4", drained.len());
    for c in &coords {
        assert!(drained.contains(c));
    }

    // Redo delete -> marks the 4 deleted tiles
    h.redo(&mut doc);
    drained.clear();
    let all = doc.dirty_mut().drain_into(&mut drained);
    assert!(!all, "redoing delete_layer marked all tiles");
    assert_eq!(drained.len(), 4, "redoing delete_layer marked {} tiles instead of 4", drained.len());
    for c in &coords {
        assert!(drained.contains(c));
    }
}

/// A pass-through clipping folder that gains its first child (with pixels in
/// one tile only) must look the same as before everywhere else: the group's
/// unpremultiply/premultiply round trip must not run where no child draws.
#[test]
fn targeted_move_into_empty_pass_through_clip_folder() {
    let mut doc = Document::new(256, 64, 72);
    let base = doc.active();
    let mut rng = Rng(3);
    for x in 0..4 {
        paint_pattern(&mut doc, &mut rng, base, TileCoord::new(x, 0));
    }
    let folder = doc.add_folder().unwrap();
    let mut p = doc.layer(folder).unwrap().props.clone();
    p.clip = true;
    doc.set_props(folder, p);
    let l = doc.add_raster_layer().unwrap();
    paint_pattern(&mut doc, &mut rng, l, TileCoord::new(3, 0));
    let mut oracle = CompositeOracle::new(&doc);
    doc.dirty_mut().drain_into(&mut Vec::new());

    let mut history = History::default();
    let snap = doc.snapshot_structure();
    assert!(doc.move_layer(l, Some(folder), 0));
    history.push(Edit::Structure(Box::new(snap)), &doc);
    oracle.verify(&mut doc, "move into folder");
    history.undo(&mut doc);
    oracle.verify(&mut doc, "undo");
    history.redo(&mut doc);
    oracle.verify(&mut doc, "redo");
}

/// Structure undo/redo of a swap marks only one of the two layers' tiles
/// (either order is a valid diff).
#[test]
fn targeted_structure_undo_move_marks_only_layer_tiles() {
    let mut doc = Document::new(256, 256, 72);
    let l1 = doc.active();
    paint_tile(&mut doc, l1, TileCoord::new(0, 0), [1000, 1000, 1000, 1000]);
    let l2 = doc.add_raster_layer().unwrap();
    paint_tile(&mut doc, l2, TileCoord::new(2, 2), [1500, 1500, 1500, 1500]);
    doc.dirty_mut().drain_into(&mut Vec::new());
    let mut history = History::default();
    let snap = doc.snapshot_structure();
    assert!(doc.shift_layer(l2, -1));
    history.push(Edit::Structure(Box::new(snap)), &doc);
    let mut drained = Vec::new();
    for step in ["do", "undo", "redo"] {
        match step {
            "undo" => drop(history.undo(&mut doc)),
            "redo" => drop(history.redo(&mut doc)),
            _ => {}
        }
        drained.clear();
        assert!(!doc.dirty_mut().drain_into(&mut drained), "{step}");
        assert!(drained == [TileCoord::new(2, 2)] || drained == [TileCoord::new(0, 0)], "{step}: {drained:?}");
    }
}

/// Undoing the add of a pass-through folder's only clip layer turns the
/// folder from isolated back to pass-through: its tiles change although the
/// removed clip is empty.
#[test]
fn targeted_undo_only_clip_of_pass_through_base() {
    let mut doc = Document::new(256, 64, 72);
    let mut rng = Rng(11);
    let base = doc.active();
    let folder = doc.add_folder().unwrap();
    let child = doc.add_raster_layer().unwrap();
    assert!(doc.move_layer(child, Some(folder), 0));
    let mut p = doc.layer(child).unwrap().props.clone();
    p.blend = BlendMode::Multiply;
    doc.set_props(child, p);
    for x in 0..2 {
        paint_pattern(&mut doc, &mut rng, base, TileCoord::new(x, 0));
        paint_pattern(&mut doc, &mut rng, child, TileCoord::new(x, 0));
    }
    doc.set_active(folder);
    let mut history = History::default();
    let snap = doc.snapshot_structure();
    let clip = doc.add_raster_layer().unwrap();
    history.push(Edit::Structure(Box::new(snap)), &doc);
    let mut p = doc.layer(clip).unwrap().props.clone();
    p.clip = true;
    doc.set_props(clip, p);
    let mut oracle = CompositeOracle::new(&doc);
    doc.dirty_mut().drain_into(&mut Vec::new());
    history.undo(&mut doc);
    oracle.verify(&mut doc, "undo");
    history.redo(&mut doc);
    oracle.verify(&mut doc, "redo");
}

/// Moving the only clip off a pass-through folder (and undo/redo of that)
/// turns the folder back to pass-through in all its tiles.
#[test]
fn targeted_move_only_clip_off_pass_through_base() {
    let mut doc = Document::new(256, 64, 72);
    let mut rng = Rng(5);
    let base = doc.active();
    let folder = doc.add_folder().unwrap();
    let child = doc.add_raster_layer().unwrap();
    assert!(doc.move_layer(child, Some(folder), 0));
    let mut p = doc.layer(child).unwrap().props.clone();
    p.blend = BlendMode::Multiply;
    doc.set_props(child, p);
    doc.set_active(folder);
    let clip = doc.add_raster_layer().unwrap();
    let top = doc.add_raster_layer().unwrap();
    for x in 0..3 {
        paint_pattern(&mut doc, &mut rng, base, TileCoord::new(x, 0));
        paint_pattern(&mut doc, &mut rng, child, TileCoord::new(x, 0));
    }
    paint_pattern(&mut doc, &mut rng, clip, TileCoord::new(3, 0));
    paint_pattern(&mut doc, &mut rng, top, TileCoord::new(3, 0));
    let mut p = doc.layer(clip).unwrap().props.clone();
    p.clip = true;
    doc.set_props(clip, p);
    let mut oracle = CompositeOracle::new(&doc);
    doc.dirty_mut().drain_into(&mut Vec::new());
    let mut history = History::default();
    let snap = doc.snapshot_structure();
    assert!(doc.move_layer(clip, None, usize::MAX));
    history.push(Edit::Structure(Box::new(snap)), &doc);
    oracle.verify(&mut doc, "move");
    history.undo(&mut doc);
    oracle.verify(&mut doc, "undo");
    history.redo(&mut doc);
    oracle.verify(&mut doc, "redo");
}
