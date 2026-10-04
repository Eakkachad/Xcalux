//! Manifest payload decoding and section parsing.
#![no_main]

use arty_io::limits::MAX_LAYER_COUNT;
use arty_io::manifest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // As a raw section stream, and as a record payload (codec, raw_len, body).
    let _ = manifest::parse(data, 0, MAX_LAYER_COUNT);
    if let Ok(raw) = manifest::decode_payload(data, 0) {
        let _ = manifest::parse(&raw, 0, MAX_LAYER_COUNT);
    }
});
