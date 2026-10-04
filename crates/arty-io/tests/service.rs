//! The IO thread and the recovery folder (plan item 11): queued autosaves
//! coalesce to the newest, saves are never dropped, shutdown finishes the
//! queued work, a cancelled load changes nothing, the startup scan sorts
//! recovery files, and a restored file is deleted after the first
//! autosave.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use arty_core::{Document, TileCoord};
use arty_io::{
    IoConfig, IoError, IoEvent, IoService, LoadOptions, Progress, RecoveryDir, Request, SaveExtras, SaveOptions, Session,
    SessionId,
};
use common::*;

/// A small page whose bottom layer holds `n` noise tiles from `seed`.
fn doc(seed: u64, n: i32) -> Document {
    let mut rng = Rng(seed);
    let mut doc = Document::new(256, 256, 72);
    let id = doc.active();
    let (g, _) = doc.paint_target(id).unwrap();
    for x in 0..n {
        g.insert(TileCoord::new(x % 4, x / 4), tile(&mut rng, 2));
    }
    doc
}

fn spawn(dir: &Path) -> IoService {
    let cfg = IoConfig { threads: 2, recovery_dir: dir.join("recovery"), load: LoadOptions::default() };
    IoService::spawn(cfg, || {}).unwrap()
}

/// Block the IO thread until the returned sender is dropped.
fn pause(io: &IoService) -> Sender<()> {
    let (go, rx) = channel();
    io.send(Request::Pause(rx));
    go
}

/// Wait until the IO thread is idle, then take every event.
fn settle(io: &IoService) -> Vec<IoEvent> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while io.busy() {
        assert!(Instant::now() < deadline, "the io thread did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
    std::iter::from_fn(|| io.try_recv()).collect()
}

fn autosave(io: &IoService, doc: &Document) -> u64 {
    io.send(Request::Autosave { doc: Box::new(doc.snapshot()), ex: SaveExtras::default(), rev: doc.revision() })
}

fn save(io: &IoService, doc: &Document, path: &Path) -> u64 {
    io.send(Request::Save {
        doc: Box::new(doc.snapshot()),
        ex: SaveExtras::default(),
        path: path.to_path_buf(),
        rev: doc.revision(),
        overwrite_external: false,
    })
}

fn files_with(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.to_string_lossy().ends_with(suffix)).collect())
        .unwrap_or_default();
    out.sort();
    out
}

fn meta(path: &Path, key: &str) -> Option<String> {
    arty_io::read_info(path).unwrap().meta.into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn load(path: &Path) -> arty_io::Loaded {
    arty_io::load(path, &LoadOptions::default(), &pool(), &Progress::default()).unwrap()
}

/// `doc` with its revision raised by `n` (revisions tell queued autosaves
/// apart).
fn with_rev(mut doc: Document, n: u64) -> Document {
    let id = doc.active();
    let rev = doc.revision();
    for _ in 0..n {
        let mut p = doc.layer(id).unwrap().props.clone();
        p.opacity = if p.opacity == 1.0 { 0.5 } else { 1.0 };
        doc.set_props(id, p);
    }
    assert_eq!(doc.revision(), rev + n);
    doc
}

#[test]
fn queued_autosaves_write_only_the_newest() {
    let dir = temp_dir("svc-coalesce");
    let io = spawn(&dir);
    let go = pause(&io);
    let docs: Vec<Document> = (1..=3).map(|r| with_rev(doc(r, 6), r)).collect();
    let tickets: Vec<u64> = docs.iter().map(|d| autosave(&io, d)).collect();
    drop(go);
    let events = settle(&io);
    assert_eq!(events.len(), 1, "one write for three queued autosaves");
    let IoEvent::Autosaved { ticket, rev, stats } = &events[0] else { panic!("expected Autosaved") };
    assert_eq!((*ticket, *rev), (tickets[2], docs[2].revision()));
    assert!(!stats.unchanged);

    let rec = files_with(&dir.join("recovery"), ".arty");
    assert_eq!(rec.len(), 1);
    assert_eq!(arty_io::read_info(&rec[0]).unwrap().commit_seq, 1, "a single commit");
    assert_eq!(meta(&rec[0], "rev"), Some(docs[2].revision().to_string()));
    assert_same_doc(&docs[2], &load(&rec[0]).doc);
    io.shutdown();
}

#[test]
fn saves_are_never_dropped() {
    let dir = temp_dir("svc-saves");
    let io = spawn(&dir);
    let go = pause(&io);
    let docs: Vec<Document> = (1..=5).map(|r| with_rev(doc(r * 7, 5), r)).collect();
    let (a, b) = (dir.join("a.arty"), dir.join("b.arty"));
    autosave(&io, &docs[0]);
    let ta = save(&io, &docs[1], &a);
    autosave(&io, &docs[2]);
    let tb = save(&io, &docs[3], &b);
    let t5 = autosave(&io, &docs[4]);
    drop(go);

    let mut saved = Vec::new();
    let mut autosaved = Vec::new();
    for e in settle(&io) {
        match e {
            IoEvent::Saved { ticket, path, rev, .. } => saved.push((ticket, path, rev)),
            IoEvent::Autosaved { ticket, rev, .. } => autosaved.push((ticket, rev)),
            IoEvent::Failed { op, error, .. } => panic!("{op}: {error}"),
            _ => panic!("unexpected event"),
        }
    }
    let rev = |i: usize| docs[i].revision();
    assert_eq!(saved, vec![(ta, a.clone(), rev(1)), (tb, b.clone(), rev(3))]);
    assert_eq!(autosaved, vec![(t5, rev(4))], "autosaves followed by a save or autosave are dropped");
    assert_same_doc(&docs[1], &load(&a).doc);
    assert_same_doc(&docs[3], &load(&b).doc);
    io.shutdown();
}

#[test]
fn shutdown_finishes_the_queued_work() {
    let dir = temp_dir("svc-shutdown");
    let io = spawn(&dir);
    let d = doc(11, 8);
    autosave(&io, &d);
    settle(&io);
    let rec_dir = dir.join("recovery");
    assert_eq!(files_with(&rec_dir, ".lock").len(), 1, "a session that autosaved holds its lock");

    let go = pause(&io);
    let path = dir.join("c.arty");
    save(&io, &d, &path);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(go);
    });
    io.shutdown();
    release.join().unwrap();
    assert_same_doc(&d, &load(&path).doc);
    // The lock is released and the recovery file kept (the app discards
    // it explicitly with CloseSession).
    assert_eq!(files_with(&rec_dir, ".lock").len(), 0);
    assert_eq!(files_with(&rec_dir, ".arty").len(), 1);
}

