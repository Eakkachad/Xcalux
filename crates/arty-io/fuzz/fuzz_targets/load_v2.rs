//! Whole-file v2 load from bytes: never panics, every allocation bounded.
#![no_main]

use std::sync::OnceLock;

use arty_io::{LoadLimits, LoadOptions, Progress};
use libfuzzer_sys::fuzz_target;

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap())
}

fuzz_target!(|data: &[u8]| {
    let limits = LoadLimits { max_decoded_bytes: 64 << 20, ..Default::default() };
    for (salvage, fallback_to_previous) in [(false, false), (true, true)] {
        let o = LoadOptions { limits, salvage, fallback_to_previous, ..Default::default() };
        let _ = arty_io::load_from(data, None, &o, pool(), &Progress::default());
    }
});
