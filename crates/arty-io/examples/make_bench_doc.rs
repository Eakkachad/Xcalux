//! Writes the synthetic gate documents of plans/bench B013 as .arty files,
//! for `ARTY_BENCH_OPEN` and the low-end gates (LG2–LG5):
//!
//! cargo run --release -p arty-io --example make_bench_doc -- --dir D:/bench_docs
//!
//! Options: `--preset NAME` (repeatable; default `a4-350-15l` and
//! `b4-350-30l`), `--dir DIR` (default `target/bench_docs`).
//!
//! | Preset | Page | Raster layers (line art / flat / tone / gradient) |
//! |---|---|---|
//! | `a4-350-15l` | A4 350 dpi, 2894×4093 | 15 (5 / 4 / 3 / 3), the gates' target page |
//! | `a4-350-30l` | A4 350 dpi | 30 (10 / 8 / 6 / 6) |
//! | `b4-350-30l` | B4 350 dpi, 3542×5016 | 30, the `io_bench --preset b4-350` page |
//! | `b4-600-30l` | B4 600 dpi, 6071×8598 | 30, the B002 reference page (~3.3 GiB in RAM) |
//!
//! Content is `arty_testkit::synthetic_manga_page_layers`: each type keeps
//! the reference page's tile coverage (35 / 40 / 50 / 20%), so a page holds
//! about 28% of "every layer full". The files are the same for the same
//! preset (fixed uuid and commit time). Each file is opened again to check it.

use std::path::PathBuf;
use std::time::Instant;

use arty_io::{LoadOptions, Progress, SaveExtras, SaveOptions, Session, SessionId, Verify, load};
use arty_testkit::{Page, synthetic_manga_page_layers};

const MIB: f64 = (1u64 << 20) as f64;
const PRESETS: &[(&str, Page, u32)] = &[
    ("a4-350-15l", Page::A4_350, 15),
    ("a4-350-30l", Page::A4_350, 30),
    ("b4-350-30l", Page::B4_350, 30),
    ("b4-600-30l", Page::B4_600, 30),
];

fn main() {
    let mut names = Vec::new();
    let mut dir = PathBuf::from("target/bench_docs");
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--preset" => names.push(it.next().expect("--preset NAME")),
            "--dir" => dir = it.next().expect("--dir DIR").into(),
            other => panic!("unknown argument {other}"),
        }
    }
    if names.is_empty() {
        names = vec!["a4-350-15l".into(), "b4-350-30l".into()];
    }
    std::fs::create_dir_all(&dir).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().thread_name(|i| format!("arty-io-{i}")).build().unwrap();
    let p = Progress::default();
    for name in &names {
        let Some(&(_, page, rasters)) = PRESETS.iter().find(|(n, ..)| n == name) else {
            let all: Vec<&str> = PRESETS.iter().map(|(n, ..)| *n).collect();
            panic!("unknown preset {name} ({})", all.join(", "));
        };
        let t = Instant::now();
        let doc = synthetic_manga_page_layers(page, rasters);
        let gen_ms = t.elapsed().as_secs_f64() * 1000.0;
        let tiles: usize = doc
            .root()
            .iter()
            .filter_map(|&f| doc.layer(f)?.children())
            .flatten()
            .filter_map(|&id| doc.layer(id)?.raster().map(|g| g.len()))
            .sum();
        let path = dir.join(format!("{name}.arty"));
        let ex = SaveExtras { title: name.clone(), ..Default::default() };
        // Same preset, same bytes.
        let mut uuid = [0u8; 16];
        uuid[..name.len().min(16)].copy_from_slice(&name.as_bytes()[..name.len().min(16)]);
        let o = SaveOptions { verify: Verify::Full, now_ms: Some(1_790_000_000_000), uuid: Some(uuid) };
        let t = Instant::now();
        Session::new(SessionId::random().unwrap(), None).save_main(&doc, &ex, &path, true, &o, &pool, &p).unwrap();
        let save_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let back = load(&path, &LoadOptions::default(), &pool, &p).unwrap();
        let load_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert_eq!((back.doc.layer_count(), back.doc.pixel_bytes()), (doc.layer_count(), doc.pixel_bytes()), "{name} reopens");
        println!(
            "{name}: {} · {}×{} px at {} dpi · {} raster layers + 4 folders · {tiles} tiles · {:.1} MiB pixels in RAM · \
             file {:.1} MiB · generated {gen_ms:.0} ms, saved {save_ms:.0} ms, reopened {load_ms:.0} ms",
            path.display(),
            page.width,
            page.height,
            page.dpi,
            rasters,
            doc.pixel_bytes() as f64 / MIB,
            std::fs::metadata(&path).map_or(0, |m| m.len()) as f64 / MIB,
        );
    }
}
