//! I/O benchmark B002: spec §14 targets T1–T14 on the synthetic manga page.
//!
//! cargo run --release -p arty-io --example io_bench -- --preset b4-600-30l --out plans/bench/B002_io.md
//!
//! Options: `--preset b4-600-30l | b4-350`, `--out FILE` (Markdown report;
//! stdout only when absent), `--dir DIR` (work files, default
//! `target/io_bench`; needs about 3× the main file size free),
//! `--skip-t10` (the 2-hour simulation), `--threads N` (io pool size,
//! default `cores - 1`; 7 matches the spec's 8-core reference machine).
//!
//! Saves run on an io pool at below-normal priority, as the app's io
//! service does. Every file is written for real (fsync
//! included), so results depend on the disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ahash::{AHashMap, AHashSet};
use arty_core::tile::new_tile_box;
use arty_core::{CompositeScratch, Document, LayerId, TileCoord, TileRef};
use arty_io::codec::{self, BlobCodec, CodecScratch, TileClass};
use arty_io::format::{TileCodec, TileEntry};
use arty_io::{LoadOptions, Progress, SaveExtras, SaveOptions, SaveStats, Session, SessionId, Verify};
use arty_testkit::{LayerType, Page, synthetic_manga_page};
use rayon::ThreadPool;
use rayon::prelude::*;

#[path = "../tests/common/v1.rs"]
#[allow(dead_code)]
mod v1;

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

const MIB: f64 = (1u64 << 20) as f64;
const TILE: u64 = 32768;

struct Args {
    preset: String,
    out: Option<PathBuf>,
    dir: PathBuf,
    skip_t10: bool,
    threads: usize,
}

fn args() -> Args {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut a = Args {
        preset: "b4-600-30l".into(),
        out: None,
        dir: PathBuf::from("target/io_bench"),
        skip_t10: false,
        threads: cores.saturating_sub(1).max(1),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--preset" => a.preset = it.next().expect("--preset NAME"),
            "--out" => a.out = Some(it.next().expect("--out FILE").into()),
            "--dir" => a.dir = it.next().expect("--dir DIR").into(),
            "--skip-t10" => a.skip_t10 = true,
            "--threads" => a.threads = it.next().and_then(|n| n.parse().ok()).expect("--threads N"),
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

/// The io pool: `threads` threads below normal priority.
fn io_pool(threads: usize) -> ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("arty-io-{i}"))
        .start_handler(|_| below_normal())
        .build()
        .unwrap()
}

#[cfg(windows)]
fn below_normal() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    // SAFETY: plain Win32 call on the current thread's pseudo-handle.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(not(windows))]
fn below_normal() {}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn timed<R>(f: impl FnOnce() -> R) -> (R, f64) {
    let t = Instant::now();
    let r = f();
    (r, ms(t.elapsed()))
}

