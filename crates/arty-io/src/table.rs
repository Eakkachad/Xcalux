//! TileTable records: one per raster layer with tiles, listing every tile
//! of the layer sorted by `(ty, tx)`.
//!
//! Payload: a 16-byte header (`layer_id`, `count`, `entry_size`, `codec`,
//! reserved, `raw_len`), then `count` entries, stored or as one lz4 block.

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

use crate::error::IoError;
use crate::format::{ByteReader, TILE_ENTRY_LEN, TileEntry};
use crate::limits::{MAX_ENTRY_SIZE, MAX_TABLE_ENTRIES, MIN_ENTRY_SIZE, lz4_max_raw};

pub const TABLE_HEADER_LEN: usize = 16;
pub const TABLE_CODEC_STORED: u8 = 0;
pub const TABLE_CODEC_LZ4: u8 = 1;

/// The fixed header of a TileTable payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableHeader {
    pub layer_id: u32,
    pub count: u32,
    pub entry_size: u16,
    pub codec: u8,
    pub raw_len: u32,
}

/// Largest payload a table of `count` entries can have (stored, with the
/// largest entry size). Readers reject longer records before reading them.
pub fn max_payload_len(count: u32) -> u64 {
    u64::from(count).saturating_mul(u64::from(MAX_ENTRY_SIZE)).saturating_add(TABLE_HEADER_LEN as u64)
}

