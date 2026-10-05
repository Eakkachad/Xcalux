//! Reference layers in `.arty` files: the `REFL` LEXT entry (FILL test 14).

mod common;

use arty_core::{Document, LayerId};
use arty_io::format::{COMMIT_RECORD_LEN, Commit, RECORD_HEADER_LEN, RecordHeader, RecordKind};
use arty_io::limits::MAX_LAYER_COUNT;
use arty_io::manifest::{self, LEXT_REFL};
use arty_io::{LayerExt, Progress, SaveExtras, Session};
use common::{assert_same_doc, opts, pool, read, records, session, temp_dir, write};

/// The LEXT entries of the newest commit, as stored.
fn stored_lext(file: &[u8]) -> Vec<LayerExt> {
    let (pos, ..) = *records(file).iter().rev().find(|(_, kind, _)| *kind == RecordKind::Commit as u8).expect("a commit");
    let pos = pos as usize;
    let c = Commit::decode_record(&file[pos..pos + COMMIT_RECORD_LEN], pos as u64).unwrap();
    let m = c.manifest_offset as usize;
    let h = RecordHeader::decode(&file[m..m + RECORD_HEADER_LEN], m as u64).unwrap();
    let payload = &file[m + RECORD_HEADER_LEN..m + RECORD_HEADER_LEN + h.payload_len as usize];
    let raw = manifest::decode_payload(payload, m as u64).unwrap();
    manifest::parse(&raw, m as u64, MAX_LAYER_COUNT).unwrap().layer_ext
}

fn set_reference(doc: &mut Document, id: LayerId, on: bool) {
    let mut p = doc.layer(id).unwrap().props.clone();
    p.reference = on;
    doc.set_props(id, p);
}

/// Two rasters (one in a folder); `refs` are marked as reference layers.
fn doc_with_refs(refs: &[usize]) -> (Document, Vec<LayerId>) {
    let mut doc = Document::new(128, 128, 350);
    let a = doc.active();
    let folder = doc.add_folder().unwrap();
    let b = doc.add_raster_layer().unwrap();
    doc.move_layer(b, Some(folder), 0);
    let ids = vec![a, folder, b];
    for &i in refs {
        set_reference(&mut doc, ids[i], true);
    }
    (doc, ids)
}

#[test]
fn fl14_reference_flag_round_trips_through_refl() {
    let pool = pool();
    let (doc, ids) = doc_with_refs(&[0, 1]);
    let file = write(&doc, &SaveExtras::default(), &pool);
    let mut ext = stored_lext(&file);
    ext.sort_by_key(|e| e.layer);
    let want: Vec<LayerExt> =
        [ids[0], ids[1]].iter().map(|id| LayerExt { layer: id.0, tag: LEXT_REFL, flags: 0, bytes: Vec::new() }).collect();
    assert_eq!(ext, want, "one empty, flag-0 entry per reference layer");

    let loaded = read(&file, &pool);
    assert!(loaded.layer_ext.is_empty(), "decoded entries are consumed");
    assert!(loaded.warnings.is_empty() && loaded.read_only_reason.is_none());
    assert_same_doc(&doc, &loaded.doc);
    let refs: Vec<bool> = ids.iter().map(|id| loaded.doc.layer(*id).unwrap().props.reference).collect();
    assert_eq!(refs, [true, true, false]);

    // Clearing the flag and saving again writes no REFL.
    let mut doc = loaded.doc;
    set_reference(&mut doc, ids[0], false);
    set_reference(&mut doc, ids[1], false);
    let again = write(&doc, &SaveExtras { layer_ext: loaded.layer_ext.clone(), ..Default::default() }, &pool);
    assert!(stored_lext(&again).iter().all(|e| e.tag != LEXT_REFL));
    assert!(!read(&again, &pool).doc.layer(ids[0]).unwrap().props.reference);
}

#[test]
fn fl14_old_reader_keeps_refl_byte_identical() {
    let pool = pool();
    let (doc, ids) = doc_with_refs(&[2]);
    let first = write(&doc, &SaveExtras::default(), &pool);
    let stored = stored_lext(&first);

    // A v2.0 reader does not decode REFL: the layer looks like a plain one
    // and the entry rides along in `layer_ext`.
    let mut old = read(&first, &pool).doc;
    set_reference(&mut old, ids[2], false);
    let ex = SaveExtras { layer_ext: stored.clone(), ..Default::default() };
    let resaved = write(&old, &ex, &pool);
    assert_eq!(stored_lext(&resaved), stored, "re-emitted byte for byte");
    let back = read(&resaved, &pool);
    assert!(back.doc.layer(ids[2]).unwrap().props.reference, "a new reader sees the flag again");

    // Through a session's save, autosave and compaction too.
    let dir = temp_dir("refl-session");
    let main = dir.join("refl.arty");
    let p = Progress::default();
    let mut s = Session::new(session(), Some(&dir.join("recovery")));
    s.save_main(&old, &ex, &main, false, &opts(), &pool, &p).unwrap();
    assert_eq!(stored_lext(&std::fs::read(&main).unwrap()), stored);
    s.autosave(&old, &ex, 1, &pool, &p).unwrap();
    s.compact(&old, &ex, 1, &pool, &p).unwrap();
    let recovery = std::fs::read(s.recovery_path().unwrap()).unwrap();
    assert_eq!(stored_lext(&recovery), stored);
}

#[test]
fn fl14_refl_on_a_missing_layer_is_dropped() {
    let pool = pool();
    let (doc, ids) = doc_with_refs(&[]);
    let stray = LayerExt { layer: 999, tag: LEXT_REFL, flags: 0, bytes: Vec::new() };
    let file = write(&doc, &SaveExtras { layer_ext: vec![stray.clone()], ..Default::default() }, &pool);
    assert!(stored_lext(&file).is_empty(), "the live-id filter drops it on save");

    // A file that does carry one (written by another tool) still loads; the
    // entry is kept aside and dropped on the next save.
    let mut l = read(&write(&doc, &SaveExtras::default(), &pool), &pool);
    let mut layers: Vec<arty_core::Layer> = ids.iter().map(|id| l.doc.layer(*id).unwrap().clone()).collect();
    let mut ext = vec![stray.clone(), LayerExt { layer: ids[0].0, tag: LEXT_REFL, flags: 0, bytes: vec![7] }];
    arty_io::refl::apply(&mut layers, &mut ext, &mut l.warnings);
    assert_eq!(ext, [stray], "only the entry with no layer stays");
    assert!(layers[0].props.reference, "a body from a later version is ignored");
    assert!(l.warnings.is_empty());
}
