//! SEL-CORE (plans/m3_page_tools.md §9.1, test 11): the `SELM` section.

mod common;

use std::sync::Arc;

use arty_core::selection::full_mask;
use arty_core::{Document, MaskPixels, MaskView, Selection, TILE_SIZE, TileCoord};
use arty_io::format::RecordKind;
use arty_io::limits::MAX_SELM_BYTES;
use arty_io::manifest::{SEC_CRITICAL, SEC_SAFE_TO_COPY, TAG_SELM, decode_payload};
use arty_io::selm::{self, SelectionSave, SelmCache};
use arty_io::{AppSection, FileWriter, LoadWarning, Progress, SaveExtras};
use common::raw::Raw;
use common::*;

const T: usize = TILE_SIZE;

/// Every tile kind: full, soft (U8), hard-edged (BIT), plus edge tiles
/// whose off-page pixels hold data.
fn mixed(w: u32, h: u32, seed: u64) -> Selection {
    let mut rng = Rng(seed);
    let (tw, th) = (w.div_ceil(64) as i32, h.div_ceil(64) as i32);
    let mut s = Selection::new();
    for ty in 0..th {
        for tx in 0..tw {
            let c = TileCoord::new(tx, ty);
            let mut m: MaskPixels = [[0; T]; T];
            match rng.below(5) {
                0 => continue,
                1 => {
                    s.insert_tile(c, full_mask().clone());
                    continue;
                }
                2 => m.as_flattened_mut().iter_mut().for_each(|v| *v = rng.next() as u8),
                3 => {
                    for (y, row) in m.iter_mut().enumerate() {
                        for (x, v) in row.iter_mut().enumerate() {
                            *v = if (x ^ y) & 4 == 0 { 255 } else { 0 };
                        }
                    }
                }
                _ => {
                    for (y, row) in m.iter_mut().enumerate() {
                        for (x, v) in row.iter_mut().enumerate() {
                            *v = (x * 3 + y) as u8;
                        }
                    }
                }
            }
            s.insert_tile(c, Arc::new(m));
        }
    }
    s
}

fn kinds(s: &Selection) -> (usize, usize, usize) {
    let (mut full, mut soft, mut hard) = (0, 0, 0);
    for (c, _) in s.tiles() {
        match s.get(c) {
            MaskView::Full => full += 1,
            MaskView::Partial(m) if m.as_flattened().iter().all(|&v| v == 0 || v == 255) => hard += 1,
            MaskView::Partial(_) => soft += 1,
            MaskView::Empty => {}
        }
    }
    (full, soft, hard)
}

fn encode(s: &Selection, w: u32, h: u32) -> (Option<Arc<[u8]>>, SelectionSave) {
    selm::encode(s, 1, w, h, &mut SelmCache::default())
}

/// The sections of the newest manifest in a file: (tag, flags, body).
fn sections(file: &[u8]) -> Vec<([u8; 4], u32, Vec<u8>)> {
    let (at, _, end) = records(file).into_iter().rev().find(|r| r.1 == RecordKind::Manifest as u8).unwrap();
    let raw = decode_payload(&file[at as usize + 24..end as usize], at).unwrap();
    let mut out = Vec::new();
    let mut p = 0;
    while p < raw.len() {
        let tag: [u8; 4] = raw[p..p + 4].try_into().unwrap();
        let flags = u32::from_le_bytes(raw[p + 4..p + 8].try_into().unwrap());
        let len = u32::from_le_bytes(raw[p + 8..p + 12].try_into().unwrap()) as usize;
        out.push((tag, flags, raw[p + 12..p + 12 + len].to_vec()));
        p += 12 + len;
    }
    out
}

fn manifest_raw(sections: &[([u8; 4], u32, Vec<u8>)]) -> Vec<u8> {
    let mut w = arty_io::manifest::SectionWriter::default();
    for (tag, flags, body) in sections {
        w.push(*tag, *flags, body);
    }
    w.into_raw()
}

