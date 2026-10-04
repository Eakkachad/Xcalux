//! Allocation gates for the tile codecs: after warm-up, decoding never
//! touches the heap and encoding allocates only inside lz4_flex.

use arty_core::tile::new_tile_box;
use arty_core::{TileCoord, TilePixels};
use arty_io::codec::{self, BlobCodec, CodecScratch, TileClass};
use arty_io::format::{TileCodec, TileEntry};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

fn tiles() -> Vec<Box<TilePixels>> {
    let mut out = Vec::new();
    let mut x = 0x1234_5678_9ABC_DEF1u64;
    for kind in 0..4 {
        let mut t = new_tile_box();
        for (i, px) in t.as_flattened_mut().iter_mut().enumerate() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let v = (i as u16).wrapping_mul(37) & 0x7FFF;
            *px = match kind {
                0 => [x as u16, (x >> 16) as u16, (x >> 32) as u16, (x >> 48) as u16], // noise: RAW
                1 => [v / 2, v / 3, v / 4, v],
                2 => [0, 0, 0, if x.is_multiple_of(7) { 0x8000 } else { 0 }],
                _ => [7, 7, 7, 0x8000], // solid
            };
        }
        out.push(t);
    }
    out
}

#[test]
fn codec_is_allocation_free_after_warm_up() {
    let tiles = tiles();
    let mut scratch = CodecScratch::new();
    let mut dst = new_tile_box();
    // Encode once outside the gate to get the stored blobs.
    let mut stored: Vec<(TileEntry, Vec<u8>)> = Vec::new();
    for t in &tiles {
        for codec in [BlobCodec::Lz4Shuf, BlobCodec::Lz4Vdelta] {
            let entry = match codec::classify(t) {
                TileClass::Solid(v) => (TileEntry::solid(TileCoord::new(0, 0), v), Vec::new()),
                TileClass::General { raw_crc } => {
                    let enc = codec::encode_tile(t, raw_crc, codec, &mut scratch);
                    let e = TileEntry {
                        coord: TileCoord::new(0, 0),
                        codec: enc.codec,
                        stored_len: enc.bytes.len() as u32,
                        raw_crc,
                        stored_crc: enc.stored_crc,
                        offset: 64,
                    };
                    (e, enc.bytes.to_vec())
                }
            };
            stored.push(entry);
        }
    }
    let codecs: Vec<TileCodec> = stored.iter().map(|(e, _)| e.codec).collect();
    for c in [TileCodec::Raw, TileCodec::Lz4Shuf, TileCodec::Lz4Vdelta, TileCodec::Solid] {
        assert!(codecs.contains(&c), "{c:?} covered");
    }
    // Warm-up decode.
    for (e, b) in &stored {
        codec::decode_tile(e, b, &mut dst, &mut scratch).unwrap();
    }

    let n = arty_testkit::count_allocs(|| {
        for _ in 0..3 {
            for (e, b) in &stored {
                codec::decode_tile(e, b, &mut dst, &mut scratch).unwrap();
            }
        }
    });
    assert_eq!(n, 0, "decode allocated {n} times");

    let n = arty_testkit::count_allocs(|| {
        for t in &tiles {
            let _ = codec::classify(t);
        }
    });
    assert_eq!(n, 0, "classify allocated {n} times");

    // lz4_flex 0.11 builds its match table per `compress_into` call (one
    // allocation, no API to reuse it); our code must add nothing to that.
    let general: Vec<_> = tiles.iter().filter(|t| matches!(codec::classify(t), TileClass::General { .. })).collect();
    let n = arty_testkit::count_allocs(|| {
        for t in &general {
            let TileClass::General { raw_crc } = codec::classify(t) else { continue };
            for codec in [BlobCodec::Lz4Shuf, BlobCodec::Lz4Vdelta] {
                std::hint::black_box(codec::encode_tile(t, raw_crc, codec, &mut scratch));
            }
        }
    });
    let encodes = general.len() * 2;
    assert!(n <= encodes, "encode allocated {n} times for {encodes} tiles");
}
