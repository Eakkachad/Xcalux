//! M3 post-merge integration (spec §10.2, tests 4 and 5): a document with
//! a selection, a page setup, a reference layer and three frame folders
//! round-trips every one of them, and a v2.0 reader keeps all four kinds
//! of data byte for byte.

mod common;

use arty_core::frame::add_frame_folder;
use arty_core::raster::rasterize_polygon;
use arty_core::selection::full_mask;
use arty_core::{BorderStyle, Document, FrameShape, LayerId, PageSetup, Panel, RectF, TileCoord};
use arty_io::format::{Commit, RecordHeader, RecordKind};
use arty_io::manifest::{
    self, LEXT_FRAM, LEXT_REFL, SectionWriter, TAG_LEXT, TAG_PSET, TAG_SELM, decode_payload, encode_payload, lext_body,
    SEC_SAFE_TO_COPY,
};
use arty_io::{LayerExt, LoadWarning, Progress, SaveExtras, Session};
use common::*;

const W: u32 = 300;
const H: u32 = 420;

fn panel(x: f32, y: f32, w: f32, h: f32) -> Panel {
    Panel::rect(RectF { x, y, w, h }).unwrap()
}

/// Painted rasters, a reference layer, three frame folders (one with a
/// bleed panel), a page setup and a selection with full and soft tiles.
fn m3_doc() -> (Document, LayerId) {
    let mut doc = Document::new(W, H, 350);
    let base = doc.active();
    doc.paint_target(base).unwrap().0.insert(TileCoord::new(1, 1), filled([ONE / 4, 0, 0, ONE / 2]));
    let refl = doc.add_raster_layer().unwrap();
    doc.paint_target(refl).unwrap().0.insert(TileCoord::new(0, 2), filled([0, 0, 0, ONE]));
    let mut p = doc.layer(refl).unwrap().props.clone();
    p.reference = true;
    doc.set_props(refl, p);
    let border = BorderStyle { width: 4.5, color: [0, 0, 0, ONE] };
    for (i, panels) in [
        vec![panel(-6.0, -6.0, 160.0, 140.0), panel(160.0, 10.0, 130.0, 120.0)],
        vec![Panel::new(vec![[20.0, 150.0], [280.0, 140.0], [285.0, 270.0], [15.0, 280.5]]).unwrap()],
        vec![panel(10.0, 290.0, 280.0, 120.0)],
    ]
    .into_iter()
    .enumerate()
    {
        let folder = add_frame_folder(
            &mut doc,
            FrameShape { panels, border: BorderStyle { width: border.width + i as f32, ..border } },
        )
        .unwrap();
        let art = doc.active();
        doc.paint_target(art).unwrap().0.insert(TileCoord::new(i as i32, 0), filled([0, ONE / 3, 0, ONE / 2]));
        // The next one goes above this folder, not inside it.
        doc.set_active(folder);
    }
    doc.set_page_unrecorded(Some(PageSetup {
        trim: RectF { x: 15.0, y: 20.0, w: 270.0, h: 380.0 },
        bleed: 10.0,
        safe: 12.0,
        inner: RectF { x: 30.0, y: 40.0, w: 240.0, h: 340.0 },
        unit: 0,
    }));
    let mut sel = rasterize_polygon(&[[20.0, 30.0], [250.0, 60.0], [200.0, 380.5], [33.3, 300.0]], W, H, true);
    sel.insert_tile(TileCoord::new(4, 6), full_mask().clone());
    doc.set_selection_unrecorded(sel);
    (doc, refl)
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

fn section(file: &[u8], tag: [u8; 4]) -> Option<(u32, Vec<u8>)> {
    sections(file).into_iter().find(|s| s.0 == tag).map(|s| (s.1, s.2))
}

/// The LEXT entries of the newest manifest, sorted.
fn lext(file: &[u8]) -> Vec<LayerExt> {
    let mut w = SectionWriter::default();
    for (tag, flags, body) in sections(file) {
        w.push(tag, flags, &body);
    }
    let mut v = manifest::parse(&w.into_raw(), 0, 1 << 16).unwrap().layer_ext;
    v.sort_by_key(|e| (e.layer, e.tag));
    v
}

/// Append a commit whose manifest is `file`'s newest one with every tag
/// (sections and LEXT entries) passed through `rename`.
fn recommit_renamed(file: &[u8], rename: impl Fn([u8; 4]) -> [u8; 4]) -> Vec<u8> {
    let entries: Vec<LayerExt> = lext(file).into_iter().map(|e| LayerExt { tag: rename(e.tag), ..e }).collect();
    let mut w = SectionWriter::default();
    for (tag, flags, body) in sections(file) {
        if tag == TAG_LEXT {
            w.push(tag, flags, &lext_body(entries.iter()));
        } else {
            w.push(rename(tag), flags, &body);
        }
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

/// Tags a v2.0 build does not know, standing in for SELM, PSET, FRAM and
/// REFL (its decoders do not exist).
const OLD: [([u8; 4], [u8; 4]); 4] =
    [(TAG_SELM, *b"SELm"), (TAG_PSET, *b"PSEt"), (LEXT_FRAM, *b"FRAm"), (LEXT_REFL, *b"REFl")];

fn to_old(t: [u8; 4]) -> [u8; 4] {
    OLD.iter().find(|(n, _)| *n == t).map_or(t, |(_, o)| *o)
}

fn to_new(t: [u8; 4]) -> [u8; 4] {
    OLD.iter().find(|(_, o)| *o == t).map_or(t, |(n, _)| *n)
}

#[test]
fn m3_all_page_tool_data_round_trips() {
    let pool = pool();
    let (doc, refl) = m3_doc();
    let file = write(&doc, &SaveExtras::default(), &pool);
    for tag in [TAG_SELM, TAG_PSET] {
        assert_eq!(section(&file, tag).unwrap().0, SEC_SAFE_TO_COPY, "{tag:?}");
    }
    let entries = lext(&file);
    assert_eq!(entries.iter().filter(|e| e.tag == LEXT_FRAM).count(), 3, "one FRAM per frame folder");
    assert_eq!(entries.iter().filter(|e| e.tag == LEXT_REFL).map(|e| e.layer).collect::<Vec<_>>(), [refl.0]);
    assert!(entries.iter().all(|e| e.flags == 0), "no entry is CRITICAL");

    let l = read(&file, &pool);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.read_only_reason, None, "not lossy");
    assert!(l.layer_ext.is_empty() && l.extra_sections.is_empty(), "everything is decoded");
    assert_same_doc(&doc, &l.doc);
    assert!(l.doc.layer(refl).unwrap().props.reference);

    // Through a session's save, autosave and compaction.
    let dir = temp_dir("m3-integration");
    let main = dir.join("m3.arty");
    let p = Progress::default();
    let mut s = Session::new(session(), Some(&dir.join("recovery")));
    let ex = SaveExtras::default();
    s.save_main(&doc, &ex, &main, false, &opts(), &pool, &p).unwrap();
    assert_same_doc(&doc, &read(&std::fs::read(&main).unwrap(), &pool).doc);
    s.autosave(&doc, &ex, 1, &pool, &p).unwrap();
    s.compact(&doc, &ex, 1, &pool, &p).unwrap();
    let recovery = read(&std::fs::read(s.recovery_path().unwrap()).unwrap(), &pool);
    assert!(recovery.warnings.is_empty(), "{:?}", recovery.warnings);
    assert_same_doc(&doc, &recovery.doc);
    s.close(true).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn m3_older_readers_keep_all_page_tool_data() {
    let pool = pool();
    let (doc, refl) = m3_doc();
    let file = write(&doc, &SaveExtras::default(), &pool);
    let (selm, pset) = (section(&file, TAG_SELM).unwrap(), section(&file, TAG_PSET).unwrap());
    let entries = lext(&file);

    // What a v2.0 build sees: none of the four is decoded.
    let as_old = recommit_renamed(&file, to_old);
    let old = read(&as_old, &pool);
    assert!(!old.doc.has_selection() && old.doc.page_setup().is_none());
    assert!(!old.doc.layer(refl).unwrap().props.reference);
    assert!(layer_ids(&old.doc).iter().all(|id| old.doc.frame(*id).is_none()), "frame folders read as plain folders");
    assert_eq!(old.read_only_reason, None, "the file stays editable");
    let skipped: Vec<_> = old.warnings.iter().filter(|w| matches!(w, LoadWarning::SkippedSection { .. })).collect();
    assert_eq!(skipped.len(), 2, "{:?}", old.warnings);

    // It saves what it kept as it read it.
    let ex = SaveExtras { sections: old.extra_sections.clone(), layer_ext: old.layer_ext.clone(), ..Default::default() };
    let resaved = write(&old.doc, &ex, &pool);
    assert_eq!(section(&resaved, to_old(TAG_SELM)), Some(selm), "SELM byte-identical");
    assert_eq!(section(&resaved, to_old(TAG_PSET)), Some(pset), "PSET byte-identical");
    let back: Vec<LayerExt> = lext(&resaved).into_iter().map(|e| LayerExt { tag: to_new(e.tag), ..e }).collect();
    assert_eq!(back, entries, "FRAM and REFL byte-identical");

    // This build reads all of it again.
    let l = read(&recommit_renamed(&resaved, to_new), &pool);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_same_doc(&doc, &l.doc);
}
