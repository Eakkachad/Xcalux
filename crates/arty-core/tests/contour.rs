//! SEL-UI: marching-ants outlines (`arty_core::contour`).

use std::sync::Arc;

use arty_core::contour::{Contours, LOD_TOL, Polyline, extract, extract_with_budget};
use arty_core::selection::full_mask;
use arty_core::{MaskPixels, Pt, Selection, TILE_SIZE, TileCoord};

const T: i32 = TILE_SIZE as i32;

/// Every page tile fully selected (what `Selection::all` stores).
fn full_page(w: u32, h: u32) -> Selection {
    let mut s = Selection::new();
    for ty in 0..h.div_ceil(64) as i32 {
        for tx in 0..w.div_ceil(64) as i32 {
            s.insert_tile(TileCoord::new(tx, ty), full_mask().clone());
        }
    }
    s
}

/// A selection with coverage `f(x, y)` on a `w`×`h` page.
fn from_fn(w: u32, h: u32, f: impl Fn(i32, i32) -> u8) -> Selection {
    let mut s = Selection::new();
    for ty in 0..h.div_ceil(64) as i32 {
        for tx in 0..w.div_ceil(64) as i32 {
            let mut m: MaskPixels = [[0; TILE_SIZE]; TILE_SIZE];
            for (y, row) in m.iter_mut().enumerate() {
                for (x, px) in row.iter_mut().enumerate() {
                    let (gx, gy) = (tx * T + x as i32, ty * T + y as i32);
                    if gx < w as i32 && gy < h as i32 {
                        *px = f(gx, gy);
                    }
                }
            }
            s.insert_tile(TileCoord::new(tx, ty), Arc::new(m));
        }
    }
    s
}

/// Area coverage of pixel `(x, y)` by the disk, 16×16 supersampled.
fn disk_cov(cx: f32, cy: f32, r: f32) -> impl Fn(i32, i32) -> u8 {
    move |x, y| {
        let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let d = (px * px + py * py).sqrt();
        if d < r - 0.75 {
            return 255;
        }
        if d > r + 0.75 {
            return 0;
        }
        let mut n = 0;
        for j in 0..16 {
            for i in 0..16 {
                let (sx, sy) = (x as f32 + (i as f32 + 0.5) / 16.0 - cx, y as f32 + (j as f32 + 0.5) / 16.0 - cy);
                n += (sx * sx + sy * sy <= r * r) as u32;
            }
        }
        (n * 255 / 256 + (n * 255 % 256 >= 128) as u32) as u8
    }
}

fn lod0(c: &Contours) -> &[Polyline] {
    &c.lods[0]
}