/// Run a load and split its wall time by `Progress`: reading metadata and
/// tables, decoding blobs (until every blob is done), then building the
/// document. Sampled every 0.2 ms.
fn load_phases<R: Send>(p: &Progress, f: impl FnOnce() -> R + Send) -> (R, f64, String) {
    let done = AtomicBool::new(false);
    let t = Instant::now();
    let (r, marks) = std::thread::scope(|scope| {
        let sampler = scope.spawn(|| {
            let (mut decode_start, mut decode_end) = (None, None);
            while !done.load(Ordering::Relaxed) {
                let phase = p.phase.load(Ordering::Relaxed);
                if phase == arty_io::phase::DECODE {
                    decode_start.get_or_insert_with(|| t.elapsed());
                    let total = p.total.load(Ordering::Relaxed);
                    if total > 0 && p.done.load(Ordering::Relaxed) >= total {
                        decode_end.get_or_insert_with(|| t.elapsed());
                    }
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            (decode_start, decode_end)
        });
        let r = f();
        done.store(true, Ordering::Relaxed);
        (r, sampler.join().unwrap())
    });
    let wall = ms(t.elapsed());
    let split = match marks {
        (Some(a), Some(b)) => {
            format!("read {:.0} ms, decode {:.0} ms, build {:.0} ms", ms(a), ms(b) - ms(a), wall - ms(b))
        }
        _ => String::from("phases not sampled"),
    };
    (r, wall, split)
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

struct Row {
    id: &'static str,
    metric: String,
    target: &'static str,
    result: String,
    pass: Option<bool>,
}

struct Report {
    rows: Vec<Row>,
    notes: Vec<String>,
}

impl Report {
    fn row(&mut self, id: &'static str, metric: impl Into<String>, target: &'static str, result: String, pass: Option<bool>) {
        let metric = metric.into();
        let mark = match pass {
            Some(true) => "pass",
            Some(false) => "MISS",
            None => "-",
        };
        eprintln!("{id:>4} {metric}: {result} (target {target}) [{mark}]");
        self.rows.push(Row { id, metric, target, result, pass });
    }

    fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        eprintln!("     {s}");
        self.notes.push(s);
    }
}

fn stats_line(s: &SaveStats) -> String {
    format!(
        "tiles {} · classified {} · encoded {} · copied {} · reused {} · tables {} · written {:.1} MiB · \
         plan {:.0} / encode {:.0} / io {:.0} / fsync {:.0} / verify {:.0} ms",
        s.tiles,
        s.classified,
        s.encoded,
        s.copied,
        s.reused,
        s.tables_written,
        s.bytes_written as f64 / MIB,
        s.ms_plan,
        s.ms_encode,
        s.ms_io,
        s.ms_fsync,
        s.ms_verify
    )
}

fn rasters(doc: &Document) -> Vec<LayerId> {
    let mut out = Vec::new();
    let mut stack: Vec<LayerId> = doc.root().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let l = doc.layer(id).unwrap();
        match l.children() {
            Some(c) => stack.extend(c.iter().rev()),
            None => out.push(id),
        }
    }
    out
}

/// Distinct tile allocations (pixel memory actually held).
fn unique_tiles(doc: &Document) -> usize {
    let mut seen = AHashSet::new();
    for id in rasters(doc) {
        for (_, t) in doc.layer(id).unwrap().raster().unwrap().iter() {
            seen.insert(Arc::as_ptr(t) as usize);
        }
    }
    seen.len()
}

fn tile_count(doc: &Document) -> usize {
    rasters(doc).iter().map(|&id| doc.layer(id).unwrap().raster().unwrap().len()).sum()
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map_or(0, |m| m.len())
}

fn opts(verify: Verify) -> SaveOptions {
    SaveOptions { verify, now_ms: None, uuid: None }
}

/// Paint one stroke: a 3 px line of `len` pixels on `layer`.
fn stroke(doc: &mut Document, layer: LayerId, rng: &mut u64, len: i32) {
    let mut next = || {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        *rng
    };
    let (w, h) = (doc.width() as i32, doc.height() as i32);
    let (x0, y0) = ((next() % w as u64) as i32, (next() % h as u64) as i32);
    let angle = (next() % 6283) as f64 / 1000.0;
    let (dx, dy) = (angle.cos(), angle.sin());
    let grid = doc.paint_target(layer).unwrap().0;
    for i in 0..len {
        let (x, y) = (x0 + (dx * i as f64) as i32, y0 + (dy * i as f64) as i32);
        for (ox, oy) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)] {
            let (px, py) = (x + ox, y + oy);
            let c = TileCoord::from_pixel(px, py);
            let t = grid.get_mut_or_create(c);
            t[py.rem_euclid(64) as usize][px.rem_euclid(64) as usize] = [0, 0, 0, 1 << 15];
        }
    }
}

/// T9: one UI frame recomposites a brush's dirty tiles on the global pool.
fn frame(doc: &Document, coords: &[TileCoord]) -> f64 {
    let t = Instant::now();
    coords.par_iter().for_each_init(
        || (CompositeScratch::new(), new_tile_box()),
        |(s, out), &c| doc.composite_tile(c, out, s),
    );
    ms(t.elapsed())
}

fn frames_until(doc: &Document, coords: &[TileCoord], done: &AtomicBool, min_frames: usize) -> Vec<f64> {
    let mut out = Vec::new();
    while out.len() < min_frames || !done.load(Ordering::Relaxed) {
        let start = Instant::now();
        out.push(frame(doc, coords));
        // 60 Hz pacing.
        if let Some(rest) = Duration::from_micros(16_667).checked_sub(start.elapsed()) {
            std::thread::sleep(rest);
        }
        if done.load(Ordering::Relaxed) && out.len() >= min_frames {
            break;
        }
    }
    out
}

