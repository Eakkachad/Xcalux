//! TRANSFORM (m3 §9.4): the floating transform session and `transform_mask`.

use std::sync::Arc;

use arty_core::fix15::ONE_U16;
use arty_core::selection::full_mask;
use arty_core::tile::{TILE_SIZE, TilePixels, new_tile_box};
use arty_core::transform::{Filter, FloatSession, XfParams, XfRefused, XfTarget, transform_mask};
use arty_core::{Document, Edit, History, LayerId, MaskRef, Selection, TileCoord, TileGrid};

const ONE: u32 = ONE_U16 as u32;

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

    /// A valid premultiplied pixel (`c ≤ a`), transparent now and then.
    fn pixel(&mut self) -> [u16; 4] {
        if self.below(5) == 0 {
            return [0; 4];
        }
        let a = 1 + self.below(ONE);
        [self.below(a + 1) as u16, self.below(a + 1) as u16, self.below(a + 1) as u16, a as u16]
    }
}

fn grid(doc: &Document, id: LayerId) -> &TileGrid {
    doc.layer(id).unwrap().raster().unwrap()
}

fn px(g: &TileGrid, x: i32, y: i32) -> [u16; 4] {
    let c = TileCoord::from_pixel(x, y);
    let (ox, oy) = c.origin();
    g.get(c).map_or([0; 4], |t| t[(y - oy) as usize][(x - ox) as usize])
}

/// Fill `[x0, x1) × [y0, y1)` of the active layer with `f(x, y)`.
fn paint(doc: &mut Document, (x0, y0, x1, y1): (i32, i32, i32, i32), mut f: impl FnMut(i32, i32) -> [u16; 4]) -> LayerId {
    let id = doc.active();
    let (g, _) = doc.paint_target(id).unwrap();
    for y in y0..y1 {
        for x in x0..x1 {
            let c = TileCoord::from_pixel(x, y);
            let (ox, oy) = c.origin();
            g.get_mut_or_create(c)[(y - oy) as usize][(x - ox) as usize] = f(x, y);
        }
    }
    id
}

fn random_doc(w: u32, h: u32, rect: (i32, i32, i32, i32), seed: u64) -> (Document, LayerId) {
    let mut doc = Document::new(w, h, 72);
    let mut rng = Rng(seed);
    let id = paint(&mut doc, rect, |_, _| rng.pixel());
    (doc, id)
}

/// Every pixel of two grids is equal (a missing tile reads as transparent).
fn assert_same_pixels(a: &TileGrid, b: &TileGrid, what: &str) {
    let zero = new_tile_box();
    let coords: std::collections::BTreeSet<TileCoord> = a.coords().chain(b.coords()).collect();
    for c in coords {
        let (ta, tb): (&TilePixels, &TilePixels) = (a.get(c).unwrap_or(&zero), b.get(c).unwrap_or(&zero));
        if ta != tb {
            let i = ta.as_flattened().iter().zip(tb.as_flattened()).position(|(p, q)| p != q).unwrap();
            panic!("{what}: tile {c:?} pixel ({}, {}) differs: {:?} vs {:?}", i % 64, i / 64, ta.as_flattened()[i], tb.as_flattened()[i]);
        }
    }
}

fn assert_valid(g: &TileGrid, what: &str) {
    for (c, t) in g.iter() {
        for p in t.as_flattened() {
            assert!(p[3] as u32 <= ONE && p[..3].iter().all(|&v| v <= p[3]), "{what}: {c:?} {p:?}");
        }
    }
}

fn commit(doc: &mut Document, id: LayerId, f: impl FnOnce(XfParams) -> XfParams, filter: Filter) -> Option<Edit> {
    let mut s = FloatSession::begin(doc, id).unwrap();
    let p = f(s.params());
    s.set_params(p);
    s.commit(doc, filter)
}

