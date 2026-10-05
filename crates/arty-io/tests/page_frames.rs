//! FRAMES: the page setup (`PSET`) and frame border panels (`FRAM` LEXT
//! entries) round-trip, survive older readers, and never fail a load.

mod common;

use arty_core::frame::add_frame_folder;
use arty_core::{BorderStyle, Document, FrameShape, LayerId, PageSetup, Panel, RectF, TileCoord};
use arty_io::format::{Commit, RecordHeader, RecordKind};
use arty_io::manifest::{
    self, LEXT_FRAM, SEC_SAFE_TO_COPY, SectionWriter, TAG_LEXT, TAG_PSET, decode_payload, encode_payload, lext_body,
};
use arty_io::{AppSection, LayerExt, LoadOptions, LoadWarning, Progress, SaveExtras, Session, fram, pset};
use common::*;

fn page() -> PageSetup {
    PageSetup {
        trim: RectF { x: 20.0, y: 16.0, w: 200.0, h: 280.0 },
        bleed: 12.0,
        safe: 9.5,
        inner: RectF { x: 40.0, y: 40.0, w: 160.0, h: 230.0 },
        unit: 0,
    }
}

fn panel(x: f32, y: f32, w: f32, h: f32) -> Panel {
    Panel::rect(RectF { x, y, w, h }).unwrap()
}

/// A 240×320 page with a page setup, two frame folders (one with a bleed
/// panel and a rotated one), a plain folder and painted rasters.
fn doc_with_frames() -> (Document, [LayerId; 3]) {
    let mut doc = Document::new(240, 320, 350);
    let base = doc.active();
    doc.paint_target(base).unwrap().0.insert(TileCoord::new(1, 1), filled([ONE, ONE / 2, 0, ONE]));
    let tilted = Panel::new(vec![[30.0, 200.0], [210.0, 190.0], [220.0, 300.0], [20.0, 310.5]]).unwrap();
    let border = BorderStyle { width: 3.25, color: [0, 0, ONE / 2, ONE / 2] };
    let a = add_frame_folder(&mut doc, FrameShape { panels: vec![panel(-4.0, -4.0, 130.0, 150.0), tilted], border })
        .unwrap();
    let art = doc.active();
    doc.paint_target(art).unwrap().0.insert(TileCoord::new(0, 0), filled([0, 0, 0, ONE]));
    doc.set_active(a);
    let b = add_frame_folder(
        &mut doc,
        FrameShape {
            panels: vec![panel(130.0, 10.0, 100.0, 140.0)],
            border: BorderStyle { width: 0.0, color: [0, 0, 0, ONE] },
        },
    )
    .unwrap();
    doc.set_active(b);
    let plain = doc.add_folder().unwrap();
    doc.set_page_unrecorded(Some(page()));
    (doc, [a, b, plain])
}

/// `(tag, flags, body)` of every section of the newest manifest in `file`.
fn sections(file: &[u8]) -> Vec<([u8; 4], u32, Vec<u8>)> {
    let (at, _, end) = *records(file).iter().rev().find(|r| r.1 == RecordKind::Manifest as u8).unwrap();
    let raw = decode_payload(&file[at as usize + 24..end as usize], at).unwrap().into_owned();
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let tag: [u8; 4] = raw[i..i + 4].try_into().unwrap();
        let flags = u32::from_le_bytes(raw[i + 4..i + 8].try_into().unwrap());
        let len = u32::from_le_bytes(raw[i + 8..i + 12].try_into().unwrap()) as usize;
        out.push((tag, flags, raw[i + 12..i + 12 + len].to_vec()));
        i += 12 + len;
    }
    out
}

fn lext(file: &[u8]) -> Vec<LayerExt> {
    let mut w = SectionWriter::default();
    for (tag, flags, body) in sections(file) {
        w.push(tag, flags, &body);
    }
    let raw = w.into_raw();
    manifest::parse(&raw, 0, 1 << 16).unwrap().layer_ext
}