/// T8: stored size of each layer type's distinct general tiles with each
/// codec, and the decode cost per tile.
fn codec_table(doc: &Document, pool: &ThreadPool, r: &mut Report) {
    let mut by_type: AHashMap<LayerType, (u64, Vec<TileRef>)> = AHashMap::new();
    let mut seen = AHashSet::new();
    for id in rasters(doc) {
        let l = doc.layer(id).unwrap();
        let Some(kind) = LayerType::of_layer(&l.props.name) else { continue };
        let e = by_type.entry(kind).or_default();
        for (_, t) in l.raster().unwrap().iter() {
            e.0 += 1;
            if seen.insert(Arc::as_ptr(t) as usize) {
                e.1.push(t.clone());
            }
        }
    }
    r.note("T8 per layer type (distinct tiles; SOLID tiles store 0 bytes; decode = ms per 1000 blobs, io pool):");
    r.note("| type | tiles | SHUF ratio | VDELTA ratio | SHUF decode | VDELTA decode |");
    r.note("|---|---:|---:|---:|---:|---:|");
    let (mut all_shuf, mut all_vd, mut all_raw) = (0u64, 0u64, 0u64);
    let (mut dec_shuf, mut dec_vd, mut blobs) = (0.0, 0.0, 0usize);
    for kind in LayerType::ALL {
        let Some((count, tiles)) = by_type.get(&kind) else { continue };
        let raw = *count * TILE;
        let mut sizes = [0u64; 2];
        let mut decode_ms = [0f64; 2];
        let mut n = 0;
        for (k, codec) in [BlobCodec::Lz4Shuf, BlobCodec::Lz4Vdelta].into_iter().enumerate() {
            let encoded: Vec<Option<(TileEntry, Vec<u8>)>> = pool.install(|| {
                tiles
                    .par_iter()
                    .map_init(CodecScratch::new, |s, t| match codec::classify(t) {
                        TileClass::Solid(_) => None,
                        TileClass::General { raw_crc } => {
                            let e = codec::encode_tile(t, raw_crc, codec, s);
                            let entry = TileEntry {
                                coord: TileCoord::new(0, 0),
                                codec: e.codec,
                                stored_len: e.bytes.len() as u32,
                                raw_crc,
                                stored_crc: e.stored_crc,
                                offset: 64,
                            };
                            Some((entry, e.bytes.to_vec()))
                        }
                    })
                    .collect()
            });
            let encoded: Vec<_> = encoded.into_iter().flatten().collect();
            sizes[k] = encoded.iter().map(|(e, _)| u64::from(e.stored_len)).sum();
            n = encoded.len();
            let (_, t) = timed(|| {
                pool.install(|| {
                    encoded.par_iter().for_each_init(
                        || (CodecScratch::new(), new_tile_box()),
                        |(s, out), (e, b)| {
                            debug_assert!(e.codec != TileCodec::Solid);
                            codec::decode_tile(e, b, out, s).unwrap();
                        },
                    )
                })
            });
            decode_ms[k] = t;
        }
        let per_k = |t: f64| if n == 0 { 0.0 } else { t * 1000.0 / n as f64 };
        r.note(format!(
            "| {} | {} | {:.3} | {:.3} | {:.2} | {:.2} |",
            kind.name(),
            count,
            sizes[0] as f64 / raw as f64,
            sizes[1] as f64 / raw as f64,
            per_k(decode_ms[0]),
            per_k(decode_ms[1])
        ));
        all_shuf += sizes[0];
        all_vd += sizes[1];
        all_raw += raw;
        dec_shuf += decode_ms[0];
        dec_vd += decode_ms[1];
        blobs += n;
    }
    let smaller = 1.0 - all_vd as f64 / all_shuf as f64;
    let cost = dec_vd / dec_shuf;
    r.note(format!(
        "All: SHUF {:.3}, VDELTA {:.3} of raw ({blobs} blobs); VDELTA is {:.1}% {} at {:.2}x the decode time. \
         Rule (≥ 10% smaller at ≤ 1.3x decode): {}.",
        all_shuf as f64 / all_raw as f64,
        all_vd as f64 / all_raw as f64,
        smaller.abs() * 100.0,
        if smaller >= 0.0 { "smaller" } else { "larger" },
        cost,
        if smaller >= 0.10 && cost <= 1.3 { "switch the writer default to VDELTA" } else { "keep SHUF" }
    ));
}

