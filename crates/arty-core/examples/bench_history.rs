//! Undo budget accounting cost (lowend_ux_plan E1, B012).
//!
//! cargo run --release -p arty-core --example bench_history
//! E-cores: cmd /c "start /affinity F000 /wait /b target\release\examples\bench_history.exe"
//!
//! Times `History::push` on a history of 5 whole-layer and 195 stroke
//! steps, for:
//! 1. a 20-tile stroke, without and with the document scan;
//! 2. an A4 350 dpi whole-layer step (2 944 tiles) and a B4 600 dpi one
//!    (12 825 tiles), without and with the scan;
//! 3. a 35-layer structure snapshot (every map still in the document) and
//!    one that deleted a whole A4 layer;
//! 4. the first undo of a whole-layer clear;
//! 5. freeing a dropped whole-layer step inline vs through a release hook
//!    that frees on another thread (as the app does);
//! 6. re-costing a stack of about a 4 GB-tier budget (2 A4 whole-layer and
//!    198 stroke steps of distinct tiles, all costed without the scan)
//!    before a trim.
//!
//! Documents: 15 full A4 layers; 10 B4 600 layers at 40% coverage. Layers
//! the step does not come from share one tile each (their maps are full
//! size, so the scan walks every coordinate) so the bench fits in RAM; the
//! tiles of the step itself are distinct and absent from the document, the
//! scan's worst case (it never stops early). "With the scan" sets the budget
//! to the bytes already held, so the push must scan and then trims the
//! oldest step into a stash (not freed inside the timing).

use std::sync::Arc;
use std::time::Instant;

use arty_core::tile::new_tile;
use arty_core::{Document, Edit, History, LayerId, TileCoord, TileRef};

const A4: (u32, u32, u32) = (2894, 4093, 350);
const B4: (u32, u32, u32) = (6071, 8598, 600);

/// A tile with every page of its pixels written, as painted tiles are.
fn painted_tile(seed: u16) -> TileRef {
    let mut t = new_tile();
    let px = Arc::get_mut(&mut t).expect("fresh tile");
    for (i, p) in px.as_flattened_mut().iter_mut().enumerate() {
        *p = [seed, i as u16, 0, 1 << 15];
    }
    t
}

fn coords(doc: &Document, coverage: u32) -> Vec<TileCoord> {
    let (tw, th) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
    (0..th)
        .flat_map(|y| (0..tw).map(move |x| TileCoord::new(x, y)))
        .filter(|c| ((c.x as u32).wrapping_mul(2_654_435_761) ^ (c.y as u32).wrapping_mul(40_503)) % 100 < coverage)
        .collect()
}

/// `layers` raster layers, each covering `coverage`% of the page with one
/// shared tile.
fn page(size: (u32, u32, u32), layers: usize, coverage: u32) -> Document {
    let mut doc = Document::new(size.0, size.1, size.2);
    let at = coords(&doc, coverage);
    for i in 0..layers {
        let id = if i == 0 { doc.active() } else { doc.add_raster_layer().unwrap() };
        let t = painted_tile(i as u16);
        let (grid, _) = doc.paint_target(id).unwrap();
        for &c in &at {
            grid.insert(c, t.clone());
        }
    }
    doc
}

/// 5 whole-layer and 195 stroke steps (holding the layer's own tiles, so
/// the stack is cheap to build and its costs are exact: a push walks only
/// its own step and never re-costs the stack).
fn history(doc: &Document, layer: LayerId) -> History {
    let mut h = History::with_budget(200, usize::MAX);
    let own: Vec<(TileCoord, TileRef)> =
        doc.layer(layer).unwrap().raster().unwrap().iter().map(|(c, t)| (c, t.clone())).collect();
    for i in 0..200 {
        let n = if i < 5 { own.len() } else { 20 };
        h.push(Edit::Pixels { layer, tiles: own[..n].iter().map(|(c, t)| (*c, Some(t.clone()))).collect() }, doc);
    }
    h
}

fn pixels(layer: LayerId, at: &[TileCoord], tiles: &[TileRef]) -> Edit {
    Edit::Pixels { layer, tiles: at.iter().zip(tiles).map(|(&c, t)| (c, Some(t.clone()))).collect() }
}

/// Median and minimum of `f` over `n` runs, in µs. `f` returns the timed µs.
fn stats(n: usize, mut f: impl FnMut() -> f64) -> (f64, f64) {
    let mut v: Vec<f64> = (0..n).map(|_| f()).collect();
    v.sort_by(f64::total_cmp);
    (v[n / 2], v[0])
}

fn us(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e6
}

fn row(what: &str, walked: usize, doc_tiles: usize, (med, min): (f64, f64)) {
    println!("| {what} | {walked} | {doc_tiles} | {med:.1} | {min:.1} |");
}

