//! One tile: the first 32 bytes are its table entry, the rest its stored
//! bytes.
#![no_main]

use arty_core::tile::new_tile_box;
use arty_io::codec::{CodecScratch, decode_tile};
use arty_io::format::{TILE_ENTRY_LEN, TileEntry};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((entry, stored)) = data.split_at_checked(TILE_ENTRY_LEN) else { return };
    // The table offset bounds the blob range: past every possible blob.
    let Ok(e) = TileEntry::decode(entry, u64::MAX) else { return };
    let (mut tile, mut scratch) = (new_tile_box(), CodecScratch::new());
    let _ = decode_tile(&e, stored, &mut tile, &mut scratch);
});
