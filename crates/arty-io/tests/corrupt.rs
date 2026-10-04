//! R5: seeded mutations of a valid two-commit file never panic, and a
//! load never needs more than 64 MiB of heap for a ~200 KB input with
//! `max_decoded_bytes = 8 MiB`. The only test in this binary that runs by
//! default, because peak tracking is process-wide.
//!
//! `cargo test -p arty-io --test corrupt -- --ignored` runs 200k
//! iterations.

mod common;

use arty_core::{Document, TileCoord};
use arty_io::{FileWriter, LoadLimits, LoadOptions, Progress, SaveExtras};
use common::*;

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

const PEAK_LIMIT: usize = 64 << 20;

/// A folder, three rasters and a mix of tile shapes, committed twice.
fn base_file() -> Vec<u8> {
    let pool = pool();
    let mut rng = Rng(0xC0FF_EE00);
    let mut a = Document::new(700, 500, 350);
    let bottom = a.active();
    let folder = a.add_folder();
    let inner = a.add_raster_layer();
    a.move_layer(inner, Some(folder), 0);
    let top = a.add_raster_layer();
    let kinds = [[2, 3, 1, 0, 6].as_slice(), &[4, 5, 3, 1], &[6, 4, 0, 5, 3]];
    for (id, kinds) in [bottom, inner, top].into_iter().zip(kinds) {
        let (g, _) = a.paint_target(id).unwrap();
        for (x, &kind) in kinds.iter().enumerate() {
            g.insert(TileCoord::new(x as i32 - 1, (x % 3) as i32), tile(&mut rng, kind));
        }
    }
    let mut b = a.snapshot();
    let (g, _) = b.paint_target(top).unwrap();
    g.insert(TileCoord::new(0, 0), tile(&mut rng, 4));
    g.insert(TileCoord::new(7, 7), tile(&mut rng, 3));

    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    for doc in [&a, &b] {
        w.commit(doc, &SaveExtras::default(), &meta(doc), &opts(), &pool, &Progress::default()).unwrap();
    }
    w.into_sink()
}

/// Recompute a record's payload and header CRCs after editing it, so the
/// parsers behind the CRC checks see the damage.
fn reseal(f: &mut [u8], at: usize) {
    let Some(h) = f.get(at..at + 24) else { return };
    let len = u64::from_le_bytes(h[8..16].try_into().unwrap());
    let Some(end) = usize::try_from(len).ok().and_then(|l| (at + 24).checked_add(l)).filter(|&e| e <= f.len()) else { return };
    if f[at + 4] != 1 {
        let crc = crc32fast::hash(&f[at + 24..end]);
        f[at + 16..at + 20].copy_from_slice(&crc.to_le_bytes());
    }
    let crc = crc32fast::hash(&f[at..at + 20]);
    f[at + 20..at + 24].copy_from_slice(&crc.to_le_bytes());
}

fn set_field(f: &mut [u8], at: usize, width: usize, rng: &mut Rng) {
    let Some(b) = f.get_mut(at..at + width) else { return };
    let mut v = [0u8; 8];
    v[..width].copy_from_slice(b);
    let old = u64::from_le_bytes(v);
    let max = if width == 8 { u64::MAX } else { (1u64 << (8 * width)) - 1 };
    let new = rng.pick(&[0, 1, max, old.wrapping_add(1) & max, old.wrapping_sub(1) & max, old.wrapping_mul(2) & max]);
    b.copy_from_slice(&new.to_le_bytes()[..width]);
}

/// One random mutation of `f`, using the record layout of the clean file.
fn mutate(f: &mut Vec<u8>, recs: &[(u64, u8, u64)], rng: &mut Rng) {
    let (at, kind, end) = recs[rng.below(recs.len() as u64) as usize];
    let (at, end) = (at as usize, end as usize);
    match rng.below(8) {
        0 => {
            let i = rng.below(f.len() as u64) as usize;
            f[i] ^= 1 << rng.below(8);
        }
        1 => {
            let i = rng.below(f.len() as u64) as usize;
            f[i] = rng.next() as u8;
        }
        2 => f.truncate(rng.below(f.len() as u64 + 1) as usize),
        3 => {
            // A header field: payload length, or a commit's offsets.
            match kind {
                3 => set_field(f, at + 24 + 8 * rng.below(3) as usize, 8, rng),
                _ => set_field(f, at + 8, 8, rng),
            }
            if rng.chance(1, 2) {
                reseal(f, at);
            }
        }
        4 | 5 => {
            // Inside a payload, CRCs fixed so the parsers see it.
            if end > at + 24 {
                let i = at + 24 + rng.below((end - at - 24) as u64) as usize;
                if rng.chance(1, 2) {
                    let width = rng.pick(&[1, 2, 4, 8]);
                    set_field(f, i, width, rng);
                } else if let Some(b) = f.get_mut(i) {
                    *b ^= 1 << rng.below(8);
                }
                reseal(f, at);
            }
        }
        6 => {
            // Splice a copy of one record over another position.
            let (src_at, _, src_end) = recs[rng.below(recs.len() as u64) as usize];
            let rec = f.get(src_at as usize..src_end as usize).map(<[u8]>::to_vec).unwrap_or_default();
            let to = rng.below(f.len() as u64) as usize;
            if rng.chance(1, 2) {
                f.splice(to..to, rec);
            } else {
                let n = rec.len().min(f.len() - to);
                f[to..to + n].copy_from_slice(&rec[..n]);
            }
        }
        _ => {
            // Cut out or duplicate a range.
            let a = rng.below(f.len() as u64) as usize;
            let b = (a + rng.below(4096) as usize).min(f.len());
            if rng.chance(1, 2) {
                f.drain(a..b);
            } else {
                let dup = f[a..b].to_vec();
                f.splice(b..b, dup);
            }
        }
    }
}

fn run(iterations: u64, seed: u64) {
    let pool = pool();
    let base = base_file();
    assert!(base.len() <= 220 << 10, "base file is {} bytes", base.len());
    let recs = records(&base);
    let limits = LoadLimits { max_decoded_bytes: 8 << 20, ..Default::default() };
    let options = [
        LoadOptions { limits, ..Default::default() },
        LoadOptions { limits, salvage: true, ..Default::default() },
        LoadOptions { limits, fallback_to_previous: true, salvage: true, ..Default::default() },
    ];
    // Sanity: the clean file loads.
    read_with(&base, &options[0], &pool).unwrap();

    let mut rng = Rng(seed);
    let (mut ok, mut peak_max) = (0u32, 0usize);
    for i in 0..iterations {
        let mut f = base.clone();
        for _ in 0..1 + rng.below(3) {
            if f.is_empty() {
                break;
            }
            mutate(&mut f, &recs, &mut rng);
        }
        let o = &options[(i % 3) as usize];
        let (r, peak) = arty_testkit::peak_bytes_during(|| read_with(&f, o, &pool).map(drop));
        assert!(peak <= PEAK_LIMIT, "iteration {i}: peak {peak} bytes");
        peak_max = peak_max.max(peak);
        ok += u32::from(r.is_ok());
    }
    // Some mutations hit only ignored bytes or salvageable tiles.
    assert!(ok > 0, "no mutated file loaded at all");
    eprintln!("{iterations} mutations: {ok} loaded, peak {} KiB", peak_max >> 10);
}

#[test]
fn mutated_files_never_panic_and_stay_bounded() {
    run(3000, 0x9E37_79B9_7F4A_7C15);
}

#[test]
#[ignore = "long: 200k iterations"]
fn mutated_files_never_panic_long() {
    run(200_000, 0xD1B5_4A32_D192_ED03);
}