#[test]
fn sc11_full_u8_and_bit_tiles_round_trip_bit_exact() {
    let pool = pool();
    for (w, h, seed) in [(300, 200, 1), (64, 64, 2), (1000, 77, 3)] {
        let sel = mixed(w, h, seed);
        let (body, save) = encode(&sel, w, h);
        assert_eq!(save, SelectionSave::Exact);
        let mut warn = Vec::new();
        let back = selm::decode(&body.unwrap(), w, h, &mut warn).unwrap();
        assert!(warn.is_empty());
        assert_same_selection(&back, &sel);
        assert!(back.tiles().all(|(c, m)| !matches!(sel.get(c), MaskView::Full) || Arc::ptr_eq(m, full_mask())));

        // Through a whole file.
        let mut doc = Document::new(w, h, 350);
        doc.swap_selection(sel.clone());
        let l = read(&write(&doc, &SaveExtras::default(), &pool), &pool);
        assert_same_selection(l.doc.selection(), &sel);
        assert_eq!(l.read_only_reason, None);
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert_ne!(l.doc.selection_rev(), 0, "loaded through the unrecorded setter");
    }
    let (full, soft, hard) = kinds(&mixed(300, 200, 1));
    assert!(full > 0 && soft > 0 && hard > 0, "all kinds covered: {full} {soft} {hard}");

    // No selection, no section.
    assert_eq!(encode(&Selection::new(), 300, 200), (None, SelectionSave::None));
    let file = write(&Document::new(300, 200, 350), &SaveExtras::default(), &pool);
    assert!(sections(&file).iter().all(|s| s.0 != TAG_SELM));
}