#[test]
fn cancelled_load_leaves_no_state() {
    let dir = temp_dir("svc-cancel");
    let path = dir.join("f.arty");
    let d = doc(5, 10);
    fs::write(&path, write(&d, &SaveExtras::default(), &pool())).unwrap();
    let rec_dir = dir.join("recovery");

    let io = spawn(&dir);
    let go = pause(&io);
    let t = io.send(Request::Open { path: path.clone() });
    io.cancel(t);
    drop(go);
    let events = settle(&io);
    assert!(matches!(events.as_slice(), [IoEvent::Failed { ticket, error: IoError::Cancelled, .. }] if *ticket == t));

    // Not adopted: the next autosave runs (the cancel does not leak into
    // it) and names no main file.
    let other = doc(6, 3);
    autosave(&io, &other);
    assert!(matches!(settle(&io).as_slice(), [IoEvent::Autosaved { .. }]));
    let rec = files_with(&rec_dir, ".arty");
    assert_eq!(rec.len(), 1);
    assert_eq!(meta(&rec[0], "src"), None);
    assert_same_doc(&other, &load(&rec[0]).doc);

    // The same open, not cancelled, is adopted and starts a new session.
    let t = io.send(Request::Open { path: path.clone() });
    let events = settle(&io);
    let [IoEvent::Loaded { ticket, path: Some(p), loaded }] = events.as_slice() else { panic!("expected Loaded") };
    assert_eq!((*ticket, p), (t, &path));
    assert_same_doc(&d, &loaded.doc);
    assert!(files_with(&rec_dir, ".arty").is_empty(), "the previous session's recovery file was discarded");
    autosave(&io, &loaded.doc);
    let events = settle(&io);
    let [IoEvent::Autosaved { stats, .. }] = events.as_slice() else { panic!("expected Autosaved") };
    assert_eq!(stats.encoded, 0, "seeded from the opened file");
    let rec = files_with(&rec_dir, ".arty");
    assert_eq!(meta(&rec[0], "src").map(PathBuf::from), Some(path));
    io.shutdown();
}

fn session(dir: &Path, id: u8) -> Session {
    Session::new(SessionId([id; 16]), Some(dir))
}

