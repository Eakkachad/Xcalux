//! FRAMES: frame border folders (rasterization, borders, cutting, editing,
//! compositing, clipping, dirty marking, history) and page presets.

use std::sync::Arc;

use arty_core::fix15::ONE_U16 as O;
use arty_core::frame::{MaskTile, add_frame_folder};
use arty_core::page::MANGA_PRESETS;
use arty_core::tile::{fill_tile, new_tile_box};
use arty_core::{
    BlendMode, BorderStyle, CompositeScratch, Cov, Document, Edit, Frame, FrameShape, History, LayerId, PageSetup,
    Panel, RectF, TILE_SIZE, TileCoord, TilePixels,
};

const RED: [u16; 4] = [O, 0, 0, O];
const BLUE: [u16; 4] = [0, 0, O, O];
const GREEN: [u16; 4] = [0, O, 0, O];
const GRAY: [u16; 4] = [O / 2, O / 2, O / 2, O];
const BLACK: [u16; 4] = [0, 0, 0, O];

fn rect(x: f32, y: f32, w: f32, h: f32) -> Panel {
    Panel::rect(RectF { x, y, w, h }).unwrap()
}

fn shape(panels: Vec<Panel>, width: f32, color: [u16; 4]) -> FrameShape {
    FrameShape { panels, border: BorderStyle { width, color } }
}

/// Coverage 0..=255 of pixel `(x, y)` from content (or border) codes.
fn coverage(f: &Frame, border: bool, x: i32, y: i32) -> u8 {
    let c = TileCoord::from_pixel(x, y);
    let cov = if border { f.border(c) } else { f.content(c) };
    match cov {
        Cov::None => 0,
        Cov::Full => 255,
        Cov::Partial(m) => m[(y - c.y * 64) as usize][(x - c.x * 64) as usize],
    }
}

/// Σ coverage / 255 over a `w`×`h` page.
fn total(f: &Frame, border: bool, w: i32, h: i32) -> f64 {
    let mut sum = 0.0;
    for y in 0..h {
        for x in 0..w {
            sum += coverage(f, border, x, y) as f64 / 255.0;
        }
    }
    sum
}

fn rotated_square(cx: f32, cy: f32, side: f32, deg: f32) -> Panel {
    let (s, c) = deg.to_radians().sin_cos();
    let h = side / 2.0;
    let pts = [[-h, -h], [h, -h], [h, h], [-h, h]].map(|[x, y]| [cx + x * c - y * s, cy + x * s + y * c]);
    Panel::new(pts.to_vec()).unwrap()
}

#[test]
fn fr01_raster_coverage() {
    // An integer-aligned rect: coverage is exactly 0 or 255.
    let f = Frame::build(shape(vec![rect(60.0, 10.0, 140.0, 150.0)], 0.0, BLACK), 256, 256);
    for y in 0..256 {
        for x in 0..256 {
            let want = if (60..200).contains(&x) && (10..160).contains(&y) { 255 } else { 0 };
            assert_eq!(coverage(&f, false, x, y), want, "({x}, {y})");
        }
    }
    // Full and Outside tiles are classified right; Partial only along edges.
    for ty in 0..4 {
        for tx in 0..4 {
            let c = TileCoord::new(tx, ty);
            let (x0, y0) = (tx * 64, ty * 64);
            let inside = x0 >= 60 && x0 + 64 <= 200 && y0 >= 10 && y0 + 64 <= 160;
            let outside = x0 + 64 <= 60 || x0 >= 200 || y0 + 64 <= 10 || y0 >= 160;
            match f.content(c) {
                Cov::Full => assert!(inside, "{c:?} full"),
                Cov::None => assert!(outside, "{c:?} outside"),
                Cov::Partial(_) => assert!(!inside && !outside, "{c:?} partial off the edges"),
            }
        }
    }
    assert_eq!(f.border(TileCoord::new(1, 1)), Cov::None, "no border at width 0");
    assert_eq!(f.content(TileCoord::new(1, 1)), Cov::Full);

    // Half-pixel offset: the edge pixels are half covered.
    let f = Frame::build(shape(vec![rect(60.5, 10.5, 140.0, 150.0)], 0.0, BLACK), 256, 256);
    for (x, y) in [(60, 50), (200, 50), (100, 10), (100, 160)] {
        let v = coverage(&f, false, x, y) as i32;
        assert!((v - 128).abs() <= 1, "({x}, {y}) = {v}");
    }
    let corner = coverage(&f, false, 60, 10) as i32;
    assert!((corner - 64).abs() <= 1, "a quarter-covered corner: {corner}");
    assert_eq!(coverage(&f, false, 61, 11), 255);

    // A rotated panel's coverage matches its area.
    let p = rotated_square(130.0, 120.0, 150.0, 30.0);
    let area = p.area() as f64;
    let f = Frame::build(shape(vec![p], 0.0, BLACK), 256, 256);
    let got = total(&f, false, 256, 256);
    assert!((got - area).abs() / area < 0.005, "{got} vs {area}");
}

