//! Tile codecs: classify, encode and decode single 64×64 tiles.
//!
//! - SOLID: every pixel equal; the value lives in the tile entry, no blob.
//! - LZ4_SHUF: byte planes (R.lo, R.hi, …, A.hi) then an lz4 block.
//! - LZ4_VDELTA: vertical delta + zigzag, low/high byte halves, lz4 block.
//! - RAW: the 32768 bytes as-is, used when lz4 does not shrink the tile.
//!
//! Decoding verifies both CRCs and then sanitizes: channels above `1<<15`
//! clamp to `1<<15`. Colour above alpha is legal (merge output) and kept.
//! All buffers live in a reusable [`CodecScratch`], so decoding allocates
//! nothing. Encoding allocates only inside `lz4_flex`, which builds its
//! 8 KiB match table per call and has no API to reuse one.

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

use arty_core::TilePixels;

use crate::error::IoError;
use crate::format::{TILE_BYTES, TileCodec, TileEntry, unpack_solid};

/// fix15 1.0, the largest valid channel value.
pub const ONE: u16 = 1 << 15;
/// Pixels per tile, and bytes per shuffle plane.
const PLANE: usize = TILE_BYTES / 8;
/// u16 values per tile row (64 pixels × RGBA).
const ROW: usize = 256;
/// Size of each byte half in VDELTA.
const HALF: usize = TILE_BYTES / 2;

/// The raw little-endian bytes of a tile.
#[inline]
pub fn tile_bytes(t: &TilePixels) -> &[u8] {
    bytemuck::bytes_of(t)
}

/// What a tile needs to be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileClass {
    /// Every pixel is this value (including present-but-empty `[0; 4]`).
    Solid([u16; 4]),
    General { raw_crc: u32 },
}

/// Classify a tile. Exits at the first pixel that differs from the first.
pub fn classify(t: &TilePixels) -> TileClass {
    let px = t.as_flattened();
    if let Some((&first, rest)) = px.split_first()
        && rest.iter().all(|&p| p == first)
    {
        return TileClass::Solid(first);
    }
    TileClass::General { raw_crc: crc32fast::hash(tile_bytes(t)) }
}

/// The lz4 transform used for general tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlobCodec {
    /// Writer default until bench B002 shows VDELTA is worth it.
    #[default]
    Lz4Shuf,
    Lz4Vdelta,
}

impl From<BlobCodec> for TileCodec {
    fn from(c: BlobCodec) -> Self {
        match c {
            BlobCodec::Lz4Shuf => TileCodec::Lz4Shuf,
            BlobCodec::Lz4Vdelta => TileCodec::Lz4Vdelta,
        }
    }
}

/// Per-thread buffers for encoding and decoding.
pub struct CodecScratch {
    /// Shuffled or delta-coded tile (exactly `TILE_BYTES`).
    planes: Vec<u8>,
    /// lz4 output, sized for the worst case.
    out: Vec<u8>,
}

impl Default for CodecScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl CodecScratch {
    pub fn new() -> Self {
        Self { planes: vec![0; TILE_BYTES], out: vec![0; lz4_flex::block::get_maximum_output_size(TILE_BYTES)] }
    }
}

/// A stored tile, borrowing either the scratch or the tile itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoded<'a> {
    pub codec: TileCodec,
    pub bytes: &'a [u8],
    pub stored_crc: u32,
}

