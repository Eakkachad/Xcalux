//! Tiles a layer structure edit invalidates, and what recompositing them
//! costs, against a full-page recomposite (what every structure edit did
//! before E2: `dirty.mark_all()`).
//!
//! cargo run -p arty-core --release --example bench_structure_dirty
//!
//! Pin it to one core (`start /affinity 1 /wait /b …`): on a hybrid CPU an
//! unpinned thread drifts between P- and E-cores mid-run.
//!
//! B4 at 600 dpi (95 × 135 = 12 825 tiles), 30 raster layers: one full-page
//! layer plus 29 rectangles of 4–45 % of the page, every third layer
//! Multiply/Screen/Overlay, layer 20 clipped to layer 19. Each layer shares
//! one tile across its rectangle (composite cost does not depend on it).
//! Single thread, like `bench_composite`.

use std::sync::Arc;
use std::time::Instant;

use arty_core::tile::new_tile_box;
use arty_core::{BlendMode, CompositeScratch, Document, Edit, History, LayerId, TileCoord, TileRef};

const W: u32 = 6071;
const H: u32 = 8598;

fn build() -> (Document, Vec<LayerId>) {
    let mut doc = Document::new(W, H, 600);
    let (tw, th) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut rnd = |n: i32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as i32
    };
    let mut ids = Vec::new();
    for i in 0..30 {
        let id = if i == 0 { doc.active() } else { doc.add_raster_layer().unwrap() };
        let (x0, y0, x1, y1) = if i == 0 {
            (0, 0, tw, th)
        } else {
            let (w, h) = (tw * (20 + rnd(48)) / 100, th * (20 + rnd(48)) / 100);
            let (x, y) = (rnd(tw - w + 1), rnd(th - h + 1));
            (x, y, x + w, y + h)
        };
        let v = 4000 + (i as u16 * 997) % 20000;
        let mut tile = new_tile_box();
        tile.as_flattened_mut().fill([v / 2, v / 3, v / 4, v]);
        let tile: TileRef = Arc::from(tile);
        let (grid, _) = doc.paint_target(id).unwrap();
        for ty in y0..y1 {
            for tx in x0..x1 {
                grid.insert(TileCoord::new(tx, ty), tile.clone());
            }
        }
        let mut p = doc.layer(id).unwrap().props.clone();
        if i % 3 == 1 {
            p.blend = [BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay][(i / 3) % 3];
        }
        p.clip = i == 20;
        doc.set_props(id, p);
        ids.push(id);
    }
    (doc, ids)
}

fn composite_ms(doc: &Document, tiles: &[TileCoord], scratch: &mut CompositeScratch) -> f64 {
    let mut out = new_tile_box();
    let start = Instant::now();
    for &c in tiles {
        doc.composite_tile(c, &mut out, scratch);
    }
    start.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let (mut doc, ids) = build();
    let mut scratch = CompositeScratch::new();
    let all: Vec<TileCoord> = (0..doc.tiles_high() as i32)
        .flat_map(|y| (0..doc.tiles_wide() as i32).map(move |x| TileCoord::new(x, y)))
        .collect();
    let mut history = History::default();
    let mut dirty = Vec::new();
    doc.dirty_mut().drain_into(&mut dirty);
    let tiles_of = |doc: &Document, id| doc.layer(id).unwrap().raster().unwrap().len();
    println!(
        "B4@600dpi {} tiles, 30 layers, {} layer tiles (layer 10: {}, layer 15: {}, layer 19 base / 20 clip: {} / {})",
        all.len(),
        ids.iter().map(|&id| tiles_of(&doc, id)).sum::<usize>(),
        tiles_of(&doc, ids[10]),
        tiles_of(&doc, ids[15]),
        tiles_of(&doc, ids[19]),
        tiles_of(&doc, ids[20]),
    );
    println!("warm-up full page {:.1} ms", composite_ms(&doc, &all, &mut scratch));
    println!("| Edit | Dirty tiles (before = mark_all) | Recomposite ms (after) | Full-page recomposite ms (before) | Edit + diff ms |");
    println!("|---|---|---|---|---|");
    type Op = Box<dyn Fn(&mut Document, &mut History) -> bool>;
    let structure = |f: fn(&mut Document, &[LayerId]) -> bool, ids: Vec<LayerId>| -> Op {
        Box::new(move |doc, history| {
            let snap = doc.snapshot_structure();
            let done = f(doc, &ids);
            if done {
                history.push(Edit::Structure(Box::new(snap)), doc);
            }
            done
        })
    };
    let undo = || -> Op {
        Box::new(|doc, history| {
            let n = history.undo_len();
            history.undo(doc);
            history.undo_len() < n
        })
    };
    let redo = || -> Op {
        Box::new(|doc, history| {
            let n = history.redo_len();
            history.redo(doc);
            history.redo_len() < n
        })
    };
    let ops: Vec<(&str, Op)> = vec![
        ("add empty layer (above layer 15)", structure(|d, i| {
            d.set_active(i[15]);
            d.add_raster_layer().is_some()
        }, ids.clone())),
        ("undo add empty layer", undo()),
        ("add empty folder", structure(|d, _| d.add_folder().is_some(), ids.clone())),
        ("delete layer 15", structure(|d, i| d.delete_layer(i[15]), ids.clone())),
        ("undo delete layer 15", undo()),
        ("redo delete layer 15", redo()),
        ("undo delete layer 15 (again)", undo()),
        ("shift layer 10 up one", structure(|d, i| d.shift_layer(i[10], 1), ids.clone())),
        ("move layer 10 to top", structure(|d, i| d.move_layer(i[10], None, usize::MAX), ids.clone())),
        ("undo move layer 10 to top", undo()),
        ("duplicate layer 10", structure(|d, i| d.duplicate_layer(i[10]).is_some(), ids.clone())),
        ("undo duplicate layer 10", undo()),
        ("merge layer 15 down", structure(|d, i| d.merge_down(i[15]), ids.clone())),
        ("undo merge layer 15 down", undo()),
        ("delete clip base layer 19", structure(|d, i| d.delete_layer(i[19]), ids.clone())),
        ("undo delete clip base layer 19", undo()),
    ];
    for (name, op) in ops {
        let start = Instant::now();
        assert!(op(&mut doc, &mut history), "{name}");
        let edit_ms = start.elapsed().as_secs_f64() * 1e3;
        let marked_all = doc.dirty_mut().drain_into(&mut dirty);
        let tiles: &[TileCoord] = if marked_all { &all } else { &dirty };
        let inc = composite_ms(&doc, tiles, &mut scratch);
        let full = composite_ms(&doc, &all, &mut scratch);
        println!(
            "| {name} | {}{} ({}) | {inc:.1} | {full:.1} | {edit_ms:.2} |",
            tiles.len(),
            if marked_all { " (mark_all)" } else { "" },
            all.len(),
        );
    }
}