#[test]
fn fr02_border() {
    let w = 6.0;
    let p = rotated_square(128.0, 128.0, 160.0, 20.0);
    let ring = p.area() as f64 - p.inset(w).unwrap().area() as f64;
    let f = Frame::build(shape(vec![p], w, BLACK), 256, 256);
    let got = total(&f, true, 256, 256);
    assert!((got - ring).abs() / ring < 0.005, "ring {got} vs {ring}");
    // The border lies inside the panel.
    for y in 0..256 {
        for x in 0..256 {
            assert!(coverage(&f, true, x, y) <= coverage(&f, false, x, y).saturating_add(1), "({x}, {y})");
        }
    }

    // A panel thinner than 2w is all border.
    let thin = rect(40.0, 40.0, 10.0, 100.0);
    assert!(thin.inset(w).is_none());
    let f = Frame::build(shape(vec![thin], w, BLACK), 128, 192);
    for y in 0..192 {
        for x in 0..128 {
            assert_eq!(coverage(&f, true, x, y), coverage(&f, false, x, y), "({x}, {y})");
        }
    }

    // The inset meets at mitered corners.
    let tri = Panel::new(vec![[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]]).unwrap();
    let inner = tri.inset(10.0).unwrap();
    assert_eq!(inner.points().len(), 3);
    let near = |q: [f32; 2]| inner.points().iter().any(|p| (p[0] - q[0]).hypot(p[1] - q[1]) < 1e-3);
    let s = 10.0 * std::f32::consts::SQRT_2;
    assert!(near([10.0, 10.0]), "{:?}", inner.points());
    assert!(near([100.0 - s - 10.0, 10.0]), "{:?}", inner.points());
    assert!(near([10.0, 100.0 - s - 10.0]), "{:?}", inner.points());
    let r = rect(10.0, 20.0, 100.0, 50.0).inset(5.0).unwrap();
    assert_eq!(r.bounds(), RectF { x: 15.0, y: 25.0, w: 90.0, h: 40.0 });
}