/// Encode a general tile whose `raw_crc` came from [`classify`]. Falls back
/// to RAW when lz4 output would not be smaller than the raw tile.
pub fn encode_tile<'a>(t: &'a TilePixels, raw_crc: u32, codec: BlobCodec, s: &'a mut CodecScratch) -> Encoded<'a> {
    let raw = tile_bytes(t);
    debug_assert_eq!(crc32fast::hash(raw), raw_crc);
    match codec {
        BlobCodec::Lz4Shuf => shuffle(raw, &mut s.planes),
        BlobCodec::Lz4Vdelta => vdelta(t, &mut s.planes),
    }
    if let Ok(n) = lz4_flex::block::compress_into(&s.planes, &mut s.out)
        && n < TILE_BYTES
        && let Some(bytes) = s.out.get(..n)
    {
        return Encoded { codec: codec.into(), bytes, stored_crc: crc32fast::hash(bytes) };
    }
    Encoded { codec: TileCodec::Raw, bytes: raw, stored_crc: raw_crc }
}

/// Decode the tile of entry `e` from its `stored` bytes (empty for SOLID)
/// into `dst`, verify it and sanitize it. Returns how many channel values
/// were clamped.
pub fn decode_tile(e: &TileEntry, stored: &[u8], dst: &mut TilePixels, s: &mut CodecScratch) -> Result<u64, IoError> {
    let bad = |what| IoError::corrupt(what, e.offset);
    match e.codec {
        TileCodec::Solid => {
            let (v, clamped) = sanitize_pixel(unpack_solid(e.offset));
            dst.as_flattened_mut().fill(v);
            return Ok(clamped.saturating_mul(PLANE as u64));
        }
        TileCodec::Raw => {
            if stored.len() != TILE_BYTES {
                return Err(bad("tile stored length"));
            }
            if crc32fast::hash(stored) != e.raw_crc {
                return Err(bad("tile crc"));
            }
            bytemuck::bytes_of_mut(dst).copy_from_slice(stored);
        }
        TileCodec::Lz4Shuf | TileCodec::Lz4Vdelta => {
            if stored.len() != e.stored_len as usize {
                return Err(bad("tile stored length"));
            }
            if crc32fast::hash(stored) != e.stored_crc {
                return Err(bad("tile stored crc"));
            }
            // `checked-decode`: hostile input errors instead of panicking,
            // and output can never exceed the 32768-byte buffer.
            let n = lz4_flex::block::decompress_into(stored, &mut s.planes).map_err(|_| bad("tile lz4 data"))?;
            if n != TILE_BYTES {
                return Err(bad("tile decoded length"));
            }
            if e.codec == TileCodec::Lz4Shuf {
                unshuffle(&s.planes, bytemuck::bytes_of_mut(dst));
            } else {
                undelta(&s.planes, dst);
            }
            if crc32fast::hash(tile_bytes(dst)) != e.raw_crc {
                return Err(bad("tile crc"));
            }
        }
    }
    Ok(sanitize(dst))
}

/// Clamp channels above `1<<15` (only those: colour above alpha stays).
/// Returns how many values were clamped.
pub fn sanitize(t: &mut TilePixels) -> u64 {
    let mut clamped = 0u64;
    for c in t.as_flattened_mut().as_flattened_mut() {
        if *c > ONE {
            *c = ONE;
            clamped = clamped.saturating_add(1);
        }
    }
    clamped
}

/// [`sanitize`] for one pixel (SOLID values, paper colour).
pub fn sanitize_pixel(px: [u16; 4]) -> ([u16; 4], u64) {
    let clamped = px.iter().filter(|&&c| c > ONE).count() as u64;
    (px.map(|c| c.min(ONE)), clamped)
}

/// `out[p*4096 + i] = raw[8*i + p]`.
fn shuffle(raw: &[u8], out: &mut [u8]) {
    for (p, plane) in out.chunks_exact_mut(PLANE).enumerate() {
        for (d, &b) in plane.iter_mut().zip(raw.iter().skip(p).step_by(8)) {
            *d = b;
        }
    }
}

fn unshuffle(planes: &[u8], raw: &mut [u8]) {
    for (p, plane) in planes.chunks_exact(PLANE).enumerate() {
        for (d, &b) in raw.iter_mut().skip(p).step_by(8).zip(plane) {
            *d = b;
        }
    }
}