#[test]
fn tr01_identity_commit_is_no_step_and_keeps_every_tile() {
    let (mut doc, id) = random_doc(300, 200, (5, 7, 250, 130), 1);
    let before = grid(&doc, id).clone();
    let rev = doc.revision();
    let s = FloatSession::begin(&mut doc, id).unwrap();
    assert_eq!(s.target(), XfTarget::Layer);
    assert!(s.commit(&mut doc, Filter::Bicubic).is_none());
    assert!(grid(&doc, id).shares_storage(&before));
    assert_eq!(doc.revision(), rev, "nothing was written");

    // After previews, coming back to the identity still restores every Arc.
    let mut s = FloatSession::begin(&mut doc, id).unwrap();
    let id_params = s.params();
    s.preview(&mut doc, XfParams { theta: 0.4, t: [20.0, 3.0], ..id_params }, Filter::Nearest);
    assert!(!grid(&doc, id).shares_storage(&before));
    s.set_params(id_params);
    assert!(s.commit(&mut doc, Filter::Bicubic).is_none());
    for (c, t) in before.iter() {
        assert!(Arc::ptr_eq(grid(&doc, id).get_ref(c).unwrap(), t), "{c:?}");
    }
    assert_eq!(grid(&doc, id).len(), before.len());
}

#[test]
fn tr02_integer_translation_is_exact_and_tile_moves_share_arcs() {
    let (mut doc, id) = random_doc(400, 300, (10, 20, 170, 150), 2);
    let before = grid(&doc, id).clone();
    for filter in [Filter::Bicubic, Filter::Bilinear, Filter::Nearest] {
        let mut d = Document::new(400, 300, 72);
        let (g, _) = d.paint_target(d.active()).unwrap();
        *g = before.clone();
        let lid = d.active();
        let edit = commit(&mut d, lid, |p| XfParams { t: [13.0, -7.0], ..p }, filter).unwrap();
        assert!(matches!(edit, Edit::Pixels { .. }));
        let g = grid(&d, lid);
        for y in 0..300 {
            for x in 0..400 {
                assert_eq!(px(g, x + 13, y - 7), px(&before, x, y), "{filter:?} ({x}, {y})");
            }
        }
    }

    let edit = commit(&mut doc, id, |p| XfParams { t: [128.0, -64.0], ..p }, Filter::Bicubic);
    assert!(edit.is_some());
    let g = grid(&doc, id);
    assert_eq!(g.len(), before.len());
    for (c, t) in before.iter() {
        let moved = g.get_ref(TileCoord::new(c.x + 2, c.y - 1)).unwrap();
        assert!(Arc::ptr_eq(moved, t), "{c:?} moved by its Arc");
    }
}

#[test]
fn tr03_quarter_turns_and_double_flips_are_exact() {
    // A 64 × 40 block centred on (42, 40): pixel centres map onto pixel centres.
    let (mut doc, id) = random_doc(160, 128, (10, 20, 74, 60), 3);
    let before = grid(&doc, id).clone();
    for turn in 0..4 {
        let edit = commit(&mut doc, id, |p| XfParams { theta: std::f64::consts::FRAC_PI_2, ..p }, Filter::Nearest);
        assert!(edit.is_some(), "turn {turn}");
        if turn == 0 {
            // (10, 20) is the top-left corner; a clockwise quarter turn about
            // (42, 40) puts it at the top-right of the 40 × 64 block.
            assert_eq!(px(grid(&doc, id), 61, 8), px(&before, 10, 20));
        }
    }
    assert_same_pixels(grid(&doc, id), &before, "four quarter turns");

    for s in [[-1.0, 1.0], [1.0, -1.0]] {
        for _ in 0..2 {
            commit(&mut doc, id, |p| XfParams { s, ..p }, Filter::Bicubic).unwrap();
        }
        assert_same_pixels(grid(&doc, id), &before, "flip twice");
    }
}

