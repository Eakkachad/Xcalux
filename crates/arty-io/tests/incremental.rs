//! Incremental saves (plan items 4 and 7): appends write only what
//! changed, unchanged blobs are reused or copied instead of encoded, a
//! damaged source blob is re-encoded, and the recovery file stays bounded.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use arty_core::{Document, LayerId, TileCoord, TileRef};
use arty_io::format::RecordKind;
use arty_io::{CommitMeta, Compaction, FileWriter, LoadOptions, Progress, SaveExtras, Session};
use common::*;

/// Two raster layers of mixed tiles; the bottom one is `LayerId(1)`.
fn doc_a() -> Document {
    let mut rng = Rng(0xA5);
    let mut doc = Document::new(640, 640, 350);
    let bottom = doc.active();
    let top = doc.add_raster_layer();
    for (id, kinds) in [(bottom, [2, 3, 4, 5, 6, 1, 0, 2]), (top, [6, 3, 2, 3, 2, 1, 5, 6])] {
        let (g, _) = doc.paint_target(id).unwrap();
        for (x, kind) in kinds.into_iter().enumerate() {
            g.insert(TileCoord::new(x as i32, id.0 as i32), tile(&mut rng, kind));
        }
    }
    doc
}

/// A snapshot of `doc` with new noise tiles at `xs` of the bottom layer.
fn edit(doc: &Document, seed: u64, xs: &[i32]) -> Document {
    let mut rng = Rng(seed);
    let mut doc = doc.snapshot();
    let (g, _) = doc.paint_target(LayerId(1)).unwrap();
    for &x in xs {
        g.insert(TileCoord::new(x, 1), tile(&mut rng, 2));
    }
    doc
}

/// Fixed revision, so documents that differ only in it give equal
/// manifests.
fn meta0() -> CommitMeta<'static> {
    CommitMeta { session: session(), rev: 0, src: None, clean: false }
}

fn kinds_from(file: &[u8], from: u64) -> Vec<u8> {
    records(file).into_iter().filter(|r| r.0 >= from).map(|r| r.1).collect()
}

fn meta_of(path: &Path) -> Vec<(String, String)> {
    arty_io::read_info(path).unwrap().meta
}

fn meta_value<'a>(meta: &'a [(String, String)], key: &str) -> Option<&'a str> {
    meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn load(path: &Path) -> arty_io::Loaded {
    arty_io::load(path, &LoadOptions::default(), &pool(), &Progress::default()).unwrap()
}

