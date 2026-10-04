//! Crash and damage simulation for the rewrite save path and the reader's
//! commit search (plan item 6, main-file part).

mod common;

use std::fs;

use arty_core::{Document, TileCoord};
use arty_io::{FileWriter, IoError, LoadOptions, LoadWarning, Progress, SaveExtras, SaveOptions, Session};
use common::*;

/// A page with a mix of tile shapes.
fn doc_a() -> Document {
    let mut rng = Rng(0xA11CE);
    let mut doc = Document::new(512, 512, 350);
    let (g, _) = doc.paint_target(doc.active()).unwrap();
    for (i, kind) in [2, 3, 4, 5, 6, 1, 0].into_iter().enumerate() {
        g.insert(TileCoord::new(i as i32, 1), tile(&mut rng, kind));
    }
    doc
}

/// `a` with three tiles changed.
fn doc_b(a: &Document) -> Document {
    let mut rng = Rng(0xB0B);
    let mut doc = a.snapshot();
    let (g, _) = doc.paint_target(doc.active()).unwrap();
    for x in [0, 3, 9] {
        g.insert(TileCoord::new(x, 1), tile(&mut rng, 4));
    }
    doc
}

/// A file with commit A then commit B, and where A's commit ends.
fn two_commits(a: &Document, b: &Document) -> (Vec<u8>, u64) {
    let pool = pool();
    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    w.commit(a, &SaveExtras::default(), &meta(a), &opts(), &pool, &Progress::default()).unwrap();
    let end_a = w.sink().len() as u64;
    w.commit(b, &SaveExtras::default(), &meta(b), &opts(), &pool, &Progress::default()).unwrap();
    (w.into_sink(), end_a)
}

