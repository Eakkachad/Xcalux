//! Handcrafted hostile and odd files (plan item 8, handcrafted part):
//! each is refused with the right error, or loaded with the right fix-up
//! and warning.

mod common;

use arty_core::tile::new_tile_box;
use arty_core::{LayerId, TileCoord, TreeError};
use arty_io::codec::{self, BlobCodec, CodecScratch, TileClass};
use arty_io::format::{
    Commit, LAYER_KIND_FOLDER, LAYER_KIND_RASTER, LF_EXPANDED, LF_VISIBLE, LayerRecord, RecordKind, TileCodec, TileEntry,
};
use arty_io::manifest::{DocFields, SEC_CRITICAL};
use arty_io::{IoError, LoadLimits, LoadOptions, LoadWarning};
use common::raw::Raw;
use common::*;

fn doc(layer_count: usize) -> DocFields {
    DocFields { width: 256, height: 256, dpi: 72, paper: None, active: 1, next_id: 100_000, layer_count: layer_count as u32 }
}

fn layer(id: u32, parent_id: u32, kind: u8) -> LayerRecord<'static> {
    LayerRecord {
        id,
        parent_id,
        kind,
        flags: LF_VISIBLE | LF_EXPANDED,
        blend: 0,
        opacity_bits: 1f32.to_bits(),
        tile_count: 0,
        table_offset: 0,
        name: b"layer",
    }
}