#[test]
fn tr04_bilinear_keeps_constants_and_pixel_centres() {
    let v = [9000, 12000, 3000, 20000];
    let mut doc = Document::new(512, 512, 72);
    let id = paint(&mut doc, (0, 0, 256, 256), |_, _| v);
    let mut s = FloatSession::begin(&mut doc, id).unwrap();
    let p = XfParams { s: [1.37, 0.81], theta: 0.3, t: [40.0, 10.0], ..s.params() };
    s.preview(&mut doc, p, Filter::Bilinear);
    let inv = p.affine().inverse().unwrap();
    let g = grid(&doc, id);
    let mut checked = 0;
    for y in 0..512 {
        for x in 0..512 {
            let q = inv.apply([x as f64 + 0.5, y as f64 + 0.5]);
            if q.iter().all(|&c| (1.5..254.5).contains(&c)) {
                assert_eq!(px(g, x, y), v, "({x}, {y})");
                checked += 1;
            }
        }
    }
    assert!(checked > 40_000, "{checked}");
    s.cancel(&mut doc);

    // One opaque pixel at (10, 10) scaled 2× about the origin covers
    // [20, 22)²: its weight is centred on (21, 21), not half a pixel off.
    for filter in [Filter::Nearest, Filter::Bilinear, Filter::Bicubic] {
        let mut doc = Document::new(64, 64, 72);
        let id = paint(&mut doc, (10, 10, 11, 11), |_, _| [ONE_U16; 4]);
        commit(&mut doc, id, |p| XfParams { s: [2.0, 2.0], pivot: [0.0, 0.0], ..p }, filter).unwrap();
        let g = grid(&doc, id);
        let (mut sum, mut cx, mut cy) = (0.0, 0.0, 0.0);
        for y in 0..64 {
            for x in 0..64 {
                let a = px(g, x, y)[3] as f64;
                sum += a;
                cx += a * (x as f64 + 0.5);
                cy += a * (y as f64 + 0.5);
            }
        }
        let (cx, cy) = (cx / sum, cy / sum);
        assert!((cx - 21.0).abs() < 1e-3 && (cy - 21.0).abs() < 1e-3, "{filter:?}: centroid ({cx}, {cy})");
        if filter == Filter::Nearest {
            for (x, y) in [(20, 20), (21, 20), (20, 21), (21, 21)] {
                assert_eq!(px(g, x, y), [ONE_U16; 4]);
            }
            assert_eq!(px(g, 22, 21), [0; 4]);
            assert_eq!(px(g, 19, 21), [0; 4]);
        }
    }
}

#[test]
fn tr05_bicubic_keeps_premultiplied_order_on_a_checker() {
    let mut rng = Rng(5);
    for round in 0..8 {
        let mut doc = Document::new(256, 256, 72);
        let id = paint(&mut doc, (30, 30, 150, 140), |x, y| {
            if (x + y) % 2 == 0 { [ONE_U16, ONE_U16, ONE_U16, ONE_U16] } else if x % 3 == 0 { [0, 0, 0, ONE_U16] } else { [0; 4] }
        });
        let p = |p: XfParams| XfParams {
            s: [0.3 + 2.0 * rng.unit(), (0.3 + 2.0 * rng.unit()) * if rng.below(2) == 0 { 1.0 } else { -1.0 }],
            theta: rng.unit() * 6.3,
            t: [rng.unit() * 40.0 - 20.0, rng.unit() * 40.0 - 20.0],
            ..p
        };
        commit(&mut doc, id, p, Filter::Bicubic).unwrap();
        assert_valid(grid(&doc, id), &format!("round {round}"));
    }
}

