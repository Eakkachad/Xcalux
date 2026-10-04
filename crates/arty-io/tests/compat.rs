//! R6: files from newer 2.x versions load, and data this version does not
//! understand is either kept byte-exact on every save path or makes the
//! load read-only.

mod common;

use arty_core::{Document, LayerId, TileCoord};
use arty_io::codec::{self, BlobCodec, CodecScratch, TileClass};
use arty_io::format::{LAYER_KIND_RASTER, LF_VISIBLE, LayerRecord, TileEntry};
use arty_io::manifest::{
    DocFields, LEXT_CRITICAL, SEC_CRITICAL, SEC_SAFE_TO_COPY, SectionWriter, TAG_DOC, TAG_LAYR, TAG_LEXT, TAG_PAGE,
    lext_body,
};
use arty_io::{AppSection, FileKind, LayerExt, LoadOptions, LoadWarning, Progress, SaveExtras, Session};
use common::raw::Raw;
use common::*;

fn doc_fields(layer_count: u32) -> DocFields {
    DocFields { width: 200, height: 100, dpi: 300, paper: None, active: 1, next_id: 2, layer_count }
}

fn raster(tile_count: u32, table_offset: u64) -> LayerRecord<'static> {
    LayerRecord {
        id: 1,
        parent_id: 0,
        kind: LAYER_KIND_RASTER,
        flags: LF_VISIBLE,
        blend: 0,
        opacity_bits: 0.75f32.to_bits(),
        tile_count,
        table_offset,
        name: b"Ink",
    }
}

fn layr(records: &[Vec<u8>]) -> Vec<u8> {
    let mut b = (records.len() as u32).to_le_bytes().to_vec();
    for r in records {
        b.extend_from_slice(r);
    }
    b
}

fn record(r: &LayerRecord<'_>) -> Vec<u8> {
    let mut b = Vec::new();
    r.encode_into(&mut b);
    b
}

fn load(bytes: &[u8]) -> arty_io::Loaded {
    read_with(bytes, &LoadOptions::default(), &pool()).unwrap()
}

/// Set the header's minor version and fix its CRC.
fn set_minor(f: &mut [u8], minor: u32) {
    f[8..12].copy_from_slice(&minor.to_le_bytes());
    let crc = crc32fast::hash(&f[..60]);
    f[60..64].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn newer_minor_versions_with_longer_structs_load() {
    let mut rng = Rng(11);
    let t = tile(&mut rng, 3);
    let TileClass::General { raw_crc } = codec::classify(&t) else { unreachable!() };
    let mut s = CodecScratch::new();
    let enc = codec::encode_tile(&t, raw_crc, BlobCodec::Lz4Shuf, &mut s);
    let mut f = Raw::new();
    let offset = f.blob(enc.bytes);
    let blob = TileEntry {
        coord: TileCoord::new(0, 0),
        codec: enc.codec,
        stored_len: enc.bytes.len() as u32,
        raw_crc,
        stored_crc: enc.stored_crc,
        offset,
    };
    let solid = TileEntry::solid(TileCoord::new(1, 0), [1, 2, 3, 4]);
    // 48-byte entries: a later minor's extra fields are ignored.
    let table = f.table_sized(1, &[blob, solid], 48);

    // A DOC section and a layer record longer than 2.0's.
    let mut doc = doc_fields(1).encode().to_vec();
    doc.extend_from_slice(&[0xEE; 8]);
    let mut rec = record(&raster(2, table));
    let rec_len = u16::from_le_bytes([rec[0], rec[1]]) + 6;
    rec[..2].copy_from_slice(&rec_len.to_le_bytes());
    rec.extend_from_slice(&[0xAB; 6]);
    let mut w = SectionWriter::default();
    w.push(TAG_DOC, SEC_CRITICAL, &doc);
    w.push(TAG_LAYR, SEC_CRITICAL, &layr(&[rec]));
    let mut file = f.manifest_raw(&w.into_raw());
    set_minor(&mut file, 9);

    let l = load(&file);
    assert_eq!(l.info.kind, FileKind::V2 { minor: 9 });
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.read_only_reason, None);
    assert_eq!((l.doc.width(), l.doc.height(), l.doc.dpi()), (200, 100, 300));
    let layer = l.doc.layer(LayerId(1)).unwrap();
    assert_eq!((layer.props.name.as_str(), layer.props.opacity), ("Ink", 0.75));
    let g = layer.raster().unwrap();
    assert!(*g.get(TileCoord::new(0, 0)).unwrap() == *t);
    assert_eq!(g.get(TileCoord::new(1, 0)).unwrap()[9][9], [1, 2, 3, 4]);
}