/// Append a commit whose manifest is `file`'s newest one passed through
/// `f`, as another version saving the same tiles would.
fn recommit(file: &[u8], f: impl FnOnce(&mut Vec<([u8; 4], u32, Vec<u8>)>)) -> Vec<u8> {
    let mut s = sections(file);
    f(&mut s);
    let mut w = SectionWriter::default();
    for (tag, flags, body) in &s {
        w.push(*tag, *flags, body);
    }
    let (prev, _, _) = *records(file).last().unwrap();
    let mut out = file.to_vec();
    let payload = encode_payload(&w.into_raw());
    let m = out.len() as u64;
    out.extend_from_slice(&RecordHeader::for_payload(RecordKind::Manifest, &payload).encode());
    out.extend_from_slice(&payload);
    out.extend_from_slice(
        &Commit { manifest_offset: m, prev_commit_offset: prev, commit_seq: 2, unix_ms: NOW }.encode_record(),
    );
    out
}

fn set_lext(s: &mut Vec<([u8; 4], u32, Vec<u8>)>, entries: &[LayerExt]) {
    s.retain(|x| x.0 != TAG_LEXT);
    s.push((TAG_LEXT, SEC_SAFE_TO_COPY, lext_body(entries.iter())));
}

#[test]
fn fr11_round_trip_through_save_autosave_and_compaction() {
    let pool = pool();
    let p = Progress::default();
    let dir = temp_dir("page-frames");
    let (mut doc, [a, ..]) = doc_with_frames();
    let main = dir.join("frames.arty");
    let ex = SaveExtras::default();
    Session::new(session(), None).save_main(&doc, &ex, &main, false, &opts(), &pool, &p).unwrap();
    let check = |path: &std::path::Path, doc: &Document, what: &str| {
        let o = LoadOptions { fallback_to_previous: true, ..Default::default() };
        let l = arty_io::load(path, &o, &pool, &p).unwrap();
        assert!(l.warnings.is_empty(), "{what}: {:?}", l.warnings);
        assert_eq!(l.read_only_reason, None, "{what}");
        assert!(l.layer_ext.is_empty(), "{what}: decoded entries are consumed");
        assert_same_doc(doc, &l.doc);
        assert_eq!(l.doc.page_setup(), Some(&page()), "{what}");
        assert!(l.doc.frame(a).is_some(), "{what}");
    };
    check(&main, &doc, "save");

    let mut loaded = arty_io::load(&main, &LoadOptions::default(), &pool, &p).unwrap();
    let mut s = Session::new(arty_io::SessionId([0x42; 16]), Some(&dir.join("recovery")));
    s.adopt(&mut loaded, Some(main.clone()));
    let again = SaveExtras {
        view: loaded.view.clone(),
        sections: loaded.extra_sections.clone(),
        layer_ext: loaded.layer_ext.clone(),
        title: String::new(),
    };
    let recovery = s.recovery_path().unwrap();
    s.autosave(&doc, &again, 1, &pool, &p).unwrap();
    check(&recovery, &doc, "autosave (rewrite)");
    // Edit a frame and the page: the appended commit carries both.
    let mut shape = doc.frame(a).unwrap().shape().clone();
    shape.panels.truncate(1);
    shape.border.width = 7.0;
    doc.set_frame(a, Some(arty_core::Frame::build(shape, 240, 320)));
    let mut pg = page();
    pg.bleed = 3.0;
    doc.set_page_setup(Some(pg));
    s.autosave(&doc, &again, 2, &pool, &p).unwrap();
    let l = arty_io::load(&recovery, &LoadOptions::default(), &pool, &p).unwrap();
    assert_same_doc(&doc, &l.doc);
    s.compact(&doc, &again, 2, &pool, &p).unwrap();
    let l = arty_io::load(&recovery, &LoadOptions::default(), &pool, &p).unwrap();
    assert_same_doc(&doc, &l.doc);
    assert_eq!(l.doc.page_setup().unwrap().bleed, 3.0, "compaction");
    // Removing them removes the data.
    doc.set_frame(a, None);
    doc.set_page_setup(None);
    s.save_main(&doc, &again, &main, false, &opts(), &pool, &p).unwrap();
    let l = arty_io::load(&main, &LoadOptions::default(), &pool, &p).unwrap();
    assert_same_doc(&doc, &l.doc);
    assert!(l.doc.frame(a).is_none() && l.doc.page_setup().is_none());
    s.close(true).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fr11_flags_and_stale_copies() {
    let pool = pool();
    let (doc, [a, b, plain]) = doc_with_frames();
    // A stale PSET in the extras (as an older build would keep it) is
    // never written next to the document's own.
    let stale = AppSection { tag: TAG_PSET, flags: SEC_SAFE_TO_COPY, bytes: vec![1; 44] };
    let ex = SaveExtras { sections: vec![stale], ..Default::default() };
    let file = write(&doc, &ex, &pool);
    let s = sections(&file);
    let psets: Vec<_> = s.iter().filter(|x| x.0 == TAG_PSET).collect();
    assert_eq!(psets.len(), 1);
    assert_eq!(psets[0].1, SEC_SAFE_TO_COPY, "PSET is SAFE_TO_COPY");
    assert_eq!(psets[0].2, pset::encode(Some(&page())).unwrap());
    let entries = lext(&file);
    let frams: Vec<_> = entries.iter().filter(|e| e.tag == LEXT_FRAM).collect();
    assert_eq!(
        frams.iter().map(|e| e.layer).collect::<Vec<_>>(),
        [a.0, b.0],
        "one per frame folder, none for {plain:?}"
    );
    assert!(frams.iter().all(|e| e.flags == 0), "FRAM entries are not CRITICAL");
    assert_eq!(s.iter().find(|x| x.0 == TAG_LEXT).unwrap().1, SEC_SAFE_TO_COPY);

    let l = read(&file, &pool);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.read_only_reason, None, "not lossy");
    assert_same_doc(&doc, &l.doc);
    // Reloaded and resaved, the file still parses.
    let ex = SaveExtras {
        view: l.view.clone(),
        sections: l.extra_sections.clone(),
        layer_ext: l.layer_ext.clone(),
        title: String::new(),
    };
    let again = write(&l.doc, &ex, &pool);
    assert_eq!(sections(&again).iter().filter(|x| x.0 == TAG_PSET).count(), 1);
    assert_same_doc(&doc, &read(&again, &pool).doc);
}