#[test]
fn sc11_unusable_selections_are_dropped_with_a_warning() {
    let (w, h) = (300, 200);
    let sel = mixed(w, h, 4);
    let body = encode(&sel, w, h).0.unwrap().to_vec();
    let drop = |b: &[u8], w: u32, h: u32| {
        let mut warn = Vec::new();
        let got = selm::decode(b, w, h, &mut warn);
        assert!(got.is_none());
        match warn.as_slice() {
            [LoadWarning::SelectionDropped { reason }] => *reason,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(drop(&body, 301, 200), "page size mismatch");
    assert!(matches!(drop(&body[..body.len() - 1], w, h), "lz4 data" | "bad length"));
    assert_eq!(drop(&body[..10], w, h), "truncated");

    // A hand-built stored body with a tile past the page.
    let stored = |tiles: &[(i32, i32, u8)], extra: &[u8]| {
        let mut payload = Vec::new();
        for &(x, y, k) in tiles {
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
            payload.push(k);
        }
        payload.extend_from_slice(extra);
        let mut b = vec![1, 0, 0, 0];
        for v in [w, h, tiles.len() as u32, payload.len() as u32] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&payload);
        b
    };
    let ok = stored(&[(0, 0, 0), (4, 3, 2)], &[0xAA; 512]);
    let mut warn = Vec::new();
    let s = selm::decode(&ok, w, h, &mut warn).unwrap();
    assert_eq!((s.value(0, 0), s.value(4 * 64 + 1, 3 * 64), s.value(4 * 64, 3 * 64)), (255, 255, 0));
    assert_eq!(drop(&stored(&[(5, 0, 0)], &[]), w, h), "off-page tile");
    assert_eq!(drop(&stored(&[(0, -1, 0)], &[]), w, h), "off-page tile");
    assert_eq!(drop(&stored(&[(0, 0, 3)], &[]), w, h), "bad tile kind");
    assert_eq!(drop(&stored(&[(1, 0, 0), (0, 0, 0)], &[]), w, h), "tile order");
    assert_eq!(drop(&stored(&[(0, 0, 1)], &[0; 100]), w, h), "bad length");
    let mut v2 = ok.clone();
    v2[0] = 2;
    assert_eq!(drop(&v2, w, h), "unknown version");

    // In a file: the load succeeds, without a selection, and is not lossy.
    let pool = pool();
    let mut doc = Document::new(w, h, 350);
    doc.swap_selection(sel);
    let file = write(&doc, &SaveExtras::default(), &pool);
    let mut secs = sections(&file);
    for s in &mut secs {
        if s.0 == TAG_SELM {
            assert_eq!(s.1, SEC_SAFE_TO_COPY);
            s.2.truncate(s.2.len() - 3);
        }
    }
    let l = read(&Raw::new().manifest_raw(&manifest_raw(&secs)), &pool);
    assert!(!l.doc.has_selection());
    assert!(matches!(l.warnings.as_slice(), [LoadWarning::SelectionDropped { .. }]), "{:?}", l.warnings);
    assert_eq!(l.read_only_reason, None, "a dropped selection never makes the load lossy");
}

#[test]
fn sc11_encode_is_cached_per_revision() {
    let (w, h) = (300, 200);
    let sel = mixed(w, h, 5);
    let mut cache = SelmCache::default();
    let (a, _) = selm::encode(&sel, 7, w, h, &mut cache);
    let (b, _) = selm::encode(&sel.clone(), 7, w, h, &mut cache);
    assert!(Arc::ptr_eq(a.as_ref().unwrap(), b.as_ref().unwrap()), "same rev, same Arc");
    let (c, _) = selm::encode(&sel, 8, w, h, &mut cache);
    assert!(!Arc::ptr_eq(a.as_ref().unwrap(), c.as_ref().unwrap()), "a new rev re-encodes");
    assert_eq!(a.as_deref(), c.as_deref());
    // Another selection that happens to carry the same rev never hits.
    let other = mixed(w, h, 6);
    let (d, _) = selm::encode(&other, 8, w, h, &mut cache);
    assert_ne!(d.as_deref(), c.as_deref());

    // Autosave rewrites the manifest each commit; the body is reused.
    let pool = pool();
    let p = Progress::default();
    let mut doc = Document::new(w, h, 350);
    doc.swap_selection(sel.clone());
    let mut fw = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    let s1 = fw.commit(&doc, &SaveExtras::default(), &meta(&doc), &opts(), &pool, &p).unwrap();
    assert_eq!(s1.selection_saved, SelectionSave::Exact);
    doc.swap_selection(Selection::new());
    let s2 = fw.commit(&doc, &SaveExtras::default(), &meta(&doc), &opts(), &pool, &p).unwrap();
    assert_eq!(s2.selection_saved, SelectionSave::None);
    assert!(!s2.unchanged, "deselecting is a change");
    let file = fw.into_sink();
    assert!(!read(&file, &pool).doc.has_selection());
}

/// Incompressible soft tiles over a `w`×`h` page.
fn noise(w: u32, h: u32, seed: u64) -> Selection {
    let mut rng = Rng(seed);
    let mut s = Selection::new();
    for ty in 0..h.div_ceil(64) as i32 {
        for tx in 0..w.div_ceil(64) as i32 {
            let mut m: MaskPixels = [[0; T]; T];
            for v in m.as_flattened_mut() {
                *v = (rng.next() >> 24) as u8 | 1;
            }
            s.insert_tile(TileCoord::new(tx, ty), Arc::new(m));
        }
    }
    s
}

#[test]
fn sc11_over_the_cap_binarizes_then_drops() {
    // 4096 noisy tiles: 16 MiB of U8, 2 MiB once binarized.
    let (w, h) = (4096, 4096);
    let sel = noise(w, h, 9);
    let (body, save) = encode(&sel, w, h);
    assert_eq!(save, SelectionSave::Binarized);
    let body = body.unwrap();
    assert!(body.len() <= MAX_SELM_BYTES);
    let back = selm::decode(&body, w, h, &mut Vec::new()).unwrap();
    for (y, x) in [(0, 0), (100, 2000), (4095, 4095), (1234, 77)] {
        assert_eq!(back.value(x, y), if sel.value(x, y) >= 128 { 255 } else { 0 }, "({x}, {y})");
    }
    assert!(back.tiles().all(|(_, m)| m.as_flattened().iter().all(|&v| v == 0 || v == 255)));

    // 16 384 noisy tiles: still over 8 MiB when binarized.
    let (w, h) = (8192, 8192);
    let sel = noise(w, h, 10);
    assert_eq!(encode(&sel, w, h), (None, SelectionSave::Dropped));
    let pool = pool();
    let mut doc = Document::new(w, h, 350);
    doc.swap_selection(sel);
    let mut fw = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    let stats = fw.commit(&doc, &SaveExtras::default(), &meta(&doc), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!(stats.selection_saved, SelectionSave::Dropped);
    let file = fw.into_sink();
    assert!(sections(&file).iter().all(|s| s.0 != TAG_SELM), "saved without the selection");
    assert!(!read(&file, &pool).doc.has_selection());
}

#[test]
fn sc11_old_reader_keeps_selm_byte_identical() {
    let pool = pool();
    let (w, h) = (300, 200);
    let sel = mixed(w, h, 12);
    let mut doc = Document::new(w, h, 350);
    doc.swap_selection(sel.clone());
    let file = write(&doc, &SaveExtras::default(), &pool);
    let secs = sections(&file);
    let selm: Vec<_> = secs.iter().filter(|s| s.0 == TAG_SELM).collect();
    assert_eq!(selm.len(), 1);
    let (_, flags, body) = selm[0].clone();
    assert_eq!(flags, SEC_SAFE_TO_COPY, "SAFE_TO_COPY, never CRITICAL");
    assert_eq!(flags & SEC_CRITICAL, 0);

    // A v2.0 reader does not know SELM: stand in for it by renaming the
    // tag to one this build does not know either (decoder bypassed).
    const UNKNOWN: [u8; 4] = *b"SELm";
    let renamed: Vec<_> = secs.iter().map(|s| if s.0 == TAG_SELM { (UNKNOWN, s.1, s.2.clone()) } else { s.clone() }).collect();
    let old = read(&Raw::new().manifest_raw(&manifest_raw(&renamed)), &pool);
    assert!(!old.doc.has_selection());
    assert_eq!(old.warnings, [LoadWarning::SkippedSection { tag: UNKNOWN }]);
    assert_eq!(old.read_only_reason, None, "kept, so the file stays editable");
    assert_eq!(old.extra_sections, [AppSection { tag: UNKNOWN, flags, bytes: body.clone() }]);
    // It saves the section back as it read it.
    let ex = SaveExtras { sections: old.extra_sections.clone(), ..Default::default() };
    let resaved = sections(&write(&old.doc, &ex, &pool));
    let kept: Vec<_> = resaved.iter().filter(|s| s.0 == UNKNOWN).collect();
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].1, &kept[0].2), (flags, &body), "byte-identical");
    // And this build reads it again in full.
    let mut warn = Vec::new();
    assert_same_selection(&selm::decode(&kept[0].2, w, h, &mut warn).unwrap(), &sel);

    // A stale SELM in the extras is never written next to the fresh one.
    let stale = SaveExtras { sections: vec![AppSection { tag: TAG_SELM, flags, bytes: b"stale".to_vec() }], ..Default::default() };
    let secs = sections(&write(&doc, &stale, &pool));
    let selm: Vec<_> = secs.iter().filter(|s| s.0 == TAG_SELM).collect();
    assert_eq!(selm.len(), 1);
    assert_eq!(selm[0].2, body);
}