#[test]
fn rewrite_crash_leaves_the_target_untouched() {
    let pool = pool();
    let dir = temp_dir("crash-rewrite");
    let path = dir.join("page.arty");
    let tmp = dir.join("page.arty.saving~");
    let (a, ex) = (doc_a(), SaveExtras::default());
    let b = doc_b(&a);
    let mut s = Session::new(session(), None);
    s.save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    let a_bytes = fs::read(&path).unwrap();
    let b_bytes = write(&b, &ex, &pool);
    let len = b_bytes.len() as u64;

    // Every record boundary ±1, and 200 random points.
    let mut points = vec![0, 1, 63, 64, 65];
    for (at, _, end) in records(&b_bytes) {
        points.extend([at - 1, at, at + 1, end - 1, end, end + 1]);
    }
    let mut rng = Rng(0x5EED);
    points.extend((0..200).map(|_| rng.below(len)));
    points.retain(|&k| k < len);
    points.sort_unstable();
    points.dedup();

    for &k in &points {
        for lose_unsynced in [false, true] {
            let r = s.save_main_crashing(&b, &ex, &path, &opts(), &pool, &Progress::default(), k, lose_unsynced);
            assert!(matches!(r, Err(IoError::Io { .. })), "k={k}: {r:?}");
            assert!(fs::read(&path).unwrap() == a_bytes, "k={k}: target changed");
            let partial = fs::read(&tmp).expect("temp file left behind");
            assert!(partial.len() as u64 <= k);
            assert!(partial[..] == b_bytes[..partial.len()], "k={k}: temp is a prefix of the save");
        }
    }
    // The stale temp file is replaced by the next save.
    s.save_main(&b, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    assert!(fs::read(&path).unwrap() == b_bytes);
    assert!(!tmp.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn torn_second_commit_loads_the_first() {
    let pool = pool();
    let a = doc_a();
    let (bytes, end_a) = two_commits(&a, &doc_b(&a));
    for cut in end_a + 1..bytes.len() as u64 {
        let loaded = read(&bytes[..cut as usize], &pool);
        assert!(loaded.info.recovered, "cut {cut}");
        assert_eq!(loaded.info.commit_seq, 1);
        assert_eq!(loaded.warnings, [LoadWarning::RecoveredTornTail], "cut {cut}");
        assert_same_doc(&a, &loaded.doc);
    }
    let whole = read(&bytes, &pool);
    assert_eq!((whole.info.commit_seq, whole.info.recovered), (2, false));
}

#[test]
fn damaged_commit_falls_back_or_salvages() {
    let pool = pool();
    let a = doc_a();
    let b = doc_b(&a);
    let (mut bytes, end_a) = two_commits(&a, &b);
    let (at, _, end) = records(&bytes).into_iter().find(|&(at, kind, _)| at >= end_a && kind == 1).unwrap();
    bytes[at as usize + 24..end as usize].fill(0);

    assert!(matches!(read_with(&bytes, &LoadOptions::default(), &pool), Err(IoError::Corrupt { .. })));

    let fallback = LoadOptions { fallback_to_previous: true, ..Default::default() };
    let loaded = read_with(&bytes, &fallback, &pool).unwrap();
    assert_eq!(loaded.warnings, [LoadWarning::FellBackToCommit { seq: 1 }]);
    assert_eq!(loaded.info.commit_seq, 1);
    assert_same_doc(&a, &loaded.doc);

    let salvage = LoadOptions { salvage: true, ..Default::default() };
    let loaded = read_with(&bytes, &salvage, &pool).unwrap();
    assert_eq!(loaded.info.commit_seq, 2);
    assert!(matches!(loaded.warnings[..], [LoadWarning::DamagedTiles { count }] if count > 0), "{:?}", loaded.warnings);
    assert!(loaded.read_only_reason.is_some());
    // Solid tiles need no blob and survive.
    let g = loaded.doc.active_layer().raster().unwrap();
    assert!(g.get(TileCoord::new(5, 1)).is_some() && g.get(TileCoord::new(6, 1)).is_some());
}

#[test]
fn backward_scan_finds_a_commit_past_mid_file_damage() {
    let pool = pool();
    let a = doc_a();
    let b = doc_b(&a);
    let (mut bytes, _) = two_commits(&a, &b);
    bytes[64] ^= 0xFF; // first record header: the forward scan stops here
    bytes.extend_from_slice(b"ArRc and a torn tail"); // and the tail is not a commit
    let loaded = read(&bytes, &pool);
    assert!(loaded.info.recovered);
    assert_eq!(loaded.info.commit_seq, 2);
    assert_eq!(loaded.warnings, [LoadWarning::RecoveredTornTail]);
    assert_same_doc(&b, &loaded.doc);
}

#[test]
fn externally_modified_target_needs_overwrite() {
    let pool = pool();
    let dir = temp_dir("crash-external");
    let path = dir.join("page.arty");
    let (a, ex) = (doc_a(), SaveExtras::default());
    let b = doc_b(&a);
    let mut s = Session::new(session(), None);
    s.save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    // Unchanged on disk: saving again is fine.
    s.save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();

    // Another program (another session) writes the file.
    let other = SaveOptions { uuid: Some([1; 16]), ..opts() };
    Session::new(session(), None).save_main(&b, &ex, &path, false, &other, &pool, &Progress::default()).unwrap();
    let theirs = fs::read(&path).unwrap();
    let r = s.save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default());
    assert!(matches!(r, Err(IoError::ExternallyModified)), "{r:?}");
    assert!(fs::read(&path).unwrap() == theirs);

    s.save_main(&a, &ex, &path, true, &opts(), &pool, &Progress::default()).unwrap();
    assert_same_doc(&a, &read(&fs::read(&path).unwrap(), &pool).doc);
    // A load adopts the file's identity.
    let mut loaded = arty_io::load(&path, &LoadOptions::default(), &pool, &Progress::default()).unwrap();
    let mut s2 = Session::new(session(), None);
    s2.adopt(&mut loaded, Some(path.clone()));
    assert_eq!(s2.main_path(), Some(path.as_path()));
    s2.save_main(&b, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    let r = s.save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default());
    assert!(matches!(r, Err(IoError::ExternallyModified)));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn saving_over_a_v1_file_backs_it_up_first() {
    let pool = pool();
    let dir = temp_dir("crash-v1");
    let path = dir.join("old.arty");
    let mut v1 = b"ARTY\x01\0\0\0".to_vec();
    v1.extend_from_slice(&[0x18, 0, 0, 0, 0, 0, 0, 0, 0x18, 0, 0, 0, 0, 0, 0, 0]);
    let a = doc_a();
    for backup in ["old.v1-backup.arty", "old.v1-backup-1.arty"] {
        fs::write(&path, &v1).unwrap();
        let mut s = Session::new(session(), None);
        s.save_main(&a, &SaveExtras::default(), &path, false, &opts(), &pool, &Progress::default()).unwrap();
        assert_eq!(fs::read(dir.join(backup)).unwrap(), v1, "{backup}");
        assert_same_doc(&a, &read(&fs::read(&path).unwrap(), &pool).doc);
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(windows)]
#[test]
fn held_files_give_busy_or_saved_to_temp() {
    use std::os::windows::fs::OpenOptionsExt;

    let pool = pool();
    let dir = temp_dir("crash-held");
    let path = dir.join("page.arty");
    let tmp = dir.join("page.arty.saving~");
    let (a, ex) = (doc_a(), SaveExtras::default());
    let b = doc_b(&a);
    let mut s = Session::new(session(), None);
    s.save_main(&a, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    let a_bytes = fs::read(&path).unwrap();

    // Someone else is writing the temp file.
    let held = fs::OpenOptions::new().write(true).create(true).truncate(true).share_mode(0).open(&tmp).unwrap();
    let r = s.save_main(&b, &ex, &path, false, &opts(), &pool, &Progress::default());
    assert!(matches!(r, Err(IoError::Busy)), "{r:?}");
    drop(held);

    // The target is open without delete sharing (antivirus, a viewer):
    // the rename fails after its retries and the save stays in the temp.
    const FILE_SHARE_READ: u32 = 1;
    let held = fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path).unwrap();
    let r = s.save_main(&b, &ex, &path, false, &opts(), &pool, &Progress::default());
    assert!(matches!(&r, Err(IoError::SavedToTemp(p)) if *p == tmp), "{r:?}");
    drop(held);
    assert!(fs::read(&path).unwrap() == a_bytes);
    assert_same_doc(&b, &read(&fs::read(&tmp).unwrap(), &pool).doc);

    s.save_main(&b, &ex, &path, false, &opts(), &pool, &Progress::default()).unwrap();
    assert!(!tmp.exists());
    fs::remove_dir_all(&dir).unwrap();
}