#[test]
fn fr11_older_readers_keep_the_bytes() {
    let pool = pool();
    let (doc, [a, b, _]) = doc_with_frames();
    let file = write(&doc, &SaveExtras::default(), &pool);
    let pset_bytes = sections(&file).into_iter().find(|x| x.0 == TAG_PSET).unwrap();
    let frams: Vec<LayerExt> = lext(&file).into_iter().filter(|e| e.tag == LEXT_FRAM).collect();

    // A v2.0 build: the decoders do not exist, so it opens the document
    // without frames or page setup, keeps the FRAM entries in `layer_ext`
    // and PSET as an unknown SAFE_TO_COPY section, and writes both back.
    let mut old = read(&file, &pool).doc;
    old.set_frame(a, None);
    old.set_frame(b, None);
    old.set_page_unrecorded(None);
    let kept = SaveExtras { layer_ext: frams.clone(), ..Default::default() };
    let resaved = write(&old, &kept, &pool);
    assert_eq!(
        lext(&resaved).into_iter().filter(|e| e.tag == LEXT_FRAM).collect::<Vec<_>>(),
        frams,
        "FRAM byte-identical"
    );
    // Its writer puts unknown sections after its own (KNOWN_TAGS of 2.0
    // has no PSET); copy that commit's manifest the same way.
    let resaved = recommit(&resaved, |s| s.push(pset_bytes.clone()));
    assert_eq!(sections(&resaved).into_iter().find(|x| x.0 == TAG_PSET), Some(pset_bytes), "PSET byte-identical");

    // This version reads both back.
    let l = read(&resaved, &pool);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_same_doc(&doc, &l.doc);
}