#[test]
fn sc11_random_bytes_never_panic() {
    let (w, h) = (300, 200);
    let mut rng = Rng(0xF00D);
    let valid: Vec<Vec<u8>> = (0..4).map(|s| encode(&mixed(w, h, 20 + s), w, h).0.unwrap().to_vec()).collect();
    for i in 0..3000 {
        let mut b = if i % 3 == 0 {
            (0..rng.below(200)).map(|_| rng.next() as u8).collect::<Vec<u8>>()
        } else {
            valid[i % valid.len()].clone()
        };
        if i % 3 != 0 {
            // Flip a few bytes, sometimes in the header, sometimes cut short.
            for _ in 0..1 + rng.below(4) {
                let at = if rng.chance(1, 2) { rng.below(20) } else { rng.below(b.len() as u64) } as usize;
                b[at] = rng.next() as u8;
            }
            if rng.chance(1, 4) {
                b.truncate(rng.below(b.len() as u64) as usize);
            }
        } else if b.len() >= 20 && rng.chance(1, 2) {
            // A plausible header over random payload.
            b[0] = 1;
            b[1] = rng.below(2) as u8;
            b[4..8].copy_from_slice(&w.to_le_bytes());
            b[8..12].copy_from_slice(&h.to_le_bytes());
            b[12..16].copy_from_slice(&(rng.below(30) as u32).to_le_bytes());
            b[16..20].copy_from_slice(&(rng.below(5000) as u32).to_le_bytes());
        }
        let mut warn = Vec::new();
        match selm::decode(&b, w, h, &mut warn) {
            Some(s) => {
                assert!(warn.is_empty());
                let (tw, th) = (w.div_ceil(64) as i32, h.div_ceil(64) as i32);
                assert!(s.tiles().all(|(c, _)| c.x >= 0 && c.y >= 0 && c.x < tw && c.y < th));
            }
            None => assert!(matches!(warn.as_slice(), [LoadWarning::SelectionDropped { .. }])),
        }
    }
}