#[test]
fn fr03_cutting() {
    let frame = shape(vec![rect(0.0, 0.0, 200.0, 300.0)], 2.0, BLACK);
    // A horizontal cut: two rects exactly gap_h apart.
    let (cut, bs) = frame.cut([-10.0, 150.0], [210.0, 150.0], 20.0, 8.0).unwrap();
    assert_eq!(cut.panels.len(), 2);
    assert_eq!(bs, [1]);
    let mut rects: Vec<RectF> = cut.panels.iter().map(Panel::bounds).collect();
    rects.sort_by(|a, b| a.y.total_cmp(&b.y));
    assert_eq!(rects[0], RectF { x: 0.0, y: 0.0, w: 200.0, h: 140.0 });
    assert_eq!(rects[1], RectF { x: 0.0, y: 160.0, w: 200.0, h: 140.0 });
    assert_eq!(rects[1].y - (rects[0].y + rects[0].h), 20.0);
    // A vertical cut uses the left/right gutter.
    let (cut, _) = frame.cut([100.0, -5.0], [100.0, 305.0], 20.0, 8.0).unwrap();
    let mut xs: Vec<RectF> = cut.panels.iter().map(Panel::bounds).collect();
    xs.sort_by(|a, b| a.x.total_cmp(&b.x));
    assert_eq!(xs[1].x - (xs[0].x + xs[0].w), 8.0);

    // An angled cut: convex pieces, area conserved minus the gap band.
    let (a, b) = ([-20.0, 100.0], [220.0, 180.0]);
    let p = &frame.panels[0];
    let g = 20.0;
    let (cut, bs) = frame.cut(a, b, g, 8.0).unwrap();
    assert_eq!((cut.panels.len(), bs.len()), (2, 1));
    for q in &cut.panels {
        assert_eq!(Panel::new(q.points().to_vec()).as_ref(), Some(q), "a valid convex piece");
    }
    let (c0, c1) = p.chord(a, [b[0] - a[0], b[1] - a[1]]).unwrap();
    let chord = (c1[0] - c0[0]).hypot(c1[1] - c0[1]);
    let pieces: f32 = cut.panels.iter().map(Panel::area).sum();
    let want = p.area() - g * chord;
    assert!((pieces - want).abs() / want < 1e-4, "{pieces} vs {want}");

    // A segment that misses changes nothing.
    assert!(frame.cut([300.0, 0.0], [400.0, 50.0], 20.0, 8.0).is_none());
    assert!(frame.cut([-50.0, -10.0], [250.0, -10.0], 20.0, 8.0).is_none(), "along the outside");
    // Only crossed panels are split.
    let two = shape(vec![rect(0.0, 0.0, 100.0, 100.0), rect(200.0, 0.0, 100.0, 100.0)], 2.0, BLACK);
    let (cut, bs) = two.cut([-10.0, 50.0], [110.0, 50.0], 10.0, 10.0).unwrap();
    assert_eq!(cut.panels.len(), 3);
    assert_eq!(bs, [1]);
    assert_eq!(cut.panels[2], two.panels[1]);

    // Tiny pieces and pieces narrower than twice the border are dropped.
    let narrow = shape(vec![rect(0.0, 0.0, 10.0, 100.0)], 0.0, BLACK);
    let (cut, bs) = narrow.cut([-5.0, 0.6], [15.0, 0.6], 0.4, 0.4).unwrap();
    assert_eq!(cut.panels.len(), 1, "a 10 × 0.4 sliver is dropped");
    assert!(bs.is_empty() || cut.panels[bs[0]].area() >= 16.0);
    let bordered = shape(vec![rect(0.0, 0.0, 200.0, 200.0)], 4.0, BLACK);
    let (cut, _) = bordered.cut([-5.0, 7.0], [205.0, 7.0], 2.0, 2.0).unwrap();
    assert_eq!(cut.panels.len(), 1, "a 6 px strip under a 4 px border is dropped");
    assert_eq!(cut.panels[0].bounds().y, 8.0);

    // A gutter wider than the panel would drop both pieces: the panel is
    // not divided, and with nothing divided there is no cut.
    let caption = shape(vec![rect(0.0, 0.0, 100.0, 60.0)], 2.0, BLACK);
    assert!(caption.cut([-10.0, 30.0], [110.0, 30.0], 70.0, 8.0).is_none());
    // Across it and a tall panel: only the tall one is divided.
    let both = shape(vec![rect(0.0, 0.0, 100.0, 60.0), rect(150.0, 0.0, 150.0, 300.0)], 2.0, BLACK);
    let (cut, _) = both.cut([-10.0, 30.0], [310.0, 30.0], 70.0, 8.0).unwrap();
    assert_eq!(cut.panels[0], both.panels[0], "the caption is kept whole");
    assert_eq!(cut.panels.len(), 2);
    assert!((cut.panels[1].bounds().y - 65.0).abs() < 1e-3, "{:?}", cut.panels[1].bounds());
}

fn dir(p: &Panel, i: usize) -> [f32; 2] {
    let (a, b) = p.edge(i);
    let l = (b[0] - a[0]).hypot(b[1] - a[1]);
    [(b[0] - a[0]) / l, (b[1] - a[1]) / l]
}