#[test]
fn fr11_unknown_versions_and_bad_entries() {
    let pool = pool();
    let (doc, [a, b, plain]) = doc_with_frames();
    let raster = doc.layer(a).unwrap().children().unwrap()[0];
    let file = write(&doc, &SaveExtras::default(), &pool);
    let good = fram::encode(doc.frame(b).unwrap().shape());

    // An unknown version stays opaque (and the folder plain).
    let mut v9 = good.clone();
    v9[0] = 9;
    let future = LayerExt { layer: plain.0, tag: LEXT_FRAM, flags: 0, bytes: v9 };
    let f = recommit(&file, |s| set_lext(s, std::slice::from_ref(&future)));
    let l = read(&f, &pool);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.layer_ext, std::slice::from_ref(&future));
    assert!(l.doc.frame(plain).is_none() && l.doc.frame(a).is_none());
    // Saved again, it is written back unchanged.
    let ex = SaveExtras { layer_ext: l.layer_ext.clone(), ..Default::default() };
    assert_eq!(lext(&write(&l.doc, &ex, &pool)), [future]);

    // On a raster, a missing id, or twice for one folder: dropped with a warning.
    let on = |layer: u32| LayerExt { layer, tag: LEXT_FRAM, flags: 0, bytes: good.clone() };
    let f = recommit(&file, |s| set_lext(s, &[on(raster.0), on(9999), on(b.0), on(b.0)]));
    let l = read(&f, &pool);
    assert_eq!(
        l.warnings,
        [
            LoadWarning::FrameDropped { layer: raster.0, reason: "not a folder" },
            LoadWarning::FrameDropped { layer: 9999, reason: "no such layer" },
            LoadWarning::FrameDropped { layer: b.0, reason: "a second frame for the folder" },
        ]
    );
    assert!(l.layer_ext.is_empty(), "dropped, not kept");
    assert_eq!(l.read_only_reason, None);
    assert_eq!(l.doc.frame(b).unwrap().shape(), doc.frame(b).unwrap().shape());

    // An invalid panel is dropped, the others kept.
    let mut shape = doc.frame(a).unwrap().shape().clone();
    let copy = fram::encode(&FrameShape { panels: vec![shape.panels[0].clone()], border: shape.border });
    let mut bytes = fram::encode(&shape);
    bytes[2] = 3; // three panels: the two, then a concave one
    bytes.extend_from_slice(&[4, 0]);
    for [x, y] in [[0.0f32, 0.0], [50.0, 0.0], [10.0, 10.0], [0.0, 50.0]] {
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
    }
    // And a panel past the bleed range (more than a page outside).
    bytes[2] = 4;
    bytes.extend_from_slice(&copy[16..]);
    let far = bytes.len() - 32;
    bytes[far..far + 4].copy_from_slice(&(-500.0f32).to_le_bytes());
    let f = recommit(&file, |s| set_lext(s, &[LayerExt { layer: a.0, tag: LEXT_FRAM, flags: 0, bytes }]));
    let l = read(&f, &pool);
    assert_eq!(l.warnings, [LoadWarning::FramePanelDropped { layer: a.0, count: 2 }]);
    shape.panels.truncate(2);
    assert_eq!(l.doc.frame(a).unwrap().shape(), &shape);

    // An invalid page setup is dropped with a warning; the file is not lossy.
    let mut bad = page();
    bad.trim.x = f32::NAN;
    let f = recommit(&file, |s| {
        s.retain(|x| x.0 != TAG_PSET);
        s.push((TAG_PSET, SEC_SAFE_TO_COPY, pset::encode(Some(&bad)).unwrap()));
    });
    let l = read(&f, &pool);
    assert_eq!(l.warnings, [LoadWarning::PageSetupDropped]);
    assert_eq!(l.doc.page_setup(), None);
    assert_eq!(l.read_only_reason, None);
    let mut outside = page();
    outside.trim.w = 1000.0;
    let mut short = pset::encode(Some(&page())).unwrap();
    short.truncate(40);
    for body in [pset::encode(Some(&outside)).unwrap(), short] {
        let f = recommit(&file, |s| {
            s.retain(|x| x.0 != TAG_PSET);
            s.push((TAG_PSET, SEC_SAFE_TO_COPY, body));
        });
        assert_eq!(read(&f, &pool).warnings, [LoadWarning::PageSetupDropped]);
    }
}