/// SELM size and timings for B006 (not a test; run explicitly):
/// `cargo test -p arty-io --release --test selm -- --ignored --nocapture sc11_bench`
#[test]
#[ignore]
fn sc11_bench_selm_size_and_time() {
    use std::time::Instant;
    let (w, h) = (6071u32, 8598u32);
    let lasso: Vec<[f32; 2]> = (0..5445)
        .map(|i| {
            let t = i as f32 / 5445.0 * std::f32::consts::TAU;
            let k = 1.0 + 0.15 * (0.6 * (5.0 * t).sin() + 0.3 * (13.0 * t + 1.0).cos() + 0.1 * (41.0 * t).sin());
            [3000.0 + 2200.0 * k * t.cos(), 4300.0 + 3000.0 * k * t.sin()]
        })
        .collect();
    let typical = arty_core::raster::rasterize_polygon(&lasso, w, h, true);
    let feathered = arty_core::morph::feather(&typical, 16, w, h);
    // Wand on screentone: hard 0/255 dots in every tile.
    let mut dots: MaskPixels = [[0; T]; T];
    for (y, row) in dots.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            let (dx, dy) = ((x % 16) as f32 - 7.5, (y % 16) as f32 - 7.5);
            *v = if dx * dx + dy * dy < 25.0 { 255 } else { 0 };
        }
    }
    let mut tone = Selection::new();
    let mut soft = Selection::new();
    let mut rng = Rng(1);
    for ty in 0..h.div_ceil(64) as i32 {
        for tx in 0..w.div_ceil(64) as i32 {
            // Each tile its own Arc, as a wand fill makes them.
            tone.insert_tile(TileCoord::new(tx, ty), Arc::new(dots));
            let mut m: MaskPixels = [[0; T]; T];
            for v in m.as_flattened_mut() {
                *v = (rng.next() >> 24) as u8 | 1;
            }
            soft.insert_tile(TileCoord::new(tx, ty), Arc::new(m));
        }
    }
    println!("| selection | tiles | partial | result | body KiB | encode ms | decode ms |");
    println!("|---|---:|---:|---|---:|---:|---:|");
    let cases =
        [("select all", Selection::all(w, h)), ("typical lasso", typical), ("lasso feathered σ=16", feathered), ("wand on screentone (BIT)", tone), ("noise, every tile soft (worst)", soft)];
    for (name, sel) in cases {
        let mut enc = f64::MAX;
        let mut out = (None, SelectionSave::None);
        for _ in 0..5 {
            let t = Instant::now();
            out = selm::encode(&sel, 1, w, h, &mut SelmCache::default());
            enc = enc.min(t.elapsed().as_secs_f64() * 1e3);
        }
        let (body, save) = out;
        let (kib, dec) = match &body {
            Some(b) => {
                let mut dec = f64::MAX;
                for _ in 0..5 {
                    let t = Instant::now();
                    let s = selm::decode(b, w, h, &mut Vec::new()).unwrap();
                    dec = dec.min(t.elapsed().as_secs_f64() * 1e3);
                    drop(s);
                }
                (format!("{:.1}", b.len() as f64 / 1024.0), format!("{dec:.2}"))
            }
            None => ("—".into(), "—".into()),
        };
        println!("| {name} | {} | {} | {save:?} | {kib} | {enc:.2} | {dec} |", sel.tile_count(), sel.byte_size() / 4096);
    }
    let mut cache = SelmCache::default();
    let sel = Selection::all(w, h);
    selm::encode(&sel, 1, w, h, &mut cache);
    let t = Instant::now();
    selm::encode(&sel, 1, w, h, &mut cache);
    println!("cached re-encode (same rev): {:.4} ms", t.elapsed().as_secs_f64() * 1e3);
}