#[test]
fn fr04_editing() {
    let trap = Panel::new(vec![[0.0, 0.0], [200.0, 0.0], [160.0, 100.0], [30.0, 100.0]]).unwrap();
    for (edge, d) in [(0usize, -10.0f32), (0, 15.0), (1, -20.0), (2, 5.0)] {
        let moved = trap.with_edge_offset(edge, d).unwrap();
        let n = trap.points().len();
        for i in [edge, (edge + n - 1) % n, (edge + 1) % n] {
            let (u, v) = (dir(&trap, i), dir(&moved, i));
            assert!((u[0] - v[0]).abs() < 1e-4 && (u[1] - v[1]).abs() < 1e-4, "edge {i} turned: {u:?} → {v:?}");
        }
        // The edge moved by d along its outward normal.
        let nrm = trap.normal(edge);
        let (a0, _) = trap.edge(edge);
        let (a1, _) = moved.edge(edge);
        let off = (a1[0] - a0[0]) * nrm[0] + (a1[1] - a0[1]) * nrm[1];
        assert!((off - d).abs() < 1e-3, "{off} vs {d}");
    }

    // A vertex dragged past the diagonal makes the panel concave.
    let sq = rect(0.0, 0.0, 100.0, 100.0);
    assert!(sq.with_vertex(0, [80.0, 80.0]).is_none());
    assert!(sq.with_vertex(0, [-20.0, -10.0]).is_some());
    assert!(sq.with_vertex(9, [0.0, 0.0]).is_none());

    // Insert and delete keep validity.
    let five = sq.with_inserted_vertex(0, [50.0, 0.0]).unwrap();
    assert_eq!(five.points().len(), 5);
    assert!((five.area() - sq.area()).abs() < 1e-3);
    let bumped = five.with_vertex(1, [50.0, -20.0]).unwrap();
    assert!(bumped.area() > sq.area());
    assert_eq!(five.without_vertex(1).unwrap(), sq);
    let tri = Panel::new(vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]).unwrap();
    assert!(tri.without_vertex(0).is_none(), "three vertices at least");

    // Construction rejects the degenerate and the non-convex.
    assert!(Panel::new(vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]]).is_none());
    assert!(Panel::new(vec![[0.0, 0.0], [10.0, 0.0], [2.0, 2.0], [0.0, 10.0]]).is_none());
    assert!(Panel::new(vec![[0.0, 0.0], [f32::NAN, 0.0], [0.0, 10.0]]).is_none());
    let star: Vec<[f32; 2]> = (0..5)
        .map(|i| {
            let a = (i * 2 % 5) as f32 * std::f32::consts::TAU / 5.0;
            [a.cos() * 50.0, a.sin() * 50.0]
        })
        .collect();
    assert!(Panel::new(star).is_none(), "a pentagram turns twice");
    let cw = Panel::new(vec![[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]]).unwrap();
    assert!(cw.area() > 0.0, "clockwise input is reversed");
    assert_eq!(cw.translated([5.0, 1.0]).bounds(), RectF { x: 5.0, y: 1.0, w: 10.0, h: 10.0 });
}

// ----- compositing --------------------------------------------------------

fn fill(doc: &mut Document, id: LayerId, color: [u16; 4]) {
    let (tw, th) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
    let (grid, _) = doc.paint_target(id).unwrap();
    for ty in 0..th {
        for tx in 0..tw {
            fill_tile(grid.get_mut_or_create(TileCoord::new(tx, ty)), color);
        }
    }
}

fn edit(doc: &mut Document, id: LayerId, f: impl FnOnce(&mut arty_core::LayerProps)) {
    let mut p = doc.layer(id).unwrap().props.clone();
    f(&mut p);
    doc.set_props(id, p);
}

fn render(doc: &Document, c: TileCoord) -> Box<TilePixels> {
    let mut out = new_tile_box();
    doc.composite_tile(c, &mut out, &mut CompositeScratch::new());
    out
}

fn px(doc: &Document, x: i32, y: i32) -> [u16; 4] {
    let c = TileCoord::from_pixel(x, y);
    render(doc, c)[(y - c.y * 64) as usize][(x - c.x * 64) as usize]
}

fn close(a: [u16; 4], b: [u16; 4], tol: i32) -> bool {
    (0..4).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
}

/// `[red, folder{child}]` on a 256×192 page; returns (folder, child).
fn red_under_folder(child: [u16; 4], blend: BlendMode, child_blend: BlendMode) -> (Document, LayerId, LayerId) {
    let mut doc = Document::new(256, 192, 72);
    let bottom = doc.active();
    fill(&mut doc, bottom, RED);
    let folder = doc.add_folder().unwrap();
    edit(&mut doc, folder, |p| p.blend = blend);
    let inner = doc.add_raster_layer().unwrap();
    doc.move_layer(inner, Some(folder), 0);
    edit(&mut doc, inner, |p| p.blend = child_blend);
    fill(&mut doc, inner, child);
    (doc, folder, inner)
}

fn set(doc: &mut Document, folder: LayerId, s: FrameShape) -> Arc<Frame> {
    let f = Frame::build(s, doc.width(), doc.height());
    doc.set_frame(folder, Some(f.clone()));
    f
}