fn main() {
    let a = args();
    let page = match a.preset.as_str() {
        "b4-600-30l" => Page::B4_600,
        "b4-350" => Page::B4_350,
        other => panic!("unknown preset {other} (b4-600-30l, b4-350)"),
    };
    std::fs::create_dir_all(&a.dir).unwrap();
    let dir = a.dir.canonicalize().unwrap();
    let pool = io_pool(a.threads);
    let p = Progress::default();
    let ex = SaveExtras { title: "io_bench".into(), ..Default::default() };
    let mut r = Report { rows: Vec::new(), notes: Vec::new() };

    let (mut doc, gen_ms) = timed(|| synthetic_manga_page(page));
    let (tiles, unique) = (tile_count(&doc), unique_tiles(&doc));
    let raw = tiles as u64 * TILE;
    eprintln!("generated {tiles} tiles ({:.2} GB raw) in {gen_ms:.0} ms", raw as f64 / 1e9);

    // T1: UI snapshot.
    let mut snaps: Vec<f64> = (0..200).map(|_| timed(|| doc.snapshot()).1).collect();
    let (p50, max) = (percentile(&mut snaps, 0.5), percentile(&mut snaps, 1.0));
    r.row("T1", "UI `snapshot()`", "≤ 0.5 ms", format!("median {p50:.3} ms, max {max:.3} ms"), Some(p50 <= 0.5));

    // T13: first brush write after a snapshot copies the layer's map.
    let ids = rasters(&doc);
    let mut firsts = Vec::new();
    for (i, &id) in ids.iter().enumerate() {
        let snap = doc.snapshot();
        let c = doc.layer(id).unwrap().raster().unwrap().coords().next().unwrap();
        let ((), t) = timed(|| doc.paint_target(id).unwrap().0.get_mut_or_create(c)[0][0] = [i as u16, 0, 0, 1 << 15]);
        drop(snap);
        firsts.push(t);
    }
    let (p50, max) = (percentile(&mut firsts, 0.5), percentile(&mut firsts, 1.0));
    r.row("T13", "First brush write after a snapshot", "≤ 0.5 ms", format!("median {p50:.3} ms, max {max:.3} ms"), Some(max <= 0.5));

    // T2: first full save, encoding everything.
    let rec_dir = dir.join("recovery");
    let main = dir.join("main.arty");
    let mut session = Session::new(SessionId::random().unwrap(), Some(&rec_dir));
    let snap = doc.snapshot();
    let ((s, wall), extra) = arty_testkit::peak_bytes_during(|| {
        timed(|| session.save_main(&snap, &ex, &main, false, &opts(Verify::Fast), &pool, &p).unwrap())
    });
    let main_len = file_len(&main);
    r.row(
        "T2",
        "First full save (encode everything)",
        "≤ 2.5 s; extra RAM ≤ 96 MiB",
        format!("{:.2} s; extra RAM {:.0} MiB", wall / 1000.0, extra as f64 / MIB),
        Some(wall <= 2500.0 && extra as f64 <= 96.0 * MIB),
    );
    r.note(format!("T2 {}", stats_line(&s)));

    // Seed the recovery file (copies every blob from the main file).
    let (s, wall) = timed(|| session.autosave(&snap, &ex, snap.revision(), &pool, &p).unwrap());
    r.note(format!("First autosave after the save (recovery seeded by copy): {:.0} ms; {}", wall, stats_line(&s)));
    drop(snap);

    // T4: 500 changed tiles on one layer.
    // A line-art layer: every tile is distinct, so 500 edits are 500 blobs.
    let line_art = |id: &LayerId| LayerType::of_layer(&doc.layer(*id).unwrap().props.name) == Some(LayerType::LineArt);
    let layer = ids.iter().copied().find(|id| line_art(id) && doc.layer(*id).unwrap().raster().unwrap().len() >= 500).unwrap();
    let mut coords: Vec<TileCoord> = doc.layer(layer).unwrap().raster().unwrap().coords().collect();
    coords.sort_by_key(|c| (c.y, c.x));
    let grid = doc.paint_target(layer).unwrap().0;
    for (k, &c) in coords.iter().take(500).enumerate() {
        grid.get_mut_or_create(c)[k % 64][(k * 7) % 64] = [1, 2, 3, 1 << 15];
    }
    let snap = doc.snapshot();
    let (s, wall) = timed(|| session.autosave(&snap, &ex, snap.revision(), &pool, &p).unwrap());
    r.row(
        "T4",
        "Autosave, 500 changed tiles on 1 layer",
        "≤ 80 ms incl. 2 fsyncs",
        format!("{wall:.1} ms; {} blobs, {} table, {:.2} MiB appended", s.encoded, s.tables_written, s.bytes_written as f64 / MIB),
        Some(wall <= 80.0 && s.encoded == 500 && s.tables_written == 1),
    );
    r.note(format!("T4 {}", stats_line(&s)));

    // T5 / T12: nothing changed.
    let snap = doc.snapshot();
    let (s, wall) = timed(|| session.autosave(&snap, &ex, snap.revision(), &pool, &p).unwrap());
    r.row(
        "T5",
        "No-change autosave",
        "0 bytes, ≤ 15 ms",
        format!("{} bytes, {wall:.1} ms", s.bytes_written),
        Some(s.unchanged && s.bytes_written == 0 && wall <= 15.0),
    );
    r.row(
        "T12",
        "TileCache update per autosave",
        "≤ 15 ms",
        format!("{:.1} ms ({} tiles; the plan time of T5, which is only the cache update)", s.ms_plan, s.tiles),
        Some(s.ms_plan <= 15.0),
    );

    // T3 / T14 fast: Ctrl+S after the autosave copies everything.
    let (s, wall) = timed(|| session.save_main(&snap, &ex, &main, false, &opts(Verify::Fast), &pool, &p).unwrap());
    r.row(
        "T3",
        "Ctrl+S after an autosave (copy path)",
        "≤ 1.0 s; encoded == 0",
        format!("{:.2} s; encoded {}, copied {}", wall / 1000.0, s.encoded, s.copied),
        Some(wall <= 1000.0 && s.encoded == 0),
    );
    r.note(format!("T3 {} (wall includes the clean recovery commit)", stats_line(&s)));
    let fast = s.ms_verify;
    let full_path = dir.join("main_full_verify.arty");
    let (s, _) = timed(|| session.save_main(&snap, &ex, &full_path, false, &opts(Verify::Full), &pool, &p).unwrap());
    r.row(
        "T14",
        "Streaming verify Fast after a copy-save / Full",
        "≤ 0.3 s / ≤ 0.8 s",
        format!("{:.2} s / {:.2} s", fast / 1000.0, s.ms_verify / 1000.0),
        Some(fast <= 300.0 && s.ms_verify <= 800.0),
    );
    let _ = std::fs::remove_file(&full_path);

    // T7: recovery compaction.
    let rec_path = session.recovery_path().unwrap();
    let before = file_len(&rec_path);
    let (s, wall) = timed(|| session.compact(&snap, &ex, snap.revision(), &pool, &p).unwrap());
    r.row(
        "T7",
        "Recovery compaction",
        "≤ 1.5 s (~400 MB)",
        format!("{:.2} s ({:.0} MB → {:.0} MB)", wall / 1000.0, before as f64 / 1e6, file_len(&rec_path) as f64 / 1e6),
        Some(wall <= 1500.0),
    );
    r.note(format!("T7 {}", stats_line(&s)));
    drop(snap);
    session.close(true).unwrap();

    // T6: load.
    let main_len_now = file_len(&main);
    let ((loaded, wall, split), extra) = arty_testkit::peak_bytes_during(|| {
        load_phases(&p, || arty_io::load(&main, &LoadOptions::default(), &pool, &p).unwrap())
    });
    let pixels = unique_tiles(&loaded.doc) as u64 * TILE;
    let beyond = extra as f64 - pixels as f64;
    r.row(
        "T6",
        "Load to a usable Document",
        "≤ 1.5 s; RAM beyond pixels ≤ 64 MiB",
        format!("{:.2} s; {:.0} MiB beyond {:.0} MiB of pixels", wall / 1000.0, beyond / MIB, pixels as f64 / MIB),
        Some(wall <= 1500.0 && beyond <= 64.0 * MIB),
    );
    r.note(format!("T6 {split}"));
    drop(loaded);

    // T8: file size.
    r.row(
        "T8",
        "File size",
        "≤ 0.30 × raw",
        format!("{:.3} × raw ({:.0} MB of {:.0} MB)", main_len_now as f64 / raw as f64, main_len_now as f64 / 1e6, raw as f64 / 1e6),
        Some((main_len_now as f64) <= 0.30 * raw as f64),
    );
    codec_table(&doc, &pool, &mut r);

    // T9: UI frames while a full save runs in the background.
    let center = TileCoord::new(page.tiles_wide() / 2, page.tiles_high() / 2);
    let dirty: Vec<TileCoord> = (0..32).map(|i| TileCoord::new(center.x + i % 8, center.y + i / 8)).collect();
    let idle = AtomicBool::new(true);
    let mut base = frames_until(&doc, &dirty, &idle, 240);
    let done = AtomicBool::new(false);
    let bg_path = dir.join("background.arty");
    let snap = doc.snapshot();
    let mut during = std::thread::scope(|scope| {
        // Three full saves back to back, each encoding everything (a new
        // session has nothing to copy from), for enough frames to sample.
        let saver = scope.spawn(|| {
            for _ in 0..3 {
                let mut s = Session::new(SessionId::random().unwrap(), None);
                s.save_main(&snap, &ex, &bg_path, false, &opts(Verify::Fast), &pool, &Progress::default()).unwrap();
            }
            done.store(true, Ordering::Relaxed);
        });
        let frames = frames_until(&doc, &dirty, &done, 60);
        saver.join().unwrap();
        frames
    });
    drop(snap);
    let _ = std::fs::remove_file(&bg_path);
    let (b99, d99) = (percentile(&mut base, 0.99), percentile(&mut during, 0.99));
    r.row(
        "T9",
        "Frame-time p99 during a background full save",
        "≤ +2 ms over the baseline",
        format!("{d99:.2} ms vs {b99:.2} ms baseline ({:+.2} ms; {} frames)", d99 - b99, during.len()),
        Some(d99 - b99 <= 2.0),
    );
    r.note(format!(
        "T9 frame = composite 32 tiles of all {} layers on the global pool; p50 {:.2} ms baseline, \
         {:.2} ms during three back-to-back full saves (each encoding everything)",
        doc.layer_count(),
        percentile(&mut base, 0.5),
        percentile(&mut during, 0.5)
    ));

    // T11: legacy import of the same pixels (layers flattened into one
    // v1 list, as v1 files were).
    let v1_path = dir.join("legacy_v1.arty");
    let mut task = v1::SaveTask {
        canvas_width: doc.width(),
        canvas_height: doc.height(),
        layer_order: ids.iter().rev().map(|id| id.0).collect(),
        ..Default::default()
    };
    for &id in ids.iter().rev() {
        let l = doc.layer(id).unwrap();
        let mut m = v1::LayerMetadata::new(id.0, "Raster");
        m.name = l.props.name.clone();
        task.layers_meta.push(m);
        for (c, t) in l.raster().unwrap().iter() {
            task.tiles.push(v1::TileSaveData { layer_id: id.0, tx: c.x, ty: c.y, pixels: t.clone() });
        }
    }
    let (bytes, write_ms) = timed(|| v1::perform_save(&task));
    drop(task);
    std::fs::write(&v1_path, &bytes).unwrap();
    let v1_len = bytes.len();
    drop(bytes);
    let (imported, wall, split) = load_phases(&p, || arty_io::load(&v1_path, &LoadOptions::default(), &pool, &p).unwrap());
    r.row(
        "T11",
        "Legacy import of the same content",
        "≤ 3 s",
        format!("{:.2} s ({:.0} MB v1 file, {} tiles)", wall / 1000.0, v1_len as f64 / 1e6, tile_count(&imported.doc)),
        Some(wall <= 3000.0),
    );
    r.note(format!("T11 {split}; the v1 file was written (deflate in parallel) in {write_ms:.0} ms"));
    drop(imported);
    let _ = std::fs::remove_file(&v1_path);

    // T10: two simulated hours.
    if !a.skip_t10 {
        let t10_dir = dir.join("t10");
        let _ = std::fs::remove_dir_all(&t10_dir);
        std::fs::create_dir_all(&t10_dir).unwrap();
        let main = t10_dir.join("page.arty");
        let mut s = Session::new(SessionId::random().unwrap(), Some(&t10_dir.join("recovery")));
        let snap = doc.snapshot();
        s.save_main(&snap, &ex, &main, false, &opts(Verify::Fast), &pool, &p).unwrap();
        drop(snap);
        let rec_path = s.recovery_path().unwrap();
        let mut rng = 0x1234_5678_9ABC_DEF1u64;
        let (mut worst, mut written, mut single) = (0f64, 0u64, true);
        let t = Instant::now();
        for minute in 0..120u64 {
            let strokes = (minute + 1) * 1500 / 120 - minute * 1500 / 120;
            for _ in 0..strokes {
                let pick = (rng % ids.len() as u64) as usize;
                rng = rng.rotate_left(17) ^ 0x9E37_79B9;
                stroke(&mut doc, ids[pick], &mut rng, 400);
            }
            let snap = doc.snapshot();
            let st = s.autosave(&snap, &ex, snap.revision(), &pool, &p).unwrap();
            written += st.bytes_written;
            let bound = 2.0 * st.live_bytes as f64 + 64.0 * MIB;
            worst = worst.max(file_len(&rec_path) as f64 / bound);
            if minute % 10 == 9 {
                let st = s.save_main(&snap, &ex, &main, false, &opts(Verify::Fast), &pool, &p).unwrap();
                written += st.bytes_written;
                single &= arty_io::read_info(&main).unwrap().commit_seq == 1;
            }
        }
        let wall = t.elapsed().as_secs_f64();
        r.row(
            "T10",
            "Disk over 2 simulated hours (1500 strokes, autosave/60 s, Ctrl+S/10 min)",
            "recovery ≤ 2·live + 64 MiB; main single-commit",
            format!(
                "recovery peaked at {:.2} of the bound; main single-commit: {single}; {:.1} GB written in {wall:.0} s",
                worst,
                written as f64 / 1e9
            ),
            Some(worst <= 1.0 && single),
        );
        s.close(true).unwrap();
        let _ = std::fs::remove_dir_all(&t10_dir);
    }

    let _ = std::fs::remove_file(&main);

    let md = markdown(&a, page, tiles, unique, raw, gen_ms, main_len, &r);
    println!("{md}");
    if let Some(out) = &a.out {
        std::fs::write(out, md).unwrap();
        eprintln!("wrote {}", out.display());
    }
}

