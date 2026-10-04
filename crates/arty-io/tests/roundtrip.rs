//! R1: save → load gives back the same document, and saving the loaded
//! document again gives the same bytes.

mod common;

use std::sync::Arc;

use arty_core::{Document, LayerContent, LayerId, TileCoord};
use arty_io::format::{T_MAX, T_MIN};
use arty_io::{
    AppSection, FileKind, LayerExt, LoadWarning, Progress, SaveExtras, Session, manifest::SEC_SAFE_TO_COPY,
};
use common::*;

#[test]
fn five_hundred_seeded_documents_round_trip() {
    let pool = pool();
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let dir = temp_dir("roundtrip");
    let mut session = Session::new(session(), None);
    // [domain-edge tile, active folder, 4096-byte name, no paper]
    let mut seen = [0u32; 4];
    for i in 0..500 {
        let doc = random_doc(&mut rng);
        for id in layer_ids(&doc) {
            let l = doc.layer(id).unwrap();
            let edge = |c: TileCoord| [c.x, c.y].iter().any(|v| [T_MIN, T_MAX].contains(v));
            seen[0] += l.raster().is_some_and(|g| g.coords().any(edge)) as u32;
            seen[1] += (l.is_folder() && doc.active() == id) as u32;
            seen[2] += (l.props.name.len() == 4096) as u32;
        }
        seen[3] += doc.paper().is_none() as u32;
        let bytes = write(&doc, &SaveExtras::default(), &pool);
        let loaded = read(&bytes, &pool);
        assert!(loaded.warnings.is_empty(), "doc {i}: {:?}", loaded.warnings);
        assert_eq!(loaded.read_only_reason, None);
        assert_eq!(loaded.info.commit_seq, 1);
        assert_eq!(loaded.info.saved_ms, NOW);
        assert!(!loaded.info.recovered);
        assert_same_doc(&doc, &loaded.doc);
        // Determinism: the loaded document saves to the same bytes.
        let again = write(&loaded.doc, &SaveExtras::default(), &pool);
        assert!(again == bytes, "doc {i}: resave differs");

        // Every 25th through the real save and load path on disk.
        if i % 25 == 0 {
            let path = dir.join(format!("doc{i}.arty"));
            let stats = session.save_main(&doc, &SaveExtras::default(), &path, false, &opts(), &pool, &Progress::default()).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "doc {i}: file differs from the in-memory save");
            assert_eq!(stats.file_len, bytes.len() as u64);
            let from_disk = arty_io::load(&path, &Default::default(), &pool, &Progress::default()).unwrap();
            assert_same_doc(&doc, &from_disk.doc);
            let info = arty_io::read_info(&path).unwrap();
            assert_eq!(info.kind, FileKind::V2 { minor: 0 });
            assert_eq!((info.width, info.height, info.dpi), (doc.width(), doc.height(), doc.dpi()));
            assert_eq!(info.layer_count as usize, doc.layer_count());
        }
    }
    assert!(seen.iter().all(|&n| n > 0), "generator coverage {seen:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn shared_and_equal_tiles_are_stored_once() {
    let pool = pool();
    let mut rng = Rng(7);
    let mut doc = Document::new(256, 256, 350);
    let a = doc.active();
    let noise = tile(&mut rng, 2);
    let copy = {
        let mut t = arty_core::tile::new_tile();
        *Arc::get_mut(&mut t).unwrap() = *noise;
        t
    };
    let b = doc.add_raster_layer();
    {
        let (g, _) = doc.paint_target(a).unwrap();
        g.insert(TileCoord::new(0, 0), noise.clone());
        g.insert(TileCoord::new(1, 0), noise.clone());
        g.insert(TileCoord::new(2, 0), filled([1, 2, 3, 4]));
    }
    {
        let (g, _) = doc.paint_target(b).unwrap();
        g.insert(TileCoord::new(0, 0), noise.clone());
        g.insert(TileCoord::new(5, 5), copy);
        g.insert(TileCoord::new(6, 5), filled([1, 2, 3, 4]));
    }
    let mut w = arty_io::FileWriter::create(Vec::new(), 0, UUID).unwrap();
    let stats = w.commit(&doc, &SaveExtras::default(), &meta(&doc), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!((stats.tiles, stats.classified, stats.encoded, stats.reused), (6, 4, 1, 3));
    assert_eq!(stats.tables_written, 2);
    let loaded = read(&w.into_sink(), &pool);
    let ga = loaded.doc.layer(a).unwrap().raster().unwrap();
    let gb = loaded.doc.layer(b).unwrap().raster().unwrap();
    let first = ga.get_ref(TileCoord::new(0, 0)).unwrap();
    for (g, c) in [(ga, (1, 0)), (gb, (0, 0)), (gb, (5, 5))] {
        assert!(Arc::ptr_eq(first, g.get_ref(TileCoord::new(c.0, c.1)).unwrap()), "{c:?} shares the blob");
    }
    let solid = ga.get_ref(TileCoord::new(2, 0)).unwrap();
    assert!(Arc::ptr_eq(solid, gb.get_ref(TileCoord::new(6, 5)).unwrap()), "one Arc per SOLID value");
}

#[test]
fn crc_collision_keeps_both_tiles() {
    // Two different tiles with the same crc32: the last four bytes of the
    // second are chosen to force its crc to the first one's.
    let pool = pool();
    let mut rng = Rng(99);
    let t1 = tile(&mut rng, 2);
    let mut t2 = tile(&mut rng, 3);
    let want = crc32fast::hash(bytemuck::bytes_of(&*t1));
    force_crc(bytemuck::bytes_of_mut(Arc::get_mut(&mut t2).unwrap()), want);
    assert_eq!(crc32fast::hash(bytemuck::bytes_of(&*t2)), want);
    assert!(*t1 != *t2);
    let mut doc = Document::new(128, 128, 72);
    let (g, _) = doc.paint_target(doc.active()).unwrap();
    g.insert(TileCoord::new(0, 0), t1.clone());
    g.insert(TileCoord::new(1, 0), t2.clone());
    let bytes = write(&doc, &SaveExtras::default(), &pool);
    let loaded = read(&bytes, &pool);
    assert_same_doc(&doc, &loaded.doc);
    let g = loaded.doc.active_layer().raster().unwrap();
    assert!(!Arc::ptr_eq(g.get_ref(TileCoord::new(0, 0)).unwrap(), g.get_ref(TileCoord::new(1, 0)).unwrap()));
}

#[test]
fn extras_round_trip_and_stale_layer_ext_is_dropped() {
    let pool = pool();
    let doc = Document::new(64, 64, 300);
    let id = doc.active().0;
    let ex = SaveExtras {
        view: Some(vec![1, 0, 0, 128, 63]),
        sections: vec![
            AppSection { tag: *b"ZZZ1", flags: SEC_SAFE_TO_COPY, bytes: b"future".to_vec() },
            // A known tag is never written twice.
            AppSection { tag: *b"META", flags: SEC_SAFE_TO_COPY, bytes: b"no".to_vec() },
        ],
        layer_ext: vec![
            LayerExt { layer: id, tag: *b"ABCD", flags: 0, bytes: vec![9; 10] },
            LayerExt { layer: id + 100, tag: *b"GONE", flags: 0, bytes: vec![1] },
        ],
        title: "Page 1".into(),
    };
    let loaded = read(&write(&doc, &ex, &pool), &pool);
    assert_eq!(loaded.view, ex.view);
    assert_eq!(loaded.extra_sections, ex.sections[..1]);
    assert_eq!(loaded.layer_ext, ex.layer_ext[..1]);
    assert_eq!(loaded.warnings, [LoadWarning::SkippedSection { tag: *b"ZZZ1" }]);
    assert_eq!(loaded.read_only_reason, None, "kept sections are not lossy");
    let meta: Vec<_> = loaded.info.meta.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(meta[0], ("app", concat!("ARTY ", env!("CARGO_PKG_VERSION"))));
    assert_eq!(&meta[1..], [("title", "Page 1"), ("session", "c3".repeat(16).as_str()), ("rev", "0")]);
}

#[test]
fn long_names_are_truncated_at_a_char_boundary() {
    let pool = pool();
    let mut doc = Document::new(64, 64, 300);
    let id = doc.active();
    let mut props = doc.layer(id).unwrap().props.clone();
    props.name = "a".to_owned() + &"é".repeat(3000); // 6001 bytes
    doc.set_props(id, props);
    let mut w = arty_io::FileWriter::create(Vec::new(), 0, UUID).unwrap();
    let stats = w.commit(&doc, &SaveExtras::default(), &meta(&doc), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!(stats.truncated_names, 1);
    let loaded = read(&w.into_sink(), &pool);
    let name = &loaded.doc.layer(id).unwrap().props.name;
    assert_eq!(name.len(), 4095);
    assert!(name.ends_with('é'));
}

#[test]
fn present_but_empty_tiles_and_folders_survive() {
    let pool = pool();
    let mut doc = Document::new(100, 100, 72);
    let f = doc.add_folder();
    let inner = doc.add_raster_layer();
    assert!(doc.move_layer(inner, Some(f), 0));
    doc.set_folder_expanded(f, false);
    let (g, _) = doc.paint_target(inner).unwrap();
    g.insert(TileCoord::new(3, -2), filled([0; 4]));
    let loaded = read(&write(&doc, &SaveExtras::default(), &pool), &pool);
    assert_same_doc(&doc, &loaded.doc);
    let LayerContent::Folder { expanded, .. } = loaded.doc.layer(f).unwrap().content else { panic!() };
    assert!(!expanded);
    assert_eq!(loaded.doc.layer(inner).unwrap().raster().unwrap().len(), 1);
    assert_eq!(loaded.doc.active(), LayerId(inner.0));
}