#[test]
fn fr05_composite() {
    for blend in [BlendMode::Normal, BlendMode::PassThrough] {
        let (mut doc, folder, _) = red_under_folder(BLUE, blend, BlendMode::Normal);
        let plain: Vec<_> = (0..12).map(|i| render(&doc, TileCoord::new(i % 4, i / 4))).collect();
        // Tile (1, 0) lies inside the panel; (3, 2) outside it.
        set(&mut doc, folder, shape(vec![rect(60.0, 0.0, 130.0, 120.5)], 0.0, BLACK));
        assert!(
            render(&doc, TileCoord::new(3, 2)).as_flattened().iter().all(|&p| p == RED),
            "{blend:?}: an Outside tile hides the folder"
        );
        assert!(*render(&doc, TileCoord::new(1, 0)) == *plain[1], "{blend:?}: a Full tile equals the plain folder");
        assert_eq!(px(&doc, 59, 10), RED, "{blend:?}: outside the polygon");
        assert_eq!(px(&doc, 60, 10), BLUE);
        assert_eq!(px(&doc, 120, 121), RED);
        assert!(
            close(px(&doc, 120, 120), [O / 2, 0, O / 2, O], 80),
            "{blend:?}: half covered {:?}",
            px(&doc, 120, 120)
        );

        // The border is drawn over the children; folder opacity scales it.
        set(&mut doc, folder, shape(vec![rect(60.0, 0.0, 130.0, 120.0)], 4.0, GREEN));
        assert_eq!(px(&doc, 61, 50), GREEN);
        assert_eq!(px(&doc, 64, 50), BLUE);
        assert_eq!(px(&doc, 120, 117), GREEN);
        assert_eq!(px(&doc, 120, 121), RED);
        edit(&mut doc, folder, |p| p.opacity = 0.5);
        assert!(close(px(&doc, 61, 50), [O / 2, O / 2, 0, O], 2), "{blend:?}: {:?}", px(&doc, 61, 50));
        assert!(close(px(&doc, 100, 50), [O / 2, 0, O / 2, O], 2));
        assert_eq!(px(&doc, 59, 50), RED);
        // An empty frame folder still draws its border.
        edit(&mut doc, folder, |p| p.opacity = 1.0);
        let child = doc.layer(folder).unwrap().children().unwrap()[0];
        doc.paint_target(child).unwrap().0.clear();
        assert_eq!(px(&doc, 61, 50), GREEN);
        assert_eq!(px(&doc, 100, 50), RED);
    }
}

#[test]
fn fr06_pass_through_frame_folder() {
    // A multiply child: pass-through blends it against the layer below.
    let (mut doc, folder, _) = red_under_folder(GRAY, BlendMode::PassThrough, BlendMode::Multiply);
    let plain: Vec<_> = (0..12).map(|i| render(&doc, TileCoord::new(i % 4, i / 4))).collect();
    assert_eq!(plain[0][5][5], [O / 2, 0, 0, O], "red multiplied by gray");
    set(&mut doc, folder, shape(vec![rect(60.0, 0.0, 130.0, 120.5)], 0.0, BLACK));
    for i in 0..12usize {
        let c = TileCoord::new(i as i32 % 4, i as i32 / 4);
        let got = render(&doc, c);
        for y in 0..64 {
            for x in 0..64 {
                let (gx, gy) = (c.x * 64 + x as i32, c.y * 64 + y as i32);
                let inside = (60..190).contains(&gx) && gy < 120;
                let outside = !(60..190).contains(&gx) || gy > 120;
                if inside {
                    assert_eq!(got[y][x], plain[i][y][x], "inside ({gx}, {gy})");
                } else if outside {
                    assert_eq!(got[y][x], RED, "outside, dst untouched ({gx}, {gy})");
                }
            }
        }
    }

    // An isolated frame folder equals mask(plain isolated).
    let (mut doc, folder, _) = red_under_folder([0, 0, O / 2, O / 2], BlendMode::Normal, BlendMode::Normal);
    let plain = px(&doc, 120, 120);
    set(&mut doc, folder, shape(vec![rect(60.0, 0.0, 130.0, 120.5)], 0.0, BLACK));
    let half = px(&doc, 120, 120);
    for ch in 0..4 {
        let want = (RED[ch] as i32 + plain[ch] as i32) / 2;
        assert!((half[ch] as i32 - want).abs() <= 80, "{half:?} vs {plain:?}");
    }
    assert_eq!(px(&doc, 120, 100), plain);
}