#[inline]
fn zigzag(d: u16) -> u16 {
    let s = d as i16;
    (s.wrapping_shl(1) ^ s.wrapping_shr(15)) as u16
}

#[inline]
fn unzigzag(z: u16) -> u16 {
    z.wrapping_shr(1) ^ 0u16.wrapping_sub(z & 1)
}

/// Predict each value from the one above (row 0: from the same channel one
/// pixel left, 0 for the first pixel), zigzag the wrapped difference and
/// split it into low bytes `[0..16384)` and high bytes `[16384..32768)`.
fn vdelta(t: &TilePixels, out: &mut [u8]) {
    let Some((lo, hi)) = out.split_at_mut_checked(HALF) else { return };
    let mut dst = lo.iter_mut().zip(hi.iter_mut());
    let mut put = |d: u16| {
        if let Some((l, h)) = dst.next() {
            [*l, *h] = zigzag(d).to_le_bytes();
        }
    };
    let mut above: Option<&[u16]> = None;
    for row in t.as_flattened().as_flattened().chunks_exact(ROW) {
        match above {
            Some(up) => row.iter().zip(up).for_each(|(&x, &p)| put(x.wrapping_sub(p))),
            None => row.iter().zip([0u16; 4].iter().chain(row)).for_each(|(&x, &p)| put(x.wrapping_sub(p))),
        }
        above = Some(row);
    }
}

/// Inverse of [`vdelta`]: prefix-add per row.
fn undelta(planes: &[u8], t: &mut TilePixels) {
    let Some((lo, hi)) = planes.split_at_checked(HALF) else { return };
    let mut deltas = lo.iter().zip(hi).map(|(&l, &h)| unzigzag(u16::from_le_bytes([l, h])));
    let mut above: Option<&[u16]> = None;
    for row in t.as_flattened_mut().as_flattened_mut().chunks_exact_mut(ROW) {
        match above {
            Some(up) => {
                for ((x, &p), d) in row.iter_mut().zip(up).zip(&mut deltas) {
                    *x = p.wrapping_add(d);
                }
            }
            None => {
                let mut left = [0u16; 4];
                for px in row.chunks_exact_mut(4) {
                    for ((x, l), d) in px.iter_mut().zip(&mut left).zip(&mut deltas) {
                        *x = l.wrapping_add(d);
                        *l = *x;
                    }
                }
            }
        }
        let done: &[u16] = row;
        above = Some(done);
    }
}

#[cfg(test)]
mod tests {
    use arty_core::TileCoord;
    use arty_core::tile::new_tile_box;

    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    /// A mix of tile shapes: noise (full u16 range), smooth gradients,
    /// sparse line art and premultiplied colour, by `kind`.
    fn tile(rng: &mut Rng, kind: u64) -> Box<TilePixels> {
        let mut t = new_tile_box();
        let base = rng.next();
        for (i, px) in t.as_flattened_mut().iter_mut().enumerate() {
            let (x, y) = ((i % 64) as u64, (i / 64) as u64);
            *px = match kind % 4 {
                0 => {
                    let r = rng.next();
                    [r as u16, (r >> 16) as u16, (r >> 32) as u16, (r >> 48) as u16]
                }
                1 => {
                    let v = ((base & 0x3FF) + x * 97 + y * 211) as u16 & 0x7FFF;
                    [v / 2, v / 3, v / 4, v]
                }
                2 => {
                    if rng.next().is_multiple_of(9) { [0, 0, 0, ONE] } else { [0; 4] }
                }
                _ => {
                    // Soft alpha with colour 1 LSB above it (merge output).
                    let a = ((x * 512 + y * 3) as u16).min(ONE - 1);
                    [a + 1, a / 2, a, a]
                }
            };
        }
        t
    }

    fn entry(enc: &Encoded<'_>, raw_crc: u32) -> TileEntry {
        TileEntry {
            coord: TileCoord::new(0, 0),
            codec: enc.codec,
            stored_len: enc.bytes.len() as u32,
            raw_crc,
            stored_crc: enc.stored_crc,
            offset: 64,
        }
    }