/// Time pushing `edit()` on a realistic history, at budget (scan) or not.
/// `warm`: an earlier push of the same size already grew the scratch sets,
/// as in a long session; cold is the first such push of a session.
fn push_case(
    doc: &Document,
    layer: LayerId,
    scan: bool,
    warm: bool,
    n: usize,
    mut edit: impl FnMut() -> Edit,
) -> (f64, f64) {
    stats(n, || {
        let mut h = history(doc, layer);
        let kept = Arc::new(std::sync::Mutex::new(Vec::new()));
        let k = kept.clone();
        h.set_release(Box::new(move |e| k.lock().unwrap().push(e)));
        if warm {
            if scan {
                h.set_budget(h.usage().undo_bytes);
            }
            h.push(edit(), doc);
        }
        if scan {
            h.set_budget(h.usage().undo_bytes);
        }
        let e = edit();
        let t = Instant::now();
        h.push(e, doc);
        let dt = us(t);
        assert!(!scan || h.usage().trimmed > 0, "the scan case trims");
        dt
    })
}

fn doc_tiles(doc: &Document) -> usize {
    doc.pixel_bytes() / size_of::<arty_core::TilePixels>()
}

/// Below normal priority for the calling thread, as `arty_io::lower_thread_priority`
/// (arty-core cannot depend on arty-io).
#[cfg(windows)]
#[allow(unsafe_code)]
fn lower_thread_priority() {
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
    // SAFETY: GetCurrentThread returns a pseudo handle valid for the calling
    // thread; SetThreadPriority only reads it.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

#[cfg(not(windows))]
fn lower_thread_priority() {}

/// Push a small step onto a history holding one whole-layer step of
/// distinct painted tiles, which the push drops for the budget. Returns
/// the push times and, with the hook, the free thread's time per step
/// (waited for before the next run, so runs do not overlap). `low`: the
/// free thread runs below normal priority, as in the app.
fn free_case(
    doc: &Document,
    layer: LayerId,
    tiles: &[TileCoord],
    hook: bool,
    low: bool,
    n: usize,
) -> ((f64, f64), (f64, f64)) {
    // As arty-app studio::undo_release, plus a reply with the time each free took.
    let (tx, rx) = std::sync::mpsc::channel::<Edit>();
    let (done_tx, done) = std::sync::mpsc::channel::<f64>();
    std::thread::spawn(move || {
        if low {
            lower_thread_priority();
        }
        for e in rx {
            let t = Instant::now();
            drop(e);
            let _ = done_tx.send(us(t));
        }
    });
    let mut frees = Vec::new();
    let push = stats(n, || {
        let mut h = History::with_budget(200, usize::MAX);
        if hook {
            let tx = tx.clone();
            h.set_release(Box::new(move |e| {
                let _ = tx.send(e);
            }));
        }
        let fresh: Vec<TileRef> = (0..tiles.len()).map(|i| painted_tile(i as u16)).collect();
        // Costed with the scan, so the next push drops it without a re-cost.
        h.set_budget(0);
        h.push(pixels(layer, tiles, &fresh), doc);
        drop(fresh);
        let small = Edit::Pixels { layer, tiles: vec![(TileCoord::new(0, 0), None)] };
        let t = Instant::now();
        h.push(small, doc);
        let dt = us(t);
        if hook {
            frees.push(done.recv().unwrap());
        }
        dt
    });
    frees.sort_by(f64::total_cmp);
    let free = if hook { (frees[frees.len() / 2], frees[0]) } else { (0.0, 0.0) };
    (push, free)
}

/// A 20-tile stroke push that re-costs a stack of about a 4 GB-tier budget
/// on A4 15 layers before trimming: 2 whole-layer and 198 stroke steps of
/// distinct tiles the document does not hold (no early stop), all pushed
/// without the scan. "Warm": a third whole-layer step was re-costed and
/// trimmed first, then one more stroke pushed without the scan.
fn recost_case() {
    let doc = page(A4, 15, 100);
    let in_doc = doc_tiles(&doc);
    let layer = doc.active();
    let whole = coords(&doc, 100);
    let at20: Vec<TileCoord> = (0..20).map(|x| TileCoord::new(x, 0)).collect();
    let fresh = |n: usize| -> Vec<TileRef> { (0..n).map(|_| new_tile()).collect() };
    let stroke = |h: &mut History| h.push(pixels(layer, &at20, &fresh(20)), &doc);
    let walked = 2 * whole.len() + 199 * 20;
    for warm in [false, true] {
        let r = stats(7, || {
            let mut h = History::with_budget(256, usize::MAX); // no step-limit drops
            let kept = Arc::new(std::sync::Mutex::new(Vec::new()));
            let k = kept.clone();
            h.set_release(Box::new(move |e| k.lock().unwrap().push(e)));
            for _ in 0..2 + warm as usize {
                h.push(pixels(layer, &whole, &fresh(whole.len())), &doc);
            }
            for _ in 0..198 {
                stroke(&mut h);
            }
            if warm {
                h.set_budget(h.usage().undo_bytes);
                stroke(&mut h);
                h.set_budget(usize::MAX);
                stroke(&mut h); // costed without the scan again
            }
            h.set_budget(h.usage().undo_bytes);
            let e = pixels(layer, &at20, &fresh(20));
            let t = Instant::now();
            h.push(e, &doc);
            let dt = us(t);
            assert!(h.usage().trimmed > 0, "the push trims");
            dt
        });
        let label = format!("A4 350, 15 layers: stroke re-costing 200 steps, {}", if warm { "warm" } else { "cold" });
        row(&label, walked, in_doc, r);
    }
}

fn main() {
    println!("| case | tiles walked | doc tiles | median µs | min µs |");
    println!("|---|---:|---:|---:|---:|");
    for (name, size, layers, coverage) in [("A4 350, 15 layers", A4, 15, 100), ("B4 600, 10 layers @40%", B4, 10, 40)] {
        let doc = page(size, layers, coverage);
        let layer = doc.active();
        let in_doc = doc_tiles(&doc);
        let at20: Vec<TileCoord> = (0..20).map(|x| TileCoord::new(x, 0)).collect();
        let t20: Vec<TileRef> = (0..20).map(|i| painted_tile(100 + i)).collect();
        for scan in [false, true] {
            let label = format!("{name}: 20-tile stroke, {}", if scan { "scan" } else { "no scan" });
            row(&label, 20, in_doc, push_case(&doc, layer, scan, true, 31, || pixels(layer, &at20, &t20)));
        }
        let whole = coords(&doc, 100);
        let unique: Vec<TileRef> = (0..whole.len()).map(|i| painted_tile(i as u16)).collect();
        for (scan, warm) in [(false, true), (true, true), (false, false), (true, false)] {
            let label = format!(
                "{name}: whole-layer step, {}, {}",
                if scan { "scan" } else { "no scan" },
                if warm { "warm" } else { "cold (first of the session)" }
            );
            row(&label, whole.len(), in_doc, push_case(&doc, layer, scan, warm, 11, || pixels(layer, &whole, &unique)));
        }
        drop(unique);
        let (inline, _) = free_case(&doc, layer, &whole, false, false, 7);
        let (normal, _) = free_case(&doc, layer, &whole, true, false, 7);
        let (low, freed) = free_case(&doc, layer, &whole, true, true, 7);
        row(&format!("{name}: push dropping a whole-layer step, inline free"), whole.len(), in_doc, inline);
        row(&format!("{name}: push dropping a whole-layer step, hook, normal priority"), whole.len(), in_doc, normal);
        row(&format!("{name}: push dropping a whole-layer step, hook, below normal (app)"), whole.len(), in_doc, low);
        row(&format!("{name}: (free thread: freeing that step)"), whole.len(), in_doc, freed);
    }

    // Structure snapshots and the first undo, on A4 with 35 layers.
    let mut doc = page(A4, 35, 100);
    let in_doc = doc_tiles(&doc);
    let layer = doc.active();
    row(
        "A4 35 layers: structure snapshot, maps in the document",
        0,
        in_doc,
        stats(31, || {
            let mut h = history(&doc, layer);
            let snap = doc.snapshot_structure();
            let t = Instant::now();
            h.push(Edit::Structure(Box::new(snap)), &doc);
            us(t)
        }),
    );
    // A layer of distinct tiles, deleted (snapshot) or cleared (pixels).
    let whole = coords(&doc, 100);
    let u = doc.add_raster_layer().unwrap();
    {
        let (grid, _) = doc.paint_target(u).unwrap();
        for (i, &c) in whole.iter().enumerate() {
            grid.insert(c, painted_tile(i as u16));
        }
    }
    let in_doc = doc_tiles(&doc);
    for scan in [false, true] {
        let label = format!("A4 36 layers: snapshot deleting a whole layer, {}", if scan { "scan" } else { "no scan" });
        let r = stats(11, || {
            let mut h = history(&doc, layer);
            let kept = Arc::new(std::sync::Mutex::new(Vec::new()));
            let k = kept.clone();
            h.set_release(Box::new(move |e| k.lock().unwrap().push(e)));
            let snap = doc.snapshot_structure();
            doc.delete_layer(u);
            if scan {
                h.set_budget(h.usage().undo_bytes);
            }
            let t = Instant::now();
            h.push(Edit::Structure(Box::new(snap)), &doc);
            let dt = us(t);
            h.undo(&mut doc); // put the layer back
            dt
        });
        row(&label, whole.len(), in_doc, r);
    }
    let mut h = history(&doc, layer);
    let r = stats(11, || {
        let grid = doc.layer(u).unwrap().raster().unwrap();
        let tiles: Vec<_> = grid.iter().map(|(c, t)| (c, Some(t.clone()))).collect();
        doc.clear_layer(u);
        h.push(Edit::Pixels { layer: u, tiles }, &doc);
        let t = Instant::now();
        h.undo(&mut doc);
        let dt = us(t);
        h.redo(&mut doc);
        h.undo(&mut doc);
        dt
    });
    row("A4 36 layers: first undo of a whole-layer clear", whole.len(), in_doc, r);
    recost_case();
    let u = h.usage();
    println!();
    let mib = u.undo_bytes >> 20;
    println!("history after the runs: {} undo / {} redo steps, {mib} MiB undo", u.undo_steps, u.redo_steps);
}