#[test]
fn fr07_clipping() {
    // A frame folder as a clip base limits its clips to panel + border.
    for blend in [BlendMode::Normal, BlendMode::PassThrough] {
        let (mut doc, folder, _) = red_under_folder(BLUE, blend, BlendMode::Normal);
        set(&mut doc, folder, shape(vec![rect(60.0, 0.0, 130.0, 120.0)], 4.0, BLACK));
        doc.set_active(folder);
        let clip = doc.add_raster_layer().unwrap();
        edit(&mut doc, clip, |p| p.clip = true);
        fill(&mut doc, clip, GREEN);
        assert_eq!(px(&doc, 100, 50), GREEN, "{blend:?}");
        assert_eq!(px(&doc, 61, 50), GREEN, "{blend:?}: over the border too");
        assert_eq!(px(&doc, 59, 50), RED, "{blend:?}");
        assert_eq!(px(&doc, 100, 150), RED, "{blend:?}");
    }

    // A pass-through frame folder that is itself a clip renders isolated.
    let build = |blend: BlendMode| {
        let mut doc = Document::new(256, 192, 72);
        let base = doc.active();
        fill(&mut doc, base, RED);
        let folder = doc.add_folder().unwrap();
        edit(&mut doc, folder, |p| {
            p.blend = blend;
            p.clip = true;
        });
        let inner = doc.add_raster_layer().unwrap();
        doc.move_layer(inner, Some(folder), 0);
        edit(&mut doc, inner, |p| p.blend = BlendMode::Multiply);
        fill(&mut doc, inner, GRAY);
        set(&mut doc, folder, shape(vec![rect(60.0, 0.0, 130.0, 120.5)], 2.0, BLACK));
        doc
    };
    let (pt, normal) = (build(BlendMode::PassThrough), build(BlendMode::Normal));
    for i in 0..12 {
        let c = TileCoord::new(i % 4, i / 4);
        assert!(*render(&pt, c) == *render(&normal, c), "tile {c:?}");
    }
    assert_eq!(px(&pt, 100, 50), GRAY, "isolated: the multiply child lands as plain gray");
    assert_eq!(px(&pt, 10, 50), RED);
}

// ----- dirty marking --------------------------------------------------------

/// Composite every page tile into `cache`, or only the dirty ones.
fn refresh(doc: &mut Document, cache: &mut Vec<Box<TilePixels>>) {
    let mut dirty = Vec::new();
    let all = doc.dirty_mut().drain_into(&mut dirty) || cache.is_empty();
    let (tw, th) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
    cache.resize_with((tw * th) as usize, new_tile_box);
    for ty in 0..th {
        for tx in 0..tw {
            let c = TileCoord::new(tx, ty);
            if all || dirty.contains(&c) {
                cache[(ty * tw + tx) as usize] = render(doc, c);
            }
        }
    }
}

fn assert_cache_fresh(doc: &mut Document, cache: &mut Vec<Box<TilePixels>>, what: &str) {
    refresh(doc, cache);
    let tw = doc.tiles_wide() as i32;
    for (i, tile) in cache.iter().enumerate() {
        let c = TileCoord::new(i as i32 % tw, i as i32 / tw);
        assert!(**tile == *render(doc, c), "{what}: stale tile {c:?}");
    }
}