fn with_table(rec: LayerRecord<'static>, count: u32, at: u64) -> LayerRecord<'static> {
    LayerRecord { tile_count: count, table_offset: at, ..rec }
}

fn solid(x: i32, y: i32, px: [u16; 4]) -> TileEntry {
    TileEntry::solid(TileCoord::new(x, y), px)
}

fn load(bytes: &[u8]) -> Result<arty_io::Loaded, IoError> {
    read_with(bytes, &LoadOptions::default(), &pool())
}

/// A file with one raster layer holding `entries` (blobs written first by
/// `blobs`).
fn one_layer(blobs: impl FnOnce(&mut Raw) -> Vec<TileEntry>) -> Vec<u8> {
    let mut f = Raw::new();
    let entries = blobs(&mut f);
    let t = f.table(1, &entries);
    f.finish(doc(1), &[with_table(layer(1, 0, LAYER_KIND_RASTER), entries.len() as u32, t)], |_| {})
}

#[test]
fn bad_trees_are_refused() {
    let tree = |layers: &[LayerRecord<'_>]| match load(&Raw::new().finish(doc(layers.len()), layers, |_| {})) {
        Err(IoError::InvalidTree(e)) => e,
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("loaded"),
    };
    let (r, f) = (LAYER_KIND_RASTER, LAYER_KIND_FOLDER);
    assert_eq!(tree(&[layer(2, 1, r), layer(1, 0, f)]), TreeError::Orphan(LayerId(2)), "child before parent");
    assert_eq!(tree(&[layer(1, 1, f), layer(2, 0, r)]), TreeError::Orphan(LayerId(1)), "self-parent");
    assert_eq!(tree(&[layer(1, 0, r), layer(1, 0, r)]), TreeError::DuplicateId(LayerId(1)));
    assert_eq!(tree(&[layer(0, 0, r)]), TreeError::ZeroId);
    assert_eq!(tree(&[layer(1, 0, r), layer(2, 1, r)]), TreeError::ChildOfRaster(LayerId(1)));
    assert_eq!(tree(&[layer(1, 0, 7), layer(2, 1, r)]), TreeError::ChildOfRaster(LayerId(1)), "unknown kind as parent");
    let mut deep: Vec<_> = (1..=1000).map(|i| layer(i, i - 1, f)).collect();
    deep.push(layer(1001, 1000, r));
    assert_eq!(tree(&deep), TreeError::TooDeep, "depth 1000");
    let mut ok: Vec<_> = (1..=63).map(|i| layer(i, i - 1, f)).collect();
    ok.push(layer(64, 63, r));
    assert!(load(&Raw::new().finish(doc(64), &ok, |_| {})).is_ok(), "depth 64 is fine");

    // 70k layers: refused from the DOC count, before reading records.
    let f70k = Raw::new().finish(DocFields { layer_count: 70_000, ..doc(1) }, &[layer(1, 0, r)], |_| {});
    assert!(matches!(load(&f70k), Err(IoError::LimitExceeded { what: "layers", .. })));
    let many: Vec<_> = (1..=10).map(|i| layer(i, 0, r)).collect();
    let o = LoadOptions { limits: LoadLimits { max_layers: 9, ..Default::default() }, ..Default::default() };
    let r10 = read_with(&Raw::new().finish(doc(10), &many, |_| {}), &o, &pool());
    assert!(matches!(r10, Err(IoError::LimitExceeded { what: "layers", .. })));
}

#[test]
fn fixable_problems_load_with_warnings() {
    let (r, f) = (LAYER_KIND_RASTER, LAYER_KIND_FOLDER);
    // NaN, too large and negative opacity; an unknown blend; a bad name.
    let mut nan = layer(1, 0, r);
    nan.opacity_bits = f32::NAN.to_bits();
    let big = LayerRecord { id: 2, opacity_bits: 2f32.to_bits(), ..layer(2, 0, r) };
    let neg = LayerRecord { id: 3, opacity_bits: (-0.5f32).to_bits(), blend: 99, name: b"bad \xFF name", ..layer(3, 0, r) };
    let doc_ = DocFields { active: 77, next_id: 2, paper: Some([0xFFFF, 0, 0x8000, 0x8001]), ..doc(3) };
    let l = load(&Raw::new().finish(doc_, &[nan, big, neg], |_| {})).unwrap();
    let props = |id| l.doc.layer(LayerId(id)).unwrap().props.clone();
    assert_eq!((props(1).opacity, props(2).opacity, props(3).opacity), (1.0, 1.0, 0.0));
    assert_eq!(props(3).blend, arty_core::BlendMode::Normal);
    assert_eq!(props(3).name, "bad \u{FFFD} name");
    assert_eq!(l.doc.paper(), Some([0x8000, 0, 0x8000, 0x8000]));
    assert_eq!(l.doc.active(), LayerId(3), "topmost raster");
    assert_eq!(l.doc.next_layer_id(), 4);
    for w in [
        LoadWarning::OpacityFixed { layer: 1 },
        LoadWarning::OpacityFixed { layer: 2 },
        LoadWarning::OpacityFixed { layer: 3 },
        LoadWarning::UnknownBlend { layer: 3, id: 99 },
        LoadWarning::LossyName { layer: 3 },
        LoadWarning::FixedActiveLayer,
        LoadWarning::FixedNextId,
        LoadWarning::ClampedPixels { count: 2 },
    ] {
        assert!(l.warnings.contains(&w), "{w:?} in {:?}", l.warnings);
    }
    assert!(l.read_only_reason.is_some(), "an unknown blend is lossy");

    // No raster at all: one is added on top and made active.
    let l = load(&Raw::new().finish(DocFields { next_id: 5, ..doc(1) }, &[layer(1, 0, f)], |_| {})).unwrap();
    assert!(l.warnings.contains(&LoadWarning::AddedMissingRaster));
    assert_eq!(l.doc.root(), [LayerId(1), LayerId(5)]);
    assert_eq!(l.doc.active(), LayerId(1), "a folder may stay active");
    assert_eq!(l.doc.next_layer_id(), 6);
    assert!(l.doc.layer(LayerId(5)).unwrap().raster().is_some());
}

#[test]
fn next_id_leaves_room_for_fresh_ids() {
    let r = LAYER_KIND_RASTER;
    let file = |next_id, top| {
        Raw::new().finish(DocFields { next_id, ..doc(2) }, &[layer(1, 0, r), layer(top, 0, r)], |_| {})
    };
    // next_id near u32::MAX is lowered to just above the largest id.
    let mut l = load(&file(u32::MAX, 2)).unwrap();
    assert!(l.warnings.contains(&LoadWarning::FixedNextId));
    assert_eq!(l.doc.next_layer_id(), 3);
    assert_eq!(l.doc.add_raster_layer(), Some(LayerId(3)));

    // A layer id that leaves no room is refused, whatever next_id says.
    let high = arty_core::MAX_NEXT_ID;
    for next_id in [2, u32::MAX] {
        let refused = matches!(load(&file(next_id, high)), Err(IoError::InvalidTree(TreeError::BadNextId)));
        assert!(refused, "next_id {next_id}");
    }
    // The largest id allowed still loads; no fresh id is left.
    let mut l = load(&file(high, high - 1)).unwrap();
    assert_eq!(l.doc.next_layer_id(), high);
    assert_eq!(l.doc.add_raster_layer(), None);
}

#[test]
fn pixels_are_sanitized_with_a_warning() {
    let mut t = new_tile_box();
    t[3][5] = [0x10, 0x20, 0x30, 0xFFFF];
    let raw = bytemuck::bytes_of(&*t).to_vec();
    let crc = crc32fast::hash(&raw);
    let file = one_layer(|f| {
        let at = f.blob(&raw);
        let e = TileEntry { coord: TileCoord::new(0, 0), codec: TileCodec::Raw, stored_len: 32768, raw_crc: crc, stored_crc: crc, offset: at };
        vec![e, solid(1, 0, [0x100, 0x100, 0x100, 0x9000])]
    });
    let l = load(&file).unwrap();
    let g = l.doc.active_layer().raster().unwrap();
    assert_eq!(g.get(TileCoord::new(0, 0)).unwrap()[3][5], [0x10, 0x20, 0x30, 0x8000]);
    assert_eq!(g.get(TileCoord::new(1, 0)).unwrap()[63][63], [0x100, 0x100, 0x100, 0x8000]);
    assert_eq!(l.warnings, [LoadWarning::ClampedPixels { count: 1 + 4096 }]);
    assert_eq!(l.read_only_reason, None);
}

/// An LZ4_SHUF blob of a gradient tile and its entry at `(0, 0)`.
fn lz4_blob(f: &mut Raw) -> TileEntry {
    let mut rng = Rng(3);
    let t = tile(&mut rng, 3);
    let TileClass::General { raw_crc } = codec::classify(&t) else { panic!() };
    let mut s = CodecScratch::new();
    let enc = codec::encode_tile(&t, raw_crc, BlobCodec::Lz4Shuf, &mut s);
    assert_eq!(enc.codec, TileCodec::Lz4Shuf);
    let (bytes, stored_crc) = (enc.bytes.to_vec(), enc.stored_crc);
    let offset = f.blob(&bytes);
    TileEntry { coord: TileCoord::new(0, 0), codec: TileCodec::Lz4Shuf, stored_len: bytes.len() as u32, raw_crc, stored_crc, offset }
}

#[test]
fn bad_tile_entries_are_refused() {
    let corrupt = |file: Vec<u8>, what: &str| match load(&file) {
        Err(IoError::Corrupt { what: w, .. }) => assert_eq!(w, what),
        Err(e) => panic!("{what}: {e:?}"),
        Ok(_) => panic!("{what}: loaded"),
    };
    corrupt(one_layer(|_| vec![solid(i32::MIN, 0, [0; 4])]), "tile coordinate out of range");
    corrupt(one_layer(|_| vec![TileEntry { stored_len: 1, ..solid(0, 0, [0; 4]) }]), "tile stored length");
    corrupt(one_layer(|f| vec![TileEntry { stored_len: 32768, ..lz4_blob(f) }]), "tile stored length");
    corrupt(one_layer(|f| vec![TileEntry { stored_crc: 1, ..lz4_blob(f) }]), "tile stored crc");
    corrupt(one_layer(|f| vec![TileEntry { raw_crc: 1, ..lz4_blob(f) }]), "tile crc");
    // The same offset with different CRCs.
    corrupt(
        one_layer(|f| {
            let e = lz4_blob(f);
            vec![e, TileEntry { coord: TileCoord::new(1, 0), raw_crc: e.raw_crc ^ 1, ..e }]
        }),
        "shared tile blob disagrees",
    );

    // A blob inside another layer's table: its CRC does not match.
    let mut f = Raw::new();
    let t1 = f.table(1, &[solid(0, 0, [1; 4]), solid(1, 0, [2; 4])]);
    let fake = TileEntry {
        coord: TileCoord::new(0, 0),
        codec: TileCodec::Lz4Shuf,
        stored_len: 40,
        raw_crc: 5,
        stored_crc: 6,
        offset: t1 + 24,
    };
    let t2 = f.table(2, &[fake]);
    let layers = [with_table(layer(1, 0, 0), 2, t1), with_table(layer(2, 0, 0), 1, t2)];
    corrupt(f.finish(doc(2), &layers, |_| {}), "tile stored crc");
    // A blob after its table (in the manifest) is out of range.
    let mut f = Raw::new();
    let len = f.0.len() as u64;
    let t = f.table(1, &[TileEntry { offset: len + 24 + 16 + 32, ..fake }]);
    corrupt(f.finish(doc(1), &[with_table(layer(1, 0, 0), 1, t)], |_| {}), "tile blob range");
    // A table shared by two layers, and a table count that differs.
    let mut f = Raw::new();
    let t = f.table(1, &[solid(0, 0, [1; 4])]);
    let shared = [with_table(layer(1, 0, 0), 1, t), with_table(layer(2, 0, 0), 1, t)];
    corrupt(f.finish(doc(2), &shared, |_| {}), "layer table offset");
    let mut f = Raw::new();
    let t = f.table(1, &[solid(0, 0, [1; 4])]);
    corrupt(f.finish(doc(1), &[with_table(layer(1, 0, 0), 2, t)], |_| {}), "tile table does not match its layer");
    // A folder with tiles.
    let mut f = Raw::new();
    let t = f.table(1, &[solid(0, 0, [1; 4])]);
    corrupt(f.finish(doc(2), &[with_table(layer(1, 0, 1), 1, t), layer(2, 0, 0)], |_| {}), "folder with tiles");
}

#[test]
fn bombs_hit_limits_before_allocating() {
    // Manifest claiming 16 MiB from a 10-byte lz4 body.
    let mut f = Raw::new();
    let mut p = 1u32.to_le_bytes().to_vec();
    p.extend_from_slice(&(16u32 << 20).to_le_bytes());
    p.extend_from_slice(&[0; 10]);
    let m = f.record(RecordKind::Manifest, &p);
    f.0.extend_from_slice(&Commit { manifest_offset: m, prev_commit_offset: 0, commit_seq: 1, unix_ms: 0 }.encode_record());
    assert!(matches!(load(&f.0), Err(IoError::Corrupt { what: "manifest length", .. })));

    // A table of 1M entries with a tiny lz4 payload.
    let mut f = Raw::new();
    let mut p = Vec::new();
    p.extend_from_slice(&1u32.to_le_bytes());
    p.extend_from_slice(&(1u32 << 20).to_le_bytes());
    p.extend_from_slice(&32u16.to_le_bytes());
    p.extend_from_slice(&[1, 0]);
    p.extend_from_slice(&(32u32 << 20).to_le_bytes());
    p.extend_from_slice(&[0; 10]);
    let t = f.record(RecordKind::TileTable, &p);
    let file = f.finish(doc(1), &[with_table(layer(1, 0, 0), 1 << 20, t)], |_| {});
    assert!(matches!(load(&file), Err(IoError::Corrupt { what: "tile table length", .. })));

    // 1M distinct SOLID values: 32 GiB decoded, over the 16 GiB default.
    let entries: Vec<TileEntry> =
        (0..1i32 << 20).map(|i| solid(i & 1023, i >> 10, [(i & 0x7FFF) as u16, (i >> 15) as u16, 0, 0x8000])).collect();
    let mut f = Raw::new();
    let t = f.record(RecordKind::TileTable, &arty_io::table::encode(1, &entries));
    let file = f.finish(doc(1), &[with_table(layer(1, 0, 0), 1 << 20, t)], |_| {});
    assert!(matches!(load(&file), Err(IoError::LimitExceeded { what: "decoded pixels", .. })));

    // Many tiny overlapping blobs: 4096 tiles would decode to 128 MiB.
    let mut f = Raw::new();
    let seg = f.blob(&[0x55; 4096]);
    let entries: Vec<TileEntry> = (0..4096)
        .map(|i| TileEntry {
            coord: TileCoord::new(i, 0),
            codec: TileCodec::Lz4Shuf,
            stored_len: 1,
            raw_crc: 0,
            stored_crc: 0,
            offset: seg + i as u64,
        })
        .collect();
    let t = f.table(1, &entries);
    let file = f.finish(doc(1), &[with_table(layer(1, 0, 0), 4096, t)], |_| {});
    let small = LoadOptions { limits: LoadLimits { max_decoded_bytes: 8 << 20, ..Default::default() }, ..Default::default() };
    assert!(matches!(read_with(&file, &small, &pool()), Err(IoError::LimitExceeded { what: "decoded pixels", .. })));
    let few = LoadOptions { limits: LoadLimits { max_entries: 4095, ..Default::default() }, ..Default::default() };
    assert!(matches!(read_with(&file, &few, &pool()), Err(IoError::LimitExceeded { what: "tile entries", .. })));
}

#[test]
fn unknown_features_and_versions() {
    let r = LAYER_KIND_RASTER;
    let crit = Raw::new().finish(doc(1), &[layer(1, 0, r)], |w| w.push(*b"VECT", SEC_CRITICAL, b"strokes"));
    assert!(matches!(load(&crit), Err(IoError::UnsupportedFeature { tag }) if tag == *b"VECT"));

    // An unknown layer kind with tiles loads as a locked raster, lossy.
    let mut f = Raw::new();
    let t = f.table(2, &[solid(0, 0, [0, 0, 0, 0x8000])]);
    let file = f.finish(doc(2), &[layer(1, 0, r), with_table(layer(2, 0, 9), 1, t)], |_| {});
    let l = load(&file).unwrap();
    let odd = l.doc.layer(LayerId(2)).unwrap();
    assert!(odd.props.locked && odd.raster().unwrap().len() == 1);
    assert_eq!(l.warnings, [LoadWarning::UnsupportedLayerKind { layer: 2, kind: 9 }]);
    assert!(l.read_only_reason.is_some());

    let mut v3 = Raw::new().finish(doc(1), &[layer(1, 0, r)], |_| {});
    v3[4] = 3;
    assert!(matches!(load(&v3), Err(IoError::NewerFormat { major: 3 })));
    let mut v9 = Raw::new().finish(doc(1), &[layer(1, 0, r)], |_| {});
    v9[8] = 9;
    let crc = crc32fast::hash(&v9[..60]);
    v9[60..64].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(load(&v9).unwrap().info.kind, arty_io::FileKind::V2 { minor: 9 });
    assert!(matches!(load(b"not an arty file at all"), Err(IoError::NotArty)));
    assert!(matches!(load(b""), Err(IoError::NotArty)));
}