    #[test]
    fn transforms_are_the_identity_over_10k_tiles() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut planes = vec![0u8; TILE_BYTES];
        let mut back = new_tile_box();
        for k in 0..10_000 {
            let t = tile(&mut rng, k);
            shuffle(tile_bytes(&t), &mut planes);
            unshuffle(&planes, bytemuck::bytes_of_mut(&mut *back));
            assert!(*back == *t, "shuffle tile {k}");
            back.as_flattened_mut().fill([1; 4]);
            vdelta(&t, &mut planes);
            undelta(&planes, &mut back);
            assert!(*back == *t, "vdelta tile {k}");
        }
        for v in [0u16, 1, 0x7FFF, 0x8000, 0xFFFF] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!([zigzag(0), zigzag(1), zigzag(0xFFFF), zigzag(2)], [0, 2, 1, 4]);
    }

    #[test]
    fn shuffle_layout_matches_spec() {
        let mut t = new_tile_box();
        t[0][0] = [0x0201, 0x0403, 0x0605, 0x0807];
        t[0][1] = [0x1211, 0x1413, 0x1615, 0x1817];
        let mut planes = vec![0u8; TILE_BYTES];
        shuffle(tile_bytes(&t), &mut planes);
        for p in 0..8 {
            assert_eq!(planes[p * 4096], p as u8 + 1, "plane {p} pixel 0");
            assert_eq!(planes[p * 4096 + 1], p as u8 + 0x11, "plane {p} pixel 1");
        }
    }

    #[test]
    fn encode_decode_round_trips() {
        let mut rng = Rng(42);
        let mut s = CodecScratch::new();
        let mut out = new_tile_box();
        for k in 0..400 {
            let t = tile(&mut rng, k);
            let TileClass::General { raw_crc } = classify(&t) else { panic!("tile {k} is solid") };
            for codec in [BlobCodec::Lz4Shuf, BlobCodec::Lz4Vdelta] {
                let enc = encode_tile(&t, raw_crc, codec, &mut s);
                let e = entry(&enc, raw_crc);
                if k % 4 == 0 {
                    assert_eq!(e.codec, TileCodec::Raw, "noise does not compress");
                    assert_eq!(enc.bytes, tile_bytes(&t));
                } else {
                    assert_eq!(e.codec, codec.into(), "tile {k} compresses");
                }
                let stored = enc.bytes.to_vec();
                // Noise has channels above 1<<15, which decode clamps.
                let clamped = decode_tile(&e, &stored, &mut out, &mut s).unwrap();
                let mut want = t.clone();
                assert_eq!(clamped, sanitize(&mut want));
                assert!(*out == *want, "tile {k} {codec:?}");
            }
        }
    }

    #[test]
    fn classify_finds_solid_tiles() {
        let mut t = new_tile_box();
        assert_eq!(classify(&t), TileClass::Solid([0; 4]), "present-but-empty");
        t.as_flattened_mut().fill([1, 2, 3, ONE]);
        assert_eq!(classify(&t), TileClass::Solid([1, 2, 3, ONE]));
        t[63][63][0] = 9;
        assert_eq!(classify(&t), TileClass::General { raw_crc: crc32fast::hash(tile_bytes(&t)) });
        t[63][63][0] = 1;
        t[0][0][3] = 0;
        assert!(matches!(classify(&t), TileClass::General { .. }), "first pixel differs");

        let solid = TileEntry::solid(TileCoord::new(0, 0), [1, 2, 3, ONE]);
        let mut out = new_tile_box();
        assert_eq!(decode_tile(&solid, &[], &mut out, &mut CodecScratch::new()).unwrap(), 0);
        assert_eq!(classify(&out), TileClass::Solid([1, 2, 3, ONE]));
        let hot = TileEntry::solid(TileCoord::new(0, 0), [0, 0, 0, 0x9000]);
        assert_eq!(decode_tile(&hot, &[], &mut out, &mut CodecScratch::new()).unwrap(), 4096);
        assert_eq!(classify(&out), TileClass::Solid([0, 0, 0, ONE]));
    }

    #[test]
    fn sanitize_clamps_only_out_of_range_channels() {
        let mut t = new_tile_box();
        t[0][0] = [ONE, ONE + 1, 0xFFFF, 0x4000];
        t[1][1] = [0x7000, 0x6000, 0x5000, 0x1000]; // colour above alpha: legal
        let clamped = sanitize(&mut t);
        assert_eq!(clamped, 2);
        assert_eq!(t[0][0], [ONE, ONE, ONE, 0x4000]);
        assert_eq!(t[1][1], [0x7000, 0x6000, 0x5000, 0x1000]);
        assert_eq!(sanitize_pixel([0xFFFF, 0, ONE, 0x8001]), ([ONE, 0, ONE, ONE], 2));
    }

    #[test]
    fn decode_rejects_bad_blobs() {
        let mut rng = Rng(7);
        let t = tile(&mut rng, 1);
        let TileClass::General { raw_crc } = classify(&t) else { unreachable!() };
        let mut s = CodecScratch::new();
        let mut out = new_tile_box();
        let enc = encode_tile(&t, raw_crc, BlobCodec::Lz4Shuf, &mut s);
        let good = entry(&enc, raw_crc);
        let stored = enc.bytes.to_vec();
        let err = |e: &TileEntry, b: &[u8], s: &mut CodecScratch, out: &mut TilePixels| match decode_tile(e, b, out, s) {
            Err(IoError::Corrupt { what, .. }) => what,
            other => panic!("expected Corrupt, got {other:?}"),
        };

        // lz4 that decodes to too few, or too many, bytes.
        for len in [100, TILE_BYTES - 1, TILE_BYTES + 1, 40_000] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let block = lz4_flex::block::compress(&data);
            let e = TileEntry { stored_len: block.len() as u32, stored_crc: crc32fast::hash(&block), ..good };
            let what = err(&e, &block, &mut s, &mut out);
            assert!(what == "tile decoded length" || what == "tile lz4 data", "{len}: {what}");
        }
        // Hostile lz4 bytes with a valid stored CRC.
        let junk = [0xF0u8, 0xFF, 0xFF, 0xFF, 0x12, 0x34];
        let e = TileEntry { stored_len: junk.len() as u32, stored_crc: crc32fast::hash(&junk), ..good };
        assert_eq!(err(&e, &junk, &mut s, &mut out), "tile lz4 data");

        let mut flipped = stored.clone();
        flipped[3] ^= 1;
        assert_eq!(err(&good, &flipped, &mut s, &mut out), "tile stored crc");
        assert_eq!(err(&good, &stored[1..], &mut s, &mut out), "tile stored length");
        let wrong_raw = TileEntry { raw_crc: raw_crc ^ 1, ..good };
        assert_eq!(err(&wrong_raw, &stored, &mut s, &mut out), "tile crc");
        let raw = TileEntry { codec: TileCodec::Raw, stored_len: TILE_BYTES as u32, ..good };
        assert_eq!(err(&raw, &stored, &mut s, &mut out), "tile stored length");
        let mut raw_bytes = tile_bytes(&t).to_vec();
        decode_tile(&raw, &raw_bytes, &mut out, &mut s).unwrap();
        assert!(*out == *t);
        raw_bytes[0] ^= 1;
        assert_eq!(err(&raw, &raw_bytes, &mut s, &mut out), "tile crc");
        // The intact blob still decodes with the same scratch.
        decode_tile(&good, &stored, &mut out, &mut s).unwrap();
        assert!(*out == *t);
    }
}