#[test]
fn appends_write_only_what_changed() {
    let pool = pool();
    let (a, ex) = (doc_a(), SaveExtras::default());
    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    let first = w.commit(&a, &ex, &meta(&a), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!(first.tables_written, 2);
    let end_a = w.sink().len() as u64;

    let b = edit(&a, 1, &[0, 3, 9]);
    let s = w.commit(&b, &ex, &meta(&b), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!((s.classified, s.encoded, s.copied, s.healed, s.tables_written), (3, 3, 0, 0, 1));
    let bytes = w.into_sink();
    assert_eq!(s.bytes_written, bytes.len() as u64 - end_a);
    // 3 blobs (noise is stored raw), one table, the manifest, the commit.
    let added: Vec<_> = records(&bytes).into_iter().filter(|r| r.0 >= end_a).collect();
    let kinds: Vec<u8> = added.iter().map(|r| r.1).collect();
    let want = [RecordKind::Segment, RecordKind::TileTable, RecordKind::Manifest, RecordKind::Commit];
    assert_eq!(kinds, want.map(|k| k as u8));
    assert_eq!(added[0].2 - added[0].0, 24 + 3 * 32768);
    assert_eq!(added[3].2 - added[3].0, 56);

    let loaded = read(&bytes, &pool);
    assert_eq!(loaded.info.commit_seq, 2);
    assert_same_doc(&b, &loaded.doc);
    let previous = read(&bytes[..end_a as usize], &pool);
    assert_eq!((previous.info.commit_seq, previous.info.recovered), (1, false));
    assert_same_doc(&a, &previous.doc);
}

#[test]
fn duplicating_a_layer_adds_no_blobs() {
    let pool = pool();
    let (a, ex) = (doc_a(), SaveExtras::default());
    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    w.commit(&a, &ex, &meta(&a), &opts(), &pool, &Progress::default()).unwrap();
    let mut b = a.snapshot();
    let copy = b.duplicate_layer(LayerId(1)).unwrap();
    let s = w.commit(&b, &ex, &meta(&b), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!((s.classified, s.encoded, s.copied, s.tables_written), (0, 0, 0, 1));
    let general = |t: &TileRef| matches!(arty_io::codec::classify(t), arty_io::TileClass::General { .. });
    let blobs = b.layer(copy).unwrap().raster().unwrap().iter().filter(|(_, t)| general(t)).count();
    assert_eq!(s.reused as usize, blobs);
    let bytes = w.into_sink();
    let loaded = read(&bytes, &pool);
    assert_same_doc(&b, &loaded.doc);
}

#[test]
fn undo_reuses_the_blobs_already_written() {
    let pool = pool();
    let (a, ex) = (doc_a(), SaveExtras::default());

    // Edited and undone before the next commit: the tiles are cached, so
    // they keep their blobs by pointer and the table is unchanged.
    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    w.commit(&a, &ex, &meta0(), &opts(), &pool, &Progress::default()).unwrap();
    let mut undone = edit(&a, 2, &[1, 2]);
    let (g, _) = undone.paint_target(LayerId(1)).unwrap();
    for x in [1, 2] {
        let c = TileCoord::new(x, 1);
        g.insert(c, a.layer(LayerId(1)).unwrap().raster().unwrap().get_ref(c).unwrap().clone());
    }
    let len = w.sink().len();
    let s = w.commit(&undone, &ex, &meta0(), &opts(), &pool, &Progress::default()).unwrap();
    assert!(s.unchanged && s.bytes_written == 0 && w.sink().len() == len, "{s:?}");

    // Undone after B was committed: A's old tiles left the cache, so they
    // are found by crc in the file and confirmed by decoding.
    let b = edit(&a, 3, &[0, 3, 9]);
    w.commit(&b, &ex, &meta0(), &opts(), &pool, &Progress::default()).unwrap();
    let s = w.commit(&a, &ex, &meta0(), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!((s.classified, s.encoded, s.copied, s.tables_written), (2, 0, 0, 1), "{s:?}");
    let loaded = read(&w.into_sink(), &pool);
    assert_eq!(loaded.info.commit_seq, 3);
    assert_same_doc(&a, &loaded.doc);

    // The same through a session, with a trim before the undo.
    let dir = temp_dir("inc-undo");
    let mut s = Session::new(session(), Some(&dir));
    s.autosave(&a, &ex, 1, &pool, &Progress::default()).unwrap();
    s.autosave(&b, &ex, 2, &pool, &Progress::default()).unwrap();
    drop(b);
    s.trim();
    let st = s.autosave(&a, &ex, 3, &pool, &Progress::default()).unwrap();
    assert_eq!((st.encoded, st.copied, st.tables_written), (0, 0, 1), "{st:?}");
    assert_same_doc(&a, &load(&s.recovery_path().unwrap()).doc);
    s.close(true).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unchanged_documents_write_nothing() {
    let pool = pool();
    let (a, ex) = (doc_a(), SaveExtras::default());
    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    w.commit(&a, &ex, &meta(&a), &opts(), &pool, &Progress::default()).unwrap();
    let len = w.sink().len();
    let s = w.commit(&a.snapshot(), &ex, &meta(&a), &opts(), &pool, &Progress::default()).unwrap();
    assert!(s.unchanged && s.bytes_written == 0 && w.sink().len() == len, "{s:?}");
    assert_eq!(s.file_len, len as u64);

    let dir = temp_dir("inc-noop");
    let mut s = Session::new(session(), Some(&dir));
    let first = s.autosave(&a, &ex, 5, &pool, &Progress::default()).unwrap();
    let st = s.autosave(&a.snapshot(), &ex, 5, &pool, &Progress::default()).unwrap();
    assert!(st.unchanged && st.bytes_written == 0, "{st:?}");
    assert_eq!(fs::metadata(s.recovery_path().unwrap()).unwrap().len(), first.file_len);
    // A new revision alone is a manifest-only commit.
    let st = s.autosave(&a, &ex, 6, &pool, &Progress::default()).unwrap();
    assert_eq!((st.encoded, st.copied, st.tables_written), (0, 0, 0));
    let file = fs::read(s.recovery_path().unwrap()).unwrap();
    assert_eq!(kinds_from(&file, first.file_len), [RecordKind::Manifest as u8, RecordKind::Commit as u8]);
    s.close(true).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn crc_collisions_across_commits_keep_both_tiles() {
    let pool = pool();
    let mut rng = Rng(99);
    let t1 = tile(&mut rng, 2);
    let mut t2 = tile(&mut rng, 3);
    let want = crc32fast::hash(bytemuck::bytes_of(&*t1));
    force_crc(bytemuck::bytes_of_mut(Arc::get_mut(&mut t2).unwrap()), want);
    assert!(*t1 != *t2);
    let mut a = Document::new(128, 128, 72);
    let (g, _) = a.paint_target(a.active()).unwrap();
    g.insert(TileCoord::new(0, 0), t1.clone());
    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    w.commit(&a, &SaveExtras::default(), &meta(&a), &opts(), &pool, &Progress::default()).unwrap();
    // t2 matches t1's crc in this commit and in the file; memcmp says no.
    let mut b = a.snapshot();
    let (g, _) = b.paint_target(b.active()).unwrap();
    g.insert(TileCoord::new(1, 0), t2.clone());
    let s = w.commit(&b, &SaveExtras::default(), &meta(&b), &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!(s.encoded, 1);
    let loaded = read(&w.into_sink(), &pool);
    assert_same_doc(&b, &loaded.doc);
}

#[test]
fn save_after_autosave_copies_every_blob() {
    let pool = pool();
    let dir = temp_dir("inc-ctrl-s");
    let (a, ex) = (doc_a(), SaveExtras::default());
    let mut s = Session::new(session(), Some(&dir.join("recovery")));
    // Untitled: the first autosave has nothing to copy from.
    let first = s.autosave(&a, &ex, a.revision(), &pool, &Progress::default()).unwrap();
    assert!(first.encoded > 0 && first.copied == 0);
    let rec = s.recovery_path().unwrap();
    assert_eq!(meta_value(&meta_of(&rec), "clean"), None);

    let path = dir.join("page.arty");
    let st = s.save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!((st.classified, st.encoded, st.copied, st.healed), (0, 0, first.encoded, 0), "{st:?}");
    // Copied blobs land where encoding would put them.
    assert!(fs::read(&path).unwrap() == write(&a, &ex, &pool));

    // The recovery file now says it holds what the main file holds.
    let m = meta_of(&rec);
    assert_eq!(meta_value(&m, "clean"), Some("1"));
    assert_eq!(meta_value(&m, "src"), Some(path.to_string_lossy().as_ref()));
    assert_eq!(meta_value(&m, "rev"), Some(a.revision().to_string().as_str()));
    let recovered = load(&rec);
    assert_same_doc(&a, &recovered.doc);

    s.close(true).unwrap();
    assert!(!rec.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn opening_seeds_copies_for_autosave_and_save() {
    let pool = pool();
    let dir = temp_dir("inc-open");
    let (a, ex) = (doc_a(), SaveExtras::default());
    let path = dir.join("page.arty");
    let made = Session::new(session(), None).save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();

    let mut loaded = load(&path);
    let mut s = Session::new(session(), Some(&dir.join("recovery")));
    s.adopt(&mut loaded, Some(path.clone()));
    let doc = &loaded.doc;
    // The first autosave after opening copies everything from the file.
    let st = s.autosave(doc, &ex, 1, &pool, &Progress::default()).unwrap();
    assert_eq!((st.classified, st.encoded, st.copied), (0, 0, made.encoded), "{st:?}");

    let b = edit(doc, 4, &[2, 5]);
    let st = s.autosave(&b, &ex, 2, &pool, &Progress::default()).unwrap();
    assert_eq!((st.classified, st.encoded, st.copied, st.tables_written), (2, 2, 0, 1), "{st:?}");

    let st = s.save_main(&b, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!((st.classified, st.encoded), (0, 0), "{st:?}");
    assert!(st.copied > 0);
    assert_same_doc(&b, &load(&path).doc);
    assert!(fs::read(&path).unwrap() == write(&b, &ex, &pool));
    s.close(true).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn damaged_blob_in_the_main_file_is_healed() {
    let pool = pool();
    let dir = temp_dir("inc-heal");
    let (a, ex) = (doc_a(), SaveExtras::default());
    let path = dir.join("page.arty");
    let made = Session::new(session(), None).save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    let mut loaded = load(&path);
    let mut s = Session::new(session(), None);
    s.adopt(&mut loaded, Some(path.clone()));

    // Bit rot in the first blob; size, uuid, commit and mtime unchanged.
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let (seg, kind, _) = records(&bytes)[0];
    assert_eq!(kind, RecordKind::Segment as u8);
    bytes[seg as usize + 24 + 5] ^= 0x40;
    fs::write(&path, &bytes).unwrap();
    fs::OpenOptions::new().write(true).open(&path).unwrap().set_modified(mtime).unwrap();

    let st = s.save_main(&loaded.doc, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!((st.healed, st.encoded, st.copied), (1, 1, made.encoded - 1), "{st:?}");
    assert_same_doc(&a, &load(&path).doc);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn externally_replaced_main_file_is_not_copied_from() {
    let pool = pool();
    let dir = temp_dir("inc-external");
    let (a, ex) = (doc_a(), SaveExtras::default());
    let path = dir.join("page.arty");
    Session::new(session(), None).save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    let mut loaded = load(&path);
    let mut s = Session::new(session(), None);
    s.adopt(&mut loaded, Some(path.clone()));

    // Someone else saves the same pixels over it (same blobs, new file).
    let other = arty_io::SaveOptions { uuid: Some([9; 16]), ..opts() };
    Session::new(session(), None).save_main(&a, &ex, &path, false, &other, &pool, &Progress::default()).unwrap();
    let r = s.save_main(&loaded.doc, &ex, &path, false, &opts(), &pool, &Progress::default());
    assert!(matches!(r, Err(arty_io::IoError::ExternallyModified)), "{r:?}");
    let st = s.save_main(&loaded.doc, &ex, &path, true, &opts(), &pool, &Progress::default()).unwrap();
    assert_eq!(st.copied, 0);
    assert!(st.encoded > 0);
    assert_same_doc(&a, &load(&path).doc);
    fs::remove_dir_all(&dir).unwrap();
}

/// Plan item 6, last case: a crash after the rename but before the clean
/// recovery commit leaves a recovery file the startup scan must classify
/// as obsolete (same session, revision not newer than the main file's).
#[test]
fn recovery_left_by_a_crash_after_rename_is_obsolete() {
    let pool = pool();
    let dir = temp_dir("inc-obsolete");
    let a = doc_a();
    let b = edit(&a, 5, &[4]);
    let ex = SaveExtras::default();
    let mut s = Session::new(session(), Some(&dir.join("recovery")));
    s.autosave(&a, &ex, a.revision(), &pool, &Progress::default()).unwrap();
    s.autosave(&b, &ex, b.revision(), &pool, &Progress::default()).unwrap();
    let rec = s.recovery_path().unwrap();
    let before_clean = fs::read(&rec).unwrap();
    let path = dir.join("page.arty");
    s.save_main(&b, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();

    let stale = dir.join("stale.arty");
    fs::write(&stale, &before_clean).unwrap();
    let (r, m) = (meta_of(&stale), meta_of(&path));
    assert_eq!(meta_value(&r, "session"), meta_value(&m, "session"));
    let rev = |meta: &[(String, String)]| meta_value(meta, "rev").unwrap().parse::<u64>().unwrap();
    assert!(rev(&r) <= rev(&m));
    assert_eq!(meta_value(&r, "clean"), None);
    assert_eq!(meta_value(&r, "src"), None, "untitled until saved");
    assert_eq!(meta_value(&meta_of(&rec), "clean"), Some("1"));
    s.close(true).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}

/// The recovery file's uuid (changes when it is rewritten).
fn file_uuid(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()[32..48].to_vec()
}

#[cfg(windows)]
fn assert_locked(s: &Session) {
    let r = fs::OpenOptions::new().read(true).open(s.lock_path().unwrap());
    assert_eq!(r.unwrap_err().raw_os_error(), Some(32), "the lock file is held");
}

#[cfg(not(windows))]
fn assert_locked(s: &Session) {
    assert!(s.lock_path().unwrap().exists());
}

/// Plan item 7: 1000 churn commits keep the recovery file within
/// `2·live + slack` (plus the commit just appended), compaction keeps the
/// lock, and the file always loads as the last state.
#[test]
fn churn_keeps_the_recovery_file_bounded() {
    let pool = pool();
    let dir = temp_dir("inc-churn");
    let ex = SaveExtras::default();
    let slack = 256 << 10;
    let mut s = Session::new(session(), Some(&dir));
    s.set_compaction(Compaction { slack, max_commits: 512 });
    let mut doc = doc_a();
    let mut rng = Rng(0xC0FFEE);
    let mut kept: Vec<TileRef> = Vec::new();
    let mut rewrites = 0;
    let mut uuid = Vec::new();
    let mut prev_live = 0;
    for i in 0..1000u64 {
        let (g, _) = doc.paint_target(LayerId(1)).unwrap();
        for _ in 0..2 {
            let c = TileCoord::new(rng.below(12) as i32, rng.below(2) as i32);
            // Sometimes back to an earlier tile, as undo would.
            let t = if !kept.is_empty() && rng.chance(1, 4) {
                kept[rng.below(kept.len() as u64) as usize].clone()
            } else {
                let kind = rng.pick(&[2, 3, 6]);
                tile(&mut rng, kind)
            };
            g.insert(c, t.clone());
            if kept.len() < 64 {
                kept.push(t);
            }
        }
        let st = s.autosave(&doc, &ex, i, &pool, &Progress::default()).unwrap();
        let rec = s.recovery_path().unwrap();
        assert_eq!(fs::metadata(&rec).unwrap().len(), st.file_len);
        // Checked before each append against the live bytes then.
        let live = prev_live.max(st.live_bytes);
        assert!(st.file_len <= 2 * live + slack + st.bytes_written, "commit {i}: {st:?}");
        prev_live = st.live_bytes;
        let now = file_uuid(&rec);
        if now != uuid {
            rewrites += 1;
            uuid = now;
            assert_locked(&s);
        }
        if i % 100 == 99 {
            assert_same_doc(&doc, &load(&rec).doc);
            s.trim();
        }
    }
    assert!(rewrites >= 3, "compactions: {}", rewrites - 1);
    assert_locked(&s);
    assert_same_doc(&doc, &load(&s.recovery_path().unwrap()).doc);
    s.close(true).unwrap();

    // The commit-count rule.
    let mut s = Session::new(session(), Some(&dir));
    s.set_compaction(Compaction { slack: u64::MAX / 4, max_commits: 10 });
    let mut uuids = Vec::new();
    for i in 0..25 {
        let doc = edit(&doc, i, &[(i % 8) as i32]);
        s.autosave(&doc, &ex, i, &pool, &Progress::default()).unwrap();
        uuids.push(file_uuid(&s.recovery_path().unwrap()));
    }
    uuids.dedup();
    assert_eq!(uuids.len(), 3, "created, then rewritten every 10 appends");
    s.close(true).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