#[test]
fn fr09_dirty_marking_keeps_caches_fresh() {
    // Children only in the top-left tile; the border reaches tiles without
    // any child pixels.
    let mut doc = Document::new(320, 256, 72);
    let bottom = doc.active();
    fill(&mut doc, bottom, RED);
    let folder = doc.add_folder().unwrap();
    let inner = doc.add_raster_layer().unwrap();
    doc.move_layer(inner, Some(folder), 0);
    fill_tile(doc.paint_target(inner).unwrap().0.get_mut_or_create(TileCoord::new(0, 0)), BLUE);
    let mut cache = Vec::new();
    let mut h = History::default();
    assert_cache_fresh(&mut doc, &mut cache, "start");

    let a = Frame::build(shape(vec![rect(10.0, 10.0, 200.0, 180.0)], 6.0, GREEN), 320, 256);
    let old = doc.set_frame(folder, Some(a.clone())).unwrap();
    h.push(Edit::Frame { layer: folder, frame: old });
    assert_cache_fresh(&mut doc, &mut cache, "add");
    let b = Frame::build(shape(vec![rect(40.5, 30.0, 260.0, 200.0)], 3.0, BLACK), 320, 256);
    let old = doc.set_frame(folder, Some(b)).unwrap();
    h.push(Edit::Frame { layer: folder, frame: old });
    assert_cache_fresh(&mut doc, &mut cache, "edit");
    for step in 0..2 {
        h.undo(&mut doc);
        assert_cache_fresh(&mut doc, &mut cache, &format!("undo {step}"));
    }
    for step in 0..2 {
        h.redo(&mut doc);
        assert_cache_fresh(&mut doc, &mut cache, &format!("redo {step}"));
    }
    let old = doc.set_frame(folder, None).unwrap();
    h.push(Edit::Frame { layer: folder, frame: old });
    assert_cache_fresh(&mut doc, &mut cache, "remove");
    h.undo(&mut doc);
    assert_cache_fresh(&mut doc, &mut cache, "undo remove");

    // Visibility and opacity of the frame folder refresh the border tiles.
    for (what, f) in [
        ("hide", Box::new(|p: &mut arty_core::LayerProps| p.visible = false) as Box<dyn Fn(&mut _)>),
        ("show", Box::new(|p: &mut arty_core::LayerProps| p.visible = true)),
        ("opacity", Box::new(|p: &mut arty_core::LayerProps| p.opacity = 0.3)),
        ("normal", Box::new(|p: &mut arty_core::LayerProps| p.blend = BlendMode::Normal)),
        ("opaque", Box::new(|p: &mut arty_core::LayerProps| p.opacity = 1.0)),
    ] {
        edit(&mut doc, folder, f);
        assert_cache_fresh(&mut doc, &mut cache, what);
    }

    // A cut and its undo.
    let shape_now = doc.frame(folder).unwrap().shape().clone();
    let (cut, _) = shape_now.cut([0.0, 100.0], [320.0, 120.0], 12.0, 8.0).unwrap();
    let old = doc.set_frame(folder, Some(Frame::build(cut, 320, 256))).unwrap();
    h.push(Edit::Frame { layer: folder, frame: old });
    assert_cache_fresh(&mut doc, &mut cache, "cut");
    h.undo(&mut doc);
    assert_cache_fresh(&mut doc, &mut cache, "undo cut");
}

// ----- history --------------------------------------------------------------

#[test]
fn fr10_history() {
    let mut doc = Document::new(256, 256, 350);
    let folder = add_frame_folder(&mut doc, shape(vec![rect(20.0, 20.0, 200.0, 100.0)], 4.0, BLACK)).unwrap();
    assert!(doc.frame(folder).is_some());
    let child = doc.active();
    assert_eq!(doc.layer(folder).unwrap().children().unwrap(), [child], "an empty raster child, now active");
    assert!(doc.layer(child).unwrap().raster().unwrap().is_empty());
    assert_eq!(doc.frame_folder_of(child), Some(folder));
    assert!(add_frame_folder(&mut doc, shape(Vec::new(), 4.0, BLACK)).is_none(), "no panels");

    let mut h = History::default();
    let a = doc.frame(folder).unwrap().clone();
    let b = Frame::build(shape(vec![rect(0.0, 0.0, 128.0, 128.0)], 2.0, GREEN), 256, 256);
    let rev = doc.revision();
    let old = doc.set_frame(folder, Some(b.clone())).unwrap();
    assert!(doc.revision() > rev, "set_frame bumps the revision");
    h.push(Edit::Frame { layer: folder, frame: old });
    h.undo(&mut doc);
    assert!(Arc::ptr_eq(doc.frame(folder).unwrap(), &a));
    h.redo(&mut doc);
    assert!(Arc::ptr_eq(doc.frame(folder).unwrap(), &b));

    let page = PageSetup {
        trim: RectF { x: 10.0, y: 10.0, w: 200.0, h: 220.0 },
        bleed: 8.0,
        safe: 6.0,
        inner: RectF::default(),
        unit: 0,
    };
    let rev = doc.revision();
    let old = doc.set_page_setup(Some(page)).unwrap();
    assert!(doc.revision() > rev, "set_page_setup bumps the revision");
    assert!(doc.set_page_setup(Some(page)).is_none(), "no change");
    h.push(Edit::Page(old));
    h.undo(&mut doc);
    assert_eq!(doc.page_setup(), None);
    h.redo(&mut doc);
    assert_eq!(doc.page_setup(), Some(&page));

    let copy = doc.duplicate_layer(folder).unwrap();
    assert!(Arc::ptr_eq(doc.frame(copy).unwrap(), &b), "duplicate copies the frame");
    assert!(doc.delete_layer(copy));
    assert!(doc.frame(copy).is_none() && doc.layer(copy).is_none(), "delete removes it");
}