#[test]
fn data_this_version_cannot_keep_makes_the_load_read_only() {
    let base = |extra: &dyn Fn(&mut SectionWriter)| Raw::new().finish(doc_fields(1), &[raster(0, 0)], extra);

    // Unknown, not SAFE_TO_COPY: skipped, and saving would lose it.
    let l = load(&base(&|w| w.push(*b"GUID", 0, b"guides")));
    assert_eq!(l.warnings, [LoadWarning::SkippedSection { tag: *b"GUID" }]);
    assert!(l.extra_sections.is_empty());
    assert!(l.read_only_reason.is_some());

    // Unknown but SAFE_TO_COPY: kept, so not read-only.
    let l = load(&base(&|w| w.push(*b"SWAT", SEC_SAFE_TO_COPY, b"palette")));
    assert_eq!(l.extra_sections, [AppSection { tag: *b"SWAT", flags: SEC_SAFE_TO_COPY, bytes: b"palette".to_vec() }]);
    assert_eq!(l.read_only_reason, None);

    // A LEXT entry another version must understand.
    let ext = [LayerExt { layer: 1, tag: *b"TEXT", flags: LEXT_CRITICAL, bytes: b"hello".to_vec() }];
    let l = load(&base(&|w| w.push(TAG_LEXT, SEC_SAFE_TO_COPY, &lext_body(ext.iter()))));
    assert_eq!(l.layer_ext, ext);
    assert!(l.read_only_reason.is_some());

    // A second page: page 1 opens, read-only.
    let page = |w: &mut SectionWriter| {
        let mut p = SectionWriter::default();
        p.push(TAG_DOC, SEC_CRITICAL, &doc_fields(0).encode());
        p.push(TAG_LAYR, SEC_CRITICAL, &layr(&[]));
        w.push(TAG_PAGE, 0, &p.into_raw());
    };
    let l = load(&base(&page));
    assert_eq!(l.warnings, [LoadWarning::ExtraPagesIgnored(1)]);
    assert_eq!(l.doc.layer_count(), 1);
    assert!(l.read_only_reason.is_some());
}

fn assert_kept(path: &std::path::Path, ex: &SaveExtras, what: &str) {
    let o = LoadOptions { fallback_to_previous: true, ..Default::default() };
    let l = arty_io::load(path, &o, &pool(), &Progress::default()).unwrap();
    assert_eq!(l.extra_sections, ex.sections, "{what}: sections");
    assert_eq!(l.layer_ext, ex.layer_ext, "{what}: LEXT");
    assert_eq!(l.view, ex.view, "{what}: VIEW");
    assert_eq!(l.read_only_reason, None, "{what}");
}

#[test]
fn kept_sections_survive_autosave_save_and_compaction() {
    let pool = pool();
    let p = Progress::default();
    let dir = temp_dir("compat-kept");
    let mut rng = Rng(5);
    let mut doc = Document::new(256, 256, 350);
    let id = doc.active();
    let (g, _) = doc.paint_target(id).unwrap();
    g.insert(TileCoord::new(0, 0), tile(&mut rng, 4));
    // Bytes no version of ARTY writes, including every byte value.
    let odd: Vec<u8> = (0..=255u8).chain([0, 0, 0xFF]).collect();
    let ex = SaveExtras {
        view: Some(vec![1, 0, 0, 0x80, 0x3F, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        sections: vec![
            AppSection { tag: *b"SWAT", flags: SEC_SAFE_TO_COPY, bytes: odd.clone() },
            AppSection { tag: *b"GUI\0", flags: SEC_SAFE_TO_COPY | 0x100, bytes: Vec::new() },
        ],
        layer_ext: vec![
            LayerExt { layer: id.0, tag: *b"FUT1", flags: 0x80, bytes: odd },
            LayerExt { layer: id.0, tag: *b"FUT2", flags: 0, bytes: Vec::new() },
        ],
        title: "Kept".into(),
    };
    let main = dir.join("kept.arty");
    Session::new(session(), None).save_main(&doc, &ex, &main, false, &opts(), &pool, &p).unwrap();
    assert_kept(&main, &ex, "first save");

    // A later session opens it and saves through every path, passing on
    // what the load returned.
    let mut loaded = arty_io::load(&main, &LoadOptions::default(), &pool, &p).unwrap();
    let mut s = Session::new(arty_io::SessionId([0x77; 16]), Some(&dir.join("recovery")));
    s.adopt(&mut loaded, Some(main.clone()));
    let again = SaveExtras {
        view: loaded.view.clone(),
        sections: loaded.extra_sections.clone(),
        layer_ext: loaded.layer_ext.clone(),
        title: "Kept".into(),
    };
    let mut doc = loaded.doc;
    let recovery = s.recovery_path().unwrap();
    s.autosave(&doc, &again, 1, &pool, &p).unwrap();
    assert_kept(&recovery, &ex, "autosave (rewrite)");
    let (g, _) = doc.paint_target(id).unwrap();
    g.insert(TileCoord::new(1, 1), tile(&mut rng, 3));
    s.autosave(&doc, &again, 2, &pool, &p).unwrap();
    assert_kept(&recovery, &ex, "autosave (append)");
    s.compact(&doc, &again, 2, &pool, &p).unwrap();
    assert_kept(&recovery, &ex, "compaction");
    s.save_main(&doc, &again, &main, false, &opts(), &pool, &p).unwrap();
    assert_kept(&main, &ex, "save");
    assert_kept(&recovery, &ex, "clean recovery commit");
    s.close(true).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}