fn assert_closed(c: &Contours) {
    for (k, lod) in c.lods.iter().enumerate() {
        for p in lod {
            assert!(p.closed, "LOD{k}: an open outline (dangling ends)");
            assert!(p.pts.len() >= 2, "LOD{k}: a degenerate outline");
        }
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f32 {
    let (abx, aby, apx, apy) = (b[0] - a[0], b[1] - a[1], p[0] - a[0], p[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 > 0.0 { ((apx * abx + apy * aby) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((apx - t * abx).powi(2) + (apy - t * aby).powi(2)).sqrt()
}

fn segments(p: &Polyline) -> impl Iterator<Item = (Pt, Pt)> + '_ {
    let n = p.pts.len();
    let m = if p.closed { n } else { n - 1 };
    (0..m).map(move |i| (p.pts[i], p.pts[(i + 1) % n]))
}

fn extent(p: &Polyline) -> f32 {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in &p.pts {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    (hi[0] - lo[0]).max(hi[1] - lo[1])
}

#[test]
fn su01_select_all_is_four_segments() {
    for (w, h) in [(128, 128), (200, 130), (64, 64), (1, 1), (300, 77)] {
        let c = extract(&full_page(w, h), w, h);
        assert_eq!(c.segments, 4, "{w}×{h}");
        assert_closed(&c);
        for (k, lod) in c.lods.iter().enumerate() {
            // A page smaller than a level's tolerance is too small to show there.
            let visible = w.max(h) as f32 >= LOD_TOL[k];
            assert_eq!(lod.len(), visible as usize, "{w}×{h} LOD{k}");
        }
        let mut pts = lod0(&c)[0].pts.clone();
        pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (w, h) = (w as f32, h as f32);
        assert_eq!(pts, vec![[0.0, 0.0], [0.0, h], [w, 0.0], [w, h]]);
    }
    assert_eq!(extract(&Selection::new(), 100, 100).segments, 0);
}

#[test]
fn su02_aa_disk_radius() {
    for (cx, cy, r) in [(100.37, 90.71, 40.3), (128.0, 128.0, 70.0), (60.2, 61.9, 12.5)] {
        let c = extract(&from_fn(256, 256, disk_cov(cx, cy, r)), 256, 256);
        assert_closed(&c);
        assert_eq!(lod0(&c).len(), 1);
        let mut worst = 0.0f32;
        for p in &lod0(&c)[0].pts {
            let d = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt();
            worst = worst.max((d - r).abs());
        }
        assert!(worst <= 0.1, "disk r = {r}: a vertex is {worst} px off the circle");
        assert!(lod0(&c)[0].pts.len() >= 16, "the disk is not over-simplified");
    }
}

#[test]
fn su03_tile_seams_close_and_do_not_depend_on_alignment() {
    // A disk and a ring (two outlines) at offsets that move them across seams.
    let counts: Vec<(usize, usize)> = [0, 13, 32, 63, 64]
        .iter()
        .map(|&k| {
            let (cx, cy) = (100.3 + k as f32, 120.6 + k as f32);
            let disk = disk_cov(cx, cy, 70.0);
            let hole = disk_cov(cx, cy, 30.0);
            let c = extract(&from_fn(384, 384, |x, y| disk(x, y).saturating_sub(hole(x, y))), 384, 384);
            assert_closed(&c);
            assert_eq!(lod0(&c).len(), 2, "offset {k}: outer outline and hole");
            let mut n: Vec<usize> = lod0(&c).iter().map(|p| p.pts.len()).collect();
            n.sort();
            (n[0], n[1])
        })
        .collect();
    let (a, b) = counts[0];
    for &(x, y) in &counts {
        assert!(x.abs_diff(a) <= 2 && y.abs_diff(b) <= 2, "vertex counts {counts:?}");
    }
    // Hard-edged shapes too: a 5×5 block of single pixels on a seam corner.
    let c = extract(&from_fn(256, 256, |x, y| if (62..67).contains(&x) && (61..66).contains(&y) { 255 } else { 0 }), 256, 256);
    assert_closed(&c);
    assert_eq!(c.segments, 4);
}

#[test]
fn su04_l_shape_of_full_tiles_is_six_segments() {
    let mut s = Selection::new();
    for (x, y) in [(0, 0), (1, 0), (0, 1)] {
        s.insert_tile(TileCoord::new(x, y), full_mask().clone());
    }
    // And the same L away from the page border.
    let mut away = Selection::new();
    for (x, y) in [(2, 2), (3, 2), (2, 3)] {
        away.insert_tile(TileCoord::new(x, y), full_mask().clone());
    }
    for (sel, o) in [(s, 0.0), (away, 128.0)] {
        let c = extract(&sel, 512, 512);
        assert_closed(&c);
        assert_eq!(c.segments, 6);
        let mut pts = lod0(&c)[0].pts.clone();
        pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut want: Vec<Pt> = [[0.0, 0.0], [128.0, 0.0], [128.0, 64.0], [64.0, 64.0], [64.0, 128.0], [0.0, 128.0]]
            .map(|p| [p[0] + o, p[1] + o])
            .into();
        want.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(pts, want);
    }
}

#[test]
fn su05_lods_stay_within_tolerance() {
    // A wiggly AA blob plus a few 3×3 specks.
    let blob = |x: i32, y: i32| {
        let (dx, dy) = (x as f32 + 0.5 - 150.0, y as f32 + 0.5 - 140.0);
        let r = 90.0 * (1.0 + 0.12 * (5.0 * dy.atan2(dx)).sin());
        let d = (dx * dx + dy * dy).sqrt();
        ((r - d + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let speck = |x: i32, y: i32| {
        [(20, 20), (280, 30), (30, 270)].iter().any(|&(sx, sy)| (sx..sx + 3).contains(&x) && (sy..sy + 3).contains(&y))
    };
    let c = extract(&from_fn(300, 300, |x, y| if speck(x, y) { 255 } else { blob(x, y) }), 300, 300);
    assert_closed(&c);
    assert_eq!(lod0(&c).len(), 4);
    for k in 1..3 {
        let tol = LOD_TOL[k] + 1e-3;
        for p in lod0(&c).iter().filter(|p| extent(p) >= LOD_TOL[k]) {
            for &q in &p.pts {
                let d = c.lods[k].iter().flat_map(segments).map(|(a, b)| seg_dist(q, a, b)).fold(f32::MAX, f32::min);
                assert!(d <= tol, "LOD{k}: a LOD0 vertex is {d} px from the outline");
            }
        }
        let verts = |k: usize| c.lods[k].iter().map(|p| p.pts.len()).sum::<usize>();
        assert!(verts(k) < verts(k - 1), "LOD{k} is coarser");
    }
    assert_eq!(c.lods[1].len(), 4, "3 px specks stay at LOD1 (2 px)");
    assert_eq!(c.lods[2].len(), 1, "3 px specks are dropped at LOD2 (8 px)");
}

#[test]
fn su06_budget_overflow_pools_or_truncates() {
    // Noise: a crossing in nearly every cell.
    let mut seed = 0x9e37_79b9_u32;
    let mut noise = vec![0u8; 256 * 256];
    for v in &mut noise {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        *v = if seed & 1 == 1 { 255 } else { 0 };
    }
    let sel = from_fn(256, 256, |x, y| noise[(y * 256 + x) as usize]);
    let full = extract(&sel, 256, 256);
    assert_eq!((full.pool, full.truncated), (1, false));
    let c = extract_with_budget(&sel, 256, 256, 5_000);
    assert!(c.pool > 1 || c.truncated, "over budget: pool {} truncated {}", c.pool, c.truncated);
    let c = extract_with_budget(&sel, 256, 256, 0);
    assert!(c.truncated);

    // A pooled disk keeps its shape (to pooling precision).
    let disk = from_fn(256, 256, disk_cov(128.0, 128.0, 60.0));
    let c = extract_with_budget(&disk, 256, 256, 350);
    assert_eq!((c.pool, c.truncated), (2, false));
    assert_closed(&c);
    for p in &lod0(&c)[0].pts {
        let d = ((p[0] - 128.0).powi(2) + (p[1] - 128.0).powi(2)).sqrt();
        assert!((d - 60.0).abs() <= 1.0, "pooled outline at radius {d}");
    }
}