// ----- presets --------------------------------------------------------------

#[test]
fn fr12_presets_and_sanitizing() {
    let b4 = MANGA_PRESETS.iter().find(|p| p.name.contains("B4")).unwrap();
    let (w, h, page) = PageSetup::from_mm(b4, 600);
    assert_eq!((w, h), (6071, 8598));
    assert_eq!((page.trim.w, page.trim.h), (5197.0, 7323.0));
    assert_eq!(page.bleed, 118.0);
    assert_eq!((page.trim.x, page.trim.y), (((6071 - 5197) / 2) as f32, ((8598 - 7323) / 2) as f32));
    assert_eq!(page.sanitized(w, h), Some(page), "a preset is valid as is");
    assert!(page.inner.w > 0.0 && page.inner.x >= page.trim.x);
    for p in MANGA_PRESETS {
        for dpi in [350, 600] {
            let (w, h, s) = PageSetup::from_mm(p, dpi);
            assert_eq!(s.sanitized(w, h), Some(s), "{} at {dpi}", p.name);
            if p.paper_mm.is_none() {
                assert_eq!(w as f32, s.trim.w + 2.0 * s.bleed, "{}", p.name);
            }
        }
    }
    let us = MANGA_PRESETS.iter().find(|p| p.name.contains("US")).unwrap();
    assert_eq!(PageSetup::from_mm(us, 600).2.unit, arty_core::page::UNIT_IN);
    assert_eq!(page.unit, arty_core::page::UNIT_MM);

    let mut bad = page;
    bad.trim.w = f32::NAN;
    assert_eq!(bad.sanitized(w, h), None, "NaN");
    let mut bad = page;
    bad.bleed = f32::INFINITY;
    assert_eq!(bad.sanitized(w, h), None);
    let mut bad = page;
    bad.trim.x = 1000.0;
    assert_eq!(bad.sanitized(w, h), None, "trim past the canvas");
    let mut bad = page;
    bad.trim.x = -1.0;
    assert_eq!(bad.sanitized(w, h), None);
    let mut bad = page;
    bad.trim.w = 0.0;
    assert_eq!(bad.sanitized(w, h), None);
    // Small fixes.
    let mut odd = page;
    odd.safe = 1e9;
    odd.bleed = -4.0;
    odd.unit = 9;
    odd.inner = RectF { x: -100.0, y: 0.0, w: 300.0, h: 0.0 };
    let fixed = odd.sanitized(w, h).unwrap();
    assert_eq!(fixed.safe, page.trim.w / 2.0);
    assert_eq!(fixed.bleed, 0.0);
    assert_eq!(fixed.unit, 0);
    assert_eq!(fixed.inner, RectF::default(), "an inner frame without height is none");
}

/// Not a numbered test: the per-tile helpers stay exact at the extremes.
#[test]
fn fr_mask_helpers() {
    use arty_core::frame::{mask_tile, mask_toward, over_color};
    let mut m: Box<MaskTile> = Box::new([[0; TILE_SIZE]; TILE_SIZE]);
    m[0][1] = 255;
    m[0][2] = 128;
    let mut t = new_tile_box();
    fill_tile(&mut t, BLUE);
    mask_tile(&mut t, &m);
    assert_eq!((t[0][0], t[0][1]), ([0; 4], BLUE));
    assert!(close(t[0][2], [0, 0, O / 2, O / 2], 80));
    let mut base = new_tile_box();
    fill_tile(&mut base, RED);
    let mut t = new_tile_box();
    fill_tile(&mut t, BLUE);
    mask_toward(&mut t, &base, &m);
    assert_eq!((t[0][0], t[0][1]), (RED, BLUE));
    over_color(&mut t, GREEN, Cov::Partial(&m));
    assert_eq!((t[0][0], t[0][1]), (RED, GREEN));
    over_color(&mut t, [0, O / 2, 0, O / 2], Cov::Full);
    assert!(close(t[0][0], [O / 2, O / 2, 0, O], 1), "{:?}", t[0][0]);
}