#[test]
fn fr11_fuzzed_entries_never_panic() {
    let mut rng = Rng(0xF2A3);
    let page_w = 600;
    let good = fram::encode(&FrameShape {
        panels: vec![panel(10.0, 10.0, 200.0, 100.0), panel(-50.0, 120.0, 400.0, 300.0)],
        border: BorderStyle { width: 4.0, color: [0, 0, 0, ONE] },
    });
    let pset_good = pset::encode(Some(&page())).unwrap();
    let mut warn = Vec::new();
    for i in 0..20_000 {
        let mut bytes = if i % 2 == 0 { good.clone() } else { pset_good.clone() };
        match rng.below(4) {
            // Random bytes after a valid version.
            0 => {
                let n = rng.below(200) as usize;
                bytes = (0..n).map(|_| rng.next() as u8).collect();
                if let Some(b) = bytes.first_mut() {
                    *b = 1;
                }
            }
            // Flipped bytes.
            1 | 2 => {
                for _ in 0..1 + rng.below(6) {
                    let at = rng.below(bytes.len() as u64) as usize;
                    bytes[at] = rng.next() as u8;
                }
            }
            // Truncated or extended.
            _ => {
                let n = rng.below(bytes.len() as u64 + 20) as usize;
                bytes.resize(n, rng.next() as u8);
            }
        }
        if let Ok((shape, _)) = fram::decode(&bytes, page_w, page_w) {
            assert!(shape.panels.len() <= 1024);
            arty_core::Frame::build(shape, 64, 64);
        }
        let _ = pset::decode(&bytes, page_w, page_w, &mut warn);
        warn.clear();
    }
    // Through `apply` as a reader calls it.
    let mut doc = Document::new(128, 128, 72);
    let folder = doc.add_folder().unwrap();
    let mut layers: Vec<arty_core::Layer> =
        [doc.root()[0], folder].iter().map(|id| doc.layer(*id).unwrap().clone()).collect();
    for _ in 0..2_000 {
        let n = rng.below(80) as usize;
        let mut bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        if let Some(b) = bytes.first_mut() {
            *b = 1;
        }
        let mut ext = vec![LayerExt { layer: folder.0, tag: LEXT_FRAM, flags: 0, bytes }];
        fram::apply(&mut layers, &mut ext, 128, 128, &mut warn);
        warn.clear();
    }
}

/// B010: what 20 frame folders add to a load (B4 600 dpi, 6 panels and a
/// 0.6 mm border each).
/// `cargo test -p arty-io --release --test page_frames -- --ignored --nocapture`
#[test]
#[ignore]
fn fr_bench_load_20_frame_folders() {
    let pool = rayon::ThreadPoolBuilder::new().build().unwrap();
    let (w, h) = (6071, 8598);
    let border = BorderStyle { width: 14.2, color: [0, 0, 0, ONE] };
    let mut doc = Document::new(w, h, 600);
    for k in 0..20 {
        let base = FrameShape { panels: vec![panel(900.0 + k as f32 * 7.0, 1100.0, 4250.0, 6370.0)], border };
        let (mut shape, _) = base.cut([0.0, 3200.0], [6071.0, 3500.0], 118.0, 47.0).unwrap();
        shape = shape.cut([0.0, 5300.0], [6071.0, 5300.0], 118.0, 47.0).unwrap().0;
        shape = shape.cut([3030.0, 0.0], [3030.0, 8598.0], 118.0, 47.0).unwrap().0;
        assert_eq!(shape.panels.len(), 6);
        doc.set_active(doc.root()[0]);
        add_frame_folder(&mut doc, shape).unwrap();
    }
    let with = write(&doc, &SaveExtras::default(), &pool);
    let mut plain = doc.snapshot();
    let folders: Vec<LayerId> = plain.root().iter().copied().filter(|&id| plain.frame(id).is_some()).collect();
    assert_eq!(folders.len(), 20);
    for id in folders {
        plain.set_frame(id, None);
    }
    let without = write(&plain, &SaveExtras::default(), &pool);
    let time = |file: &[u8]| {
        let mut t: Vec<f64> = (0..9)
            .map(|_| {
                let start = std::time::Instant::now();
                let l = read(file, &pool);
                let ms = start.elapsed().as_secs_f64() * 1e3;
                drop(l);
                ms
            })
            .collect();
        t.sort_by(f64::total_cmp);
        (t[0], t[t.len() / 2])
    };
    let (a, b) = (time(&without), time(&with));
    println!("threads: {}", pool.current_num_threads());
    println!("load without frames: {:.1} / {:.1} ms (min / median)", a.0, a.1);
    println!("load with 20 frame folders: {:.1} / {:.1} ms; adds {:.1} ms at min", b.0, b.1, b.0 - a.0);
    println!("file sizes: {} / {} bytes", without.len(), with.len());
}
