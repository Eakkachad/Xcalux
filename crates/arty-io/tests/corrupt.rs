//! R5: seeded mutations of a valid two-commit v2 file and of a v1 file
//! never panic, and a load never needs more than 64 MiB of heap for a
//! ~200 KB input with `max_decoded_bytes = 8 MiB`. 20k iterations each;
//! the only test in this binary that runs by default, because peak
//! tracking is process-wide.
//!
//! `cargo test -p arty-io --test corrupt -- --ignored` runs 200k
//! iterations of each.

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
    let folder = a.add_folder().unwrap();
    let inner = a.add_raster_layer().unwrap();
    a.move_layer(inner, Some(folder), 0);
    let top = a.add_raster_layer().unwrap();
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

/// A v1 file as `legacy/src/save.rs` writes it: a folder with children,
/// rasters, a vector layer with strokes, and a mix of tile shapes.
#[cfg(feature = "legacy")]
fn base_v1() -> Vec<u8> {
    use common::v1::{LayerMetadata, SaveTask, TileSaveData, VectorControlPoint, VectorStroke, perform_save};
    let mut rng = Rng(0xBEEF_0001);
    let mut folder = LayerMetadata::new(2, "Folder");
    folder.folder_child_ids = vec![3, 4];
    let mut vector = LayerMetadata::new(5, "Vector");
    let p = VectorControlPoint { x: 10.0, y: 20.0, pressure: 0.5, tilt_x: 0.0, tilt_y: 0.0 };
    vector.vector_strokes =
        Some((0..40).map(|i| VectorStroke { control_points: vec![p; 6], brush_preset_id: i, color: [0.0; 3], width: 3.0 }).collect());
    let mut shade = LayerMetadata::new(1, "Raster");
    shade.blend_mode = "Shade".into();
    let mut task = SaveTask {
        canvas_width: 500,
        canvas_height: 400,
        layer_order: vec![5, 2, 1],
        layers_meta: vec![vector, folder, LayerMetadata::new(3, "Raster"), LayerMetadata::new(4, "Raster"), shade],
        tiles: Vec::new(),
    };
    let kinds = [(1, [2, 3, 1, 0, 6].as_slice()), (3, &[4, 5, 3]), (4, &[6, 4, 0, 3]), (5, &[4, 1])];
    for (layer_id, kinds) in kinds {
        for (x, &kind) in kinds.iter().enumerate() {
            let pixels = tile(&mut rng, kind);
            task.tiles.push(TileSaveData { layer_id, tx: x as i32 - 1, ty: (x % 3) as i32, pixels });
        }
    }
    perform_save(&task)
}

/// One random mutation of a v1 file, aimed at its regions: header
/// offsets, deflate data, JSON, and directory fields.
#[cfg(feature = "legacy")]
fn mutate_v1(f: &mut Vec<u8>, (json_off, dir_off): (usize, usize), rng: &mut Rng) {
    let entries = (f.len().saturating_sub(dir_off) / 24).max(1);
    let pick = |rng: &mut Rng, lo: usize, hi: usize| lo + rng.below((hi.saturating_sub(lo)).max(1) as u64) as usize;
    match rng.below(9) {
        0 => {
            let i = rng.below(f.len() as u64) as usize;
            f[i] ^= 1 << rng.below(8);
        }
        1 => f.truncate(rng.below(f.len() as u64 + 1) as usize),
        2 => set_field(f, rng.pick(&[8, 16]), 8, rng),
        3 => {
            // Inside the tile data.
            let i = pick(rng, 24, json_off);
            if let Some(b) = f.get_mut(i) {
                *b = rng.next() as u8;
            }
        }
        4 | 5 => {
            // Inside the JSON: any byte, or a digit made huge or odd.
            let i = pick(rng, json_off, dir_off);
            let new = rng.pick(b"9-.e[{\"\0n");
            if let Some(b) = f.get_mut(i) {
                *b = if rng.chance(1, 2) { new } else { rng.next() as u8 };
            }
        }
        6 | 7 => {
            // A directory field: layer, tx, ty, offset or csize.
            let at = dir_off + 24 * rng.below(entries as u64) as usize;
            let (off, width) = rng.pick(&[(0, 4), (4, 4), (8, 4), (12, 8), (20, 4)]);
            set_field(f, at + off, width, rng);
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

/// Load `iterations` mutations of `base` with every option set, checking
/// the peak heap of each load.
fn run(name: &str, base: &[u8], iterations: u64, seed: u64, mutate: impl Fn(&mut Vec<u8>, &mut Rng)) {
    let pool = pool();
    assert!(base.len() <= 220 << 10, "{name}: base file is {} bytes", base.len());
    let limits = LoadLimits { max_decoded_bytes: 8 << 20, ..Default::default() };
    let options = [
        LoadOptions { limits, ..Default::default() },
        LoadOptions { limits, salvage: true, ..Default::default() },
        LoadOptions { limits, fallback_to_previous: true, salvage: true, ..Default::default() },
    ];
    // Sanity: the clean file loads.
    read_with(base, &options[0], &pool).unwrap();

    let mut rng = Rng(seed);
    let (mut ok, mut peak_max) = (0u32, 0usize);
    for i in 0..iterations {
        let mut f = base.to_vec();
        for _ in 0..1 + rng.below(3) {
            if f.is_empty() {
                break;
            }
            mutate(&mut f, &mut rng);
        }
        let o = &options[(i % 3) as usize];
        let (r, peak) = arty_testkit::peak_bytes_during(|| read_with(&f, o, &pool).map(drop));
        assert!(peak <= PEAK_LIMIT, "{name} iteration {i}: peak {peak} bytes");
        peak_max = peak_max.max(peak);
        ok += u32::from(r.is_ok());
    }
    // Some mutations hit only ignored bytes or salvageable tiles.
    assert!(ok > 0, "{name}: no mutated file loaded at all");
    eprintln!("{name}: {iterations} mutations, {ok} loaded, peak {} KiB", peak_max >> 10);
}

fn run_all(iterations: u64, seed: u64) {
    let v2 = base_file();
    let recs = records(&v2);
    run("v2", &v2, iterations, seed, |f, rng| mutate(f, &recs, rng));
    #[cfg(feature = "legacy")]
    {
        let v1 = base_v1();
        let layout = common::v1::offsets(&v1);
        run("v1", &v1, iterations, seed ^ 0x5555, |f, rng| mutate_v1(f, layout, rng));
    }
}

#[test]
fn mutated_files_never_panic_and_stay_bounded() {
    run_all(20_000, 0x9E37_79B9_7F4A_7C15);
}

#[test]
#[ignore = "long: 200k iterations of each file"]
fn mutated_files_never_panic_long() {
    run_all(200_000, 0xD1B5_4A32_D192_ED03);
}