/// The payload of a table for `entries`, which must be non-empty and
/// strictly ascending by `(ty, tx)`. Uses lz4 when that saves at least 10%.
pub fn encode(layer_id: u32, entries: &[TileEntry]) -> Vec<u8> {
    debug_assert!(!entries.is_empty() && entries.len() <= MAX_TABLE_ENTRIES as usize);
    debug_assert!(entries.windows(2).all(|w| matches!(w, [a, b] if key(a) < key(b))));
    let mut raw = Vec::with_capacity(entries.len().saturating_mul(TILE_ENTRY_LEN));
    for e in entries {
        raw.extend_from_slice(&e.encode());
    }
    let packed = lz4_flex::block::compress(&raw);
    // packed ≤ 0.9 · raw
    let (codec, body) = if packed.len().saturating_mul(10) <= raw.len().saturating_mul(9) {
        (TABLE_CODEC_LZ4, &packed)
    } else {
        (TABLE_CODEC_STORED, &raw)
    };
    let mut out = Vec::with_capacity(TABLE_HEADER_LEN.saturating_add(body.len()));
    out.extend_from_slice(&layer_id.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&(TILE_ENTRY_LEN as u16).to_le_bytes());
    out.extend_from_slice(&[codec, 0]);
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

fn key(e: &TileEntry) -> (i32, i32) {
    (e.coord.y, e.coord.x)
}

/// Parse the header of a table payload and check its sizes against each
/// other (not yet against the layer that references it).
pub fn decode_header(payload: &[u8], at: u64) -> Result<TableHeader, IoError> {
    let bad = |what| IoError::corrupt(what, at);
    let mut r = ByteReader::new(payload, "truncated tile table", at);
    let layer_id = r.u32()?;
    let count = r.u32()?;
    let entry_size = r.u16()?;
    let codec = r.u8()?;
    r.skip(1)?;
    let raw_len = r.u32()?;
    let h = TableHeader { layer_id, count, entry_size, codec, raw_len };
    if !(1..=MAX_TABLE_ENTRIES).contains(&count) {
        return Err(bad("tile table count"));
    }
    if !(MIN_ENTRY_SIZE..=MAX_ENTRY_SIZE).contains(&entry_size) {
        return Err(bad("tile table entry size"));
    }
    if u64::from(raw_len) != u64::from(count).saturating_mul(u64::from(entry_size)) {
        return Err(bad("tile table length"));
    }
    let stored = r.remaining() as u64;
    match codec {
        TABLE_CODEC_STORED if stored == u64::from(raw_len) => {}
        TABLE_CODEC_LZ4 if stored > 0 && u64::from(raw_len) <= lz4_max_raw(stored) => {}
        TABLE_CODEC_STORED | TABLE_CODEC_LZ4 => return Err(bad("tile table length")),
        _ => return Err(bad("tile table codec")),
    }
    Ok(h)
}

/// Parse a whole table payload read from file offset `at` (the record
/// header's offset), checking that it belongs to `layer_id`, holds
/// `count` entries, and that every entry is valid and in strict order.
pub fn parse(payload: &[u8], at: u64, layer_id: u32, count: u32) -> Result<Vec<TileEntry>, IoError> {
    let h = decode_header(payload, at)?;
    if h.layer_id != layer_id || h.count != count {
        return Err(IoError::corrupt("tile table does not match its layer", at));
    }
    let body = payload.get(TABLE_HEADER_LEN..).unwrap_or_default();
    let unpacked;
    let raw: &[u8] = match h.codec {
        TABLE_CODEC_LZ4 => {
            // Sized by the checked header (≤ 255 · stored + 64).
            let mut buf = vec![0u8; h.raw_len as usize];
            let n = lz4_flex::block::decompress_into(body, &mut buf)
                .map_err(|_| IoError::corrupt("tile table lz4 data", at))?;
            if n != buf.len() {
                return Err(IoError::corrupt("tile table decoded length", at));
            }
            unpacked = buf;
            &unpacked
        }
        _ => body,
    };
    let mut entries = Vec::with_capacity(h.count as usize);
    let mut prev: Option<(i32, i32)> = None;
    for chunk in raw.chunks_exact(h.entry_size.into()) {
        let e = TileEntry::decode(chunk, at)?;
        if prev.is_some_and(|p| p >= key(&e)) {
            return Err(IoError::corrupt("tile table order", at));
        }
        prev = Some(key(&e));
        entries.push(e);
    }
    if entries.len() != h.count as usize {
        return Err(IoError::corrupt("tile table length", at));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use arty_core::TileCoord;

    use super::*;
    use crate::format::{TileCodec, TileEntry};

    fn blob(x: i32, y: i32, offset: u64) -> TileEntry {
        TileEntry {
            coord: TileCoord::new(x, y),
            codec: TileCodec::Lz4Shuf,
            stored_len: 100,
            raw_crc: 1,
            stored_crc: 2,
            offset,
        }
    }

    fn sample(n: i32) -> Vec<TileEntry> {
        (0..n).map(|i| if i % 3 == 0 { TileEntry::solid(TileCoord::new(i, -i), [0; 4]) } else { blob(i, -i, 64) }).rev().collect()
    }

    #[test]
    fn round_trips_stored_and_lz4() {
        let one = [blob(-5, 7, 64)];
        let p = encode(9, &one);
        assert_eq!(p[10], TABLE_CODEC_STORED, "a single entry does not compress");
        assert_eq!(parse(&p, 1000, 9, 1).unwrap(), one);

        let many = sample(500);
        let p = encode(3, &many);
        assert_eq!(p[10], TABLE_CODEC_LZ4);
        assert!(p.len() < 500 * 32 / 2);
        assert_eq!(parse(&p, 1_000_000, 3, 500).unwrap(), many);
        assert!(parse(&p, 1_000_000, 4, 500).is_err(), "wrong layer");
        assert!(parse(&p, 1_000_000, 3, 499).is_err(), "wrong count");
    }

    #[test]
    fn longer_entries_load() {
        // A later minor with 48-byte entries: the first 32 bytes are read.
        let entries = sample(20);
        let mut p = Vec::new();
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&20u32.to_le_bytes());
        p.extend_from_slice(&48u16.to_le_bytes());
        p.extend_from_slice(&[TABLE_CODEC_STORED, 0]);
        p.extend_from_slice(&(20u32 * 48).to_le_bytes());
        for e in &entries {
            p.extend_from_slice(&e.encode());
            p.extend_from_slice(&[0xAB; 16]);
        }
        assert_eq!(parse(&p, 100_000, 1, 20).unwrap(), entries);
    }

    /// A stored-codec payload, whatever its order or compressibility.
    fn stored(layer_id: u32, entries: &[TileEntry]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&layer_id.to_le_bytes());
        p.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        p.extend_from_slice(&32u16.to_le_bytes());
        p.extend_from_slice(&[TABLE_CODEC_STORED, 0]);
        p.extend_from_slice(&(entries.len() as u32 * 32).to_le_bytes());
        for e in entries {
            p.extend_from_slice(&e.encode());
        }
        p
    }

    #[test]
    fn rejects_bad_tables() {
        let good = stored(1, &[blob(0, 0, 64), blob(1, 0, 64)]);
        let with = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut p = good.clone();
            f(&mut p);
            parse(&p, 100_000, 1, 2)
        };
        assert!(with(&|_| {}).is_ok());
        assert!(with(&|p| p[4..8].copy_from_slice(&0u32.to_le_bytes())).is_err(), "count 0");
        assert!(with(&|p| p[4..8].copy_from_slice(&(MAX_TABLE_ENTRIES + 1).to_le_bytes())).is_err());
        assert!(with(&|p| p[8..10].copy_from_slice(&31u16.to_le_bytes())).is_err(), "entry size");
        assert!(with(&|p| p[8..10].copy_from_slice(&257u16.to_le_bytes())).is_err());
        assert!(with(&|p| p[10] = 2).is_err(), "codec");
        assert!(with(&|p| p[12..16].copy_from_slice(&65u32.to_le_bytes())).is_err(), "raw_len");
        assert!(with(&|p| p.truncate(p.len() - 1)).is_err(), "short body");
        assert!(with(&|p| p.push(0)).is_err(), "long body");
        let order = stored(1, &[blob(1, 0, 64), blob(0, 0, 64)]);
        assert!(matches!(parse(&order, 100_000, 1, 2), Err(IoError::Corrupt { what: "tile table order", .. })));
        let dup = stored(1, &[blob(1, 0, 64), blob(1, 0, 64)]);
        assert!(matches!(parse(&dup, 100_000, 1, 2), Err(IoError::Corrupt { what: "tile table order", .. })));
        // Rows sort before columns.
        assert!(parse(&stored(1, &[blob(5, 0, 64), blob(0, 1, 64)]), 100_000, 1, 2).is_ok());
        // A blob that ends past the table record.
        assert!(parse(&encode(1, &[blob(0, 0, 64)]), 100, 1, 1).is_err());
    }

    #[test]
    fn lz4_bomb_is_rejected_before_allocating() {
        // A count of 1M with a 10-byte lz4 body claims 32 MiB.
        let mut p = Vec::new();
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&MAX_TABLE_ENTRIES.to_le_bytes());
        p.extend_from_slice(&32u16.to_le_bytes());
        p.extend_from_slice(&[TABLE_CODEC_LZ4, 0]);
        p.extend_from_slice(&(MAX_TABLE_ENTRIES * 32).to_le_bytes());
        p.extend_from_slice(&[0; 10]);
        assert!(matches!(decode_header(&p, 0), Err(IoError::Corrupt { what: "tile table length", .. })));
        // Garbage lz4 that passes the size check fails cleanly.
        let mut p = encode(1, &sample(100));
        assert_eq!(p[10], TABLE_CODEC_LZ4);
        for b in p.iter_mut().skip(TABLE_HEADER_LEN) {
            *b = 0xFF;
        }
        assert!(parse(&p, 1_000_000, 1, 100).is_err());
        assert_eq!(max_payload_len(2), 16 + 512);
    }
}