#[test]
fn tr06_downscale_uses_the_pyramid() {
    // Period-3 stripes alias badly when point-sampled at 1/8.
    let mut doc = Document::new(512, 512, 72);
    let id = paint(&mut doc, (0, 0, 512, 512), |x, y| {
        let v = [ONE_U16, 9000, 0][((x + 2 * y) % 3) as usize];
        [v / 2, v / 3, 0, v]
    });
    let before = grid(&doc, id).clone();
    commit(&mut doc, id, |p| XfParams { s: [0.125, 0.125], pivot: [0.0, 0.0], ..p }, Filter::Bicubic).unwrap();
    let g = grid(&doc, id);
    let (mut mean_out, mut mean_ref, mut se) = (0.0, 0.0, 0.0);
    for y in 0..64 {
        for x in 0..64 {
            let mut box_a = 0.0;
            for yy in 0..8 {
                for xx in 0..8 {
                    box_a += px(&before, x * 8 + xx, y * 8 + yy)[3] as f64;
                }
            }
            box_a /= 64.0;
            let a = px(g, x, y)[3] as f64;
            mean_out += a;
            mean_ref += box_a;
            se += (a - box_a) * (a - box_a);
        }
    }
    let rms = (se / 4096.0).sqrt();
    assert!((mean_out / mean_ref - 1.0).abs() < 0.01, "mean {mean_out} vs {mean_ref}");
    assert!(rms < 0.005 * ONE as f64, "rms {rms}");
    assert_eq!(px(g, 64, 10), [0; 4], "nothing past the shrunk page");
}

#[test]
fn tr07_cancel_restores_every_arc_and_marks_what_moved() {
    let (mut doc, id) = random_doc(512, 256, (0, 0, 130, 70), 7);
    let before = grid(&doc, id).clone();
    let mut s = FloatSession::begin(&mut doc, id).unwrap();
    s.preview(&mut doc, XfParams { t: [200.5, 100.0], ..s.params() }, Filter::Bilinear);
    let mut drained = Vec::new();
    doc.dirty_mut().drain_into(&mut drained);
    s.cancel(&mut doc);
    assert!(grid(&doc, id).shares_storage(&before));
    doc.dirty_mut().drain_into(&mut drained);
    for c in before.coords() {
        assert!(drained.contains(&c), "source tile {c:?} marked");
    }
    // (200.5, 100) moves (0, 0)..(130, 70) to cover tiles 3..=5 × 1..=2.
    for c in [TileCoord::new(3, 1), TileCoord::new(5, 2)] {
        assert!(drained.contains(&c), "preview tile {c:?} marked");
    }
}

/// Columns 64..=127 fully selected, 128..=191 half (128) on rows 64..=127.
fn two_tile_selection() -> Selection {
    let mut sel = Selection::new();
    sel.insert_tile(TileCoord::new(1, 1), full_mask().clone());
    let mut m: MaskRef = Arc::new([[0; TILE_SIZE]; TILE_SIZE]);
    for row in Arc::make_mut(&mut m).iter_mut() {
        row[..32].fill(128);
        row[32..40].fill(255);
    }
    sel.insert_tile(TileCoord::new(2, 1), m);
    sel
}

#[test]
fn tr08_selection_commit_is_one_batch_step_undo_and_redo() {
    let (mut doc, id) = random_doc(512, 384, (40, 40, 300, 200), 8);
    let sel = two_tile_selection();
    doc.swap_selection(sel.clone());
    let before = grid(&doc, id).clone();
    let mut s = FloatSession::begin(&mut doc, id).unwrap();
    assert_eq!(s.target(), XfTarget::Selection);
    s.preview(&mut doc, XfParams { t: [37.0, 85.0], theta: 0.2, ..s.params() }, Filter::Nearest);
    let edit = s.commit(&mut doc, Filter::Bicubic).unwrap();
    let Edit::Batch(ref parts) = edit else { panic!("a batch") };
    assert!(matches!(parts[..], [Edit::Pixels { .. }, Edit::Selection(_)]));
    let (after, after_sel) = (grid(&doc, id).clone(), doc.selection().clone());
    assert!(!after_sel.shares_storage(&sel));

    let mut h = History::default();
    h.push(edit);
    h.undo(&mut doc);
    assert!(!h.can_undo(), "one step");
    for (c, t) in before.iter() {
        assert!(Arc::ptr_eq(grid(&doc, id).get_ref(c).unwrap(), t), "{c:?}");
    }
    assert_eq!(grid(&doc, id).len(), before.len());
    assert!(doc.selection().shares_storage(&sel));
    h.redo(&mut doc);
    assert_same_pixels(grid(&doc, id), &after, "redo");
    assert!(doc.selection().shares_storage(&after_sel));
}