#[test]
fn recovery_scan_skips_live_sessions_and_deletes_clean_and_obsolete_ones() {
    let root = temp_dir("svc-scan");
    let dir = root.join("recovery");
    let pool = pool();
    let p = Progress::default();
    let o = SaveOptions::default();
    let ex = SaveExtras::default();

    // Live: autosaved and still running (holds its lock).
    let mut live = session(&dir, 0xA1);
    let d = doc(1, 4);
    live.autosave(&d, &ex, d.revision(), &pool, &p).unwrap();
    let live_tmp = PathBuf::from(format!("{}.saving~", live.recovery_path().unwrap().display()));
    fs::write(&live_tmp, b"partial").unwrap();

    // Clean: saved after its last autosave.
    let mut clean = session(&dir, 0xB2);
    let d = with_rev(doc(2, 4), 3);
    clean.autosave(&d, &ex, d.revision(), &pool, &p).unwrap();
    clean.save_main(&d, &ex, &root.join("clean.arty"), false, &o, &pool, &p).unwrap();
    let clean_rec = clean.recovery_path().unwrap();
    assert_eq!(meta(&clean_rec, "clean").as_deref(), Some("1"));
    clean.close(false).unwrap();

    // Obsolete: crashed after the save's rename, before the clean commit.
    let mut obsolete = session(&dir, 0xC3);
    let main = root.join("obsolete.arty");
    let d = with_rev(doc(3, 4), 2);
    obsolete.save_main(&d, &ex, &main, false, &o, &pool, &p).unwrap();
    let d = with_rev(d, 5);
    obsolete.autosave(&d, &ex, d.revision(), &pool, &p).unwrap();
    let obsolete_rec = obsolete.recovery_path().unwrap();
    let before_save = fs::read(&obsolete_rec).unwrap();
    obsolete.save_main(&d, &ex, &main, false, &o, &pool, &p).unwrap();
    fs::write(&obsolete_rec, &before_save).unwrap();
    assert_eq!(meta(&obsolete_rec, "clean"), None);
    obsolete.close(false).unwrap();

    // Offered: edits after the last save, then a crash.
    let mut crashed = session(&dir, 0xD4);
    let main_d = root.join("crashed.arty");
    let d = with_rev(doc(4, 4), 1);
    crashed.save_main(&d, &ex, &main_d, false, &o, &pool, &p).unwrap();
    let d = with_rev(d, 3);
    crashed.autosave(&d, &ex, d.revision(), &pool, &p).unwrap();
    let crashed_rec = crashed.recovery_path().unwrap();
    crashed.close(false).unwrap();

    // Leftovers of dead sessions.
    let stale_tmp = dir.join(format!("{}.arty.saving~", SessionId([0xE5; 16]).hex()));
    let stale_lock = dir.join(format!("{}.lock", SessionId([0xF6; 16]).hex()));
    fs::write(&stale_tmp, b"partial").unwrap();
    fs::write(&stale_lock, b"").unwrap();

    let found = RecoveryDir::new(&dir).scan();
    assert_eq!(found.len(), 1, "{found:?}");
    let e = &found[0];
    assert_eq!(e.path, crashed_rec);
    assert_eq!(e.session, SessionId([0xD4; 16]).hex());
    assert_eq!(e.src.as_deref(), Some(main_d.as_path()));
    assert_eq!((e.rev, e.size), (d.revision(), fs::metadata(&crashed_rec).unwrap().len()));
    assert_eq!(e.display_name(), "crashed.arty");

    assert!(live.recovery_path().unwrap().exists(), "a live session is left alone");
    assert!(live_tmp.exists(), "a live session's temp file is left alone");
    assert!(!clean_rec.exists(), "a clean recovery file is deleted");
    assert!(!obsolete_rec.exists(), "an obsolete recovery file is deleted");
    assert!(!stale_tmp.exists() && !stale_lock.exists(), "leftovers of dead sessions are deleted");
    assert!(crashed_rec.exists());

    RecoveryDir::new(&dir).discard(e).unwrap();
    assert!(!crashed_rec.exists());
    assert!(RecoveryDir::new(&dir).scan().is_empty());
    live.close(true).unwrap();
}

#[test]
fn restored_file_is_deleted_after_the_first_autosave() {
    let root = temp_dir("svc-restore");
    let dir = root.join("recovery");
    let pool = pool();
    let p = Progress::default();
    let mut old = session(&dir, 0x77);
    let d = with_rev(doc(9, 12), 4);
    old.autosave(&d, &SaveExtras::default(), d.revision(), &pool, &p).unwrap();
    let old_rec = old.recovery_path().unwrap();
    old.close(false).unwrap();

    let io = spawn(&root);
    io.send(Request::ScanRecovery);
    let events = settle(&io);
    let [IoEvent::RecoveryFound(found)] = events.as_slice() else { panic!("expected RecoveryFound") };
    assert_eq!(found.len(), 1);
    let entry = found[0].clone();
    assert_eq!(entry.path, old_rec);

    let t = io.send(Request::Restore { entry: entry.clone() });
    let events = settle(&io);
    let [IoEvent::Loaded { ticket, path, loaded }] = events.as_slice() else { panic!("expected Loaded") };
    assert_eq!((*ticket, path), (t, &None));
    assert_same_doc(&d, &loaded.doc);
    assert!(old_rec.exists(), "kept until the new session has autosaved");
    // Claimed: another scan does not offer it again.
    assert!(RecoveryDir::new(&dir).scan().is_empty());

    autosave(&io, &loaded.doc);
    let events = settle(&io);
    let [IoEvent::Autosaved { stats, .. }] = events.as_slice() else { panic!("expected Autosaved") };
    assert_eq!(stats.encoded, 0, "copied from the restored file");
    assert!(stats.copied > 0);
    assert!(!old_rec.exists(), "deleted after the first autosave");
    let rec = files_with(&dir, ".arty");
    assert_eq!(rec.len(), 1);
    assert_same_doc(&d, &load(&rec[0]).doc);
    io.send(Request::CloseSession { discard_recovery: true });
    settle(&io);
    assert!(files_with(&dir, ".arty").is_empty() && files_with(&dir, ".lock").is_empty());
    io.shutdown();
}
