//! v1 import from bytes. The v1 magic and version are prepended, so every
//! input reaches the importer.
#![no_main]

use std::sync::OnceLock;

use arty_io::{LoadLimits, LoadOptions, Progress};
use libfuzzer_sys::fuzz_target;

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap())
}

fuzz_target!(|data: &[u8]| {
    let mut file = b"ARTY\x01\0\0\0".to_vec();
    file.extend_from_slice(data);
    let limits = LoadLimits { max_decoded_bytes: 64 << 20, ..Default::default() };
    for salvage in [false, true] {
        let o = LoadOptions { limits, salvage, ..Default::default() };
        let _ = arty_io::import_v1(&file[..], None, &o, pool(), &Progress::default());
    }
});