#[test]
fn tr09_selection_lift_leaves_a_hole_and_moves_the_mask() {
    let (mut doc, id) = random_doc(512, 256, (0, 0, 220, 200), 9);
    let sel = two_tile_selection();
    doc.swap_selection(sel.clone());
    let before = grid(&doc, id).clone();
    let edit = commit(&mut doc, id, |p| XfParams { t: [230.0, 3.0], ..p }, Filter::Bicubic).unwrap();
    assert!(matches!(edit, Edit::Batch(_)));
    let g = grid(&doc, id);
    for y in 0..256 {
        for x in 0..512 {
            let m = sel.value(x, y) as u32;
            let o = px(&before, x, y);
            let lifted = o.map(|v| ((v as u32 * m + 127) / 255) as u16);
            let hole = [0, 1, 2, 3].map(|k| o[k] - lifted[k]);
            if x < 220 {
                assert_eq!(px(g, x, y), hole, "hole at ({x}, {y})");
            }
            if m > 0 {
                assert_eq!(px(g, x + 230, y + 3), lifted, "moved at ({x}, {y})");
            }
        }
    }
    let xf = XfParams { t: [230.0, 3.0], ..XfParams::identity([0.0; 2]) }.affine();
    let want = transform_mask(&sel, &xf, 512, 256);
    for y in 0..256 {
        for x in 0..512 {
            assert_eq!(doc.selection().value(x, y), want.value(x, y));
            let src = if x >= 230 && y >= 3 { sel.value(x - 230, y - 3) } else { 0 };
            assert_eq!(want.value(x, y), src, "({x}, {y})");
        }
    }
    // Rotated masks stay within 0..=255 and keep their area.
    let rot = XfParams { theta: 0.5, ..XfParams::identity([128.0, 96.0]) }.affine();
    let turned = transform_mask(&sel, &rot, 512, 256);
    let mass = |s: &Selection| (0..256).flat_map(|y| (0..512).map(move |x| (x, y))).map(|(x, y)| s.value(x, y) as f64).sum::<f64>();
    assert!((mass(&turned) / mass(&sel) - 1.0).abs() < 0.01);
}

#[test]
fn tr10_refusals() {
    let mut doc = Document::new(128, 128, 72);
    let folder = doc.add_folder().unwrap();
    assert_eq!(FloatSession::begin(&mut doc, folder).err(), Some(XfRefused::Folder));
    let empty = doc.add_raster_layer().unwrap();
    assert_eq!(FloatSession::begin(&mut doc, empty).err(), Some(XfRefused::Empty));
    let id = paint(&mut doc, (0, 0, 10, 10), |_, _| [ONE_U16; 4]);
    let mut p = doc.layer(id).unwrap().props.clone();
    p.locked = true;
    doc.set_props(id, p);
    assert_eq!(FloatSession::begin(&mut doc, id).err(), Some(XfRefused::Locked));
    let mut p = doc.layer(id).unwrap().props.clone();
    p.locked = false;
    doc.set_props(id, p);
    assert!(FloatSession::begin(&mut doc, id).is_ok());
    // A selection over nothing painted is empty too.
    let mut sel = Selection::new();
    sel.insert_tile(TileCoord::new(1, 1), full_mask().clone());
    doc.swap_selection(sel);
    assert_eq!(FloatSession::begin(&mut doc, id).err(), Some(XfRefused::Empty));
    assert_eq!(FloatSession::begin(&mut doc, LayerId(999)).err(), Some(XfRefused::Unsupported));
}