fn markdown(a: &Args, page: Page, tiles: usize, unique: usize, raw: u64, gen_ms: f64, main_len: u64, r: &Report) -> String {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let cpu = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown CPU".into());
    let mut s = String::new();
    s += &format!("### Preset `{}`: {}×{} px at {} dpi\n\n", a.preset, page.width, page.height, page.dpi);
    s += &format!(
        "- Document: 30 rasters + 4 folders, {tiles} tiles ({unique} allocations), {:.2} GB raw; generated in {:.1} s.\n",
        raw as f64 / 1e9,
        gen_ms / 1000.0
    );
    s += &format!("- First full save: {:.0} MB main file.\n", main_len as f64 / 1e6);
    s += &format!("- Machine: {cpu}, {threads} hardware threads; io pool {} threads below normal priority.\n", a.threads);
    s += &format!(
        "- Reproduce: `cargo run --release -p arty-io --example io_bench -- --preset {} --threads {} --out <file>`\n\n",
        a.preset, a.threads
    );
    s += "| # | Metric | Target | Result | |\n|---|---|---|---|---|\n";
    for row in &r.rows {
        let mark = match row.pass {
            Some(true) => "pass",
            Some(false) => "**miss**",
            None => "",
        };
        s += &format!("| {} | {} | {} | {} | {} |\n", row.id, row.metric, row.target, row.result, mark);
    }
    s += "\nDetails:\n\n";
    let mut in_table = false;
    for n in &r.notes {
        let table = n.starts_with('|');
        if table && !in_table {
            s += "\n";
        }
        if !table && in_table {
            s += "\n";
        }
        in_table = table;
        if table {
            s += &format!("{n}\n");
        } else {
            s += &format!("- {n}\n");
        }
    }
    s
}