#[test]
fn tr11_previews_restore_base_and_match_a_direct_commit() {
    let rect = (20, 30, 200, 150);
    let last = |p: XfParams| XfParams { t: [-15.0, 160.0], s: [0.7, 1.3], theta: -0.6, ..p };
    for with_sel in [false, true] {
        let setup = || {
            let (mut doc, id) = random_doc(512, 512, rect, 11);
            if with_sel {
                doc.swap_selection(two_tile_selection());
            }
            (doc, id)
        };
        let (mut doc, id) = setup();
        let base_px = |x: i32, y: i32, before: &TileGrid, sel: &Selection| {
            let m = sel.value(x, y) as u32;
            px(before, x, y).map(|v| v - ((v as u32 * m + 127) / 255) as u16)
        };
        let before = grid(&doc, id).clone();
        let sel = doc.selection().clone();
        let mut s = FloatSession::begin(&mut doc, id).unwrap();
        s.preview(&mut doc, XfParams { t: [300.0, 0.0], ..s.params() }, Filter::Nearest);
        s.preview(&mut doc, last(s.params()), Filter::Bilinear);
        // Everything the first preview wrote right of x = 380 is base again.
        let g = grid(&doc, id);
        for y in 0..512 {
            for x in 380..512 {
                assert_eq!(px(g, x, y), base_px(x, y, &before, &sel), "({x}, {y}) with_sel={with_sel}");
            }
        }
        s.commit(&mut doc, Filter::Bicubic).unwrap();

        let (mut direct, did) = setup();
        commit(&mut direct, did, last, Filter::Bicubic).unwrap();
        assert_same_pixels(grid(&doc, id), grid(&direct, did), "preview, preview, commit");
        if with_sel {
            for y in 0..512 {
                for x in 0..512 {
                    assert_eq!(doc.selection().value(x, y), direct.selection().value(x, y));
                }
            }
        }

        // Later renders draw into the earlier ones' tiles: every path must
        // overwrite all of their pixels (rotate, then shifts by whole px
        // and whole tiles, then a sampled commit).
        let (mut doc, id) = setup();
        let mut s = FloatSession::begin(&mut doc, id).unwrap();
        let p0 = s.params();
        s.preview(&mut doc, XfParams { theta: 0.7, ..p0 }, Filter::Bicubic);
        s.preview(&mut doc, XfParams { t: [13.0, 5.0], ..p0 }, Filter::Bicubic);
        let (mut direct, did) = setup();
        commit(&mut direct, did, |p| XfParams { t: [13.0, 5.0], ..p }, Filter::Bicubic).unwrap();
        assert_same_pixels(grid(&doc, id), grid(&direct, did), "shift after rotate");
        s.preview(&mut doc, XfParams { t: [64.0, -128.0], ..p0 }, Filter::Bicubic);
        s.commit(&mut doc, Filter::Bilinear).unwrap();
        let (mut direct, did) = setup();
        commit(&mut direct, did, |p| XfParams { t: [64.0, -128.0], ..p }, Filter::Bilinear).unwrap();
        assert_same_pixels(grid(&doc, id), grid(&direct, did), "tile shift after shift");
    }
}

#[test]
fn tr12_destination_is_clamped_to_the_page_plus_one_page() {
    let mut doc = Document::new(256, 192, 72);
    let id = paint(&mut doc, (0, 0, 256, 192), |_, _| [100, 100, 100, 200]);
    let mut s = FloatSession::begin(&mut doc, id).unwrap();
    s.preview(&mut doc, XfParams { s: [10.0, 10.0], ..s.params() }, Filter::Nearest);
    let inside = |g: &TileGrid| g.coords().all(|c| (-4..8).contains(&c.x) && (-3..6).contains(&c.y));
    assert!(inside(grid(&doc, id)));
    assert_eq!(grid(&doc, id).len(), 12 * 9, "the clamped area is fully covered");
    s.commit(&mut doc, Filter::Bilinear).unwrap();
    assert!(inside(grid(&doc, id)));
}
