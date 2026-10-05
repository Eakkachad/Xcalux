//! The `SELM` section: the document's selection (SAFE_TO_COPY).
//!
//! An unreadable selection is dropped with a warning; it never fails the
//! load or makes it lossy.
//!
//! Body (little-endian):
//! ```text
//! u8 ver = 1, u8 codec (0 stored, 1 lz4), u16 0, u32 page_w, u32 page_h,
//! u32 n_tiles, u32 raw_len, payload (raw_len bytes once decoded):
//!   n_tiles × { i32 tx, i32 ty, u8 kind (0 FULL, 1 U8, 2 BIT) }, sorted by (ty, tx);
//!   then every U8 tile (4096 B, each row left-delta coded), in record order;
//!   then every BIT tile (512 B, 8 B per row, bit x of byte x/8 = column x), in record order.
//! ```
//! A partial tile holding only 0 and 255 is stored as BIT. An encoding over
//! [`MAX_SELM_BYTES`] is redone with partial tiles binarized (at 128); if
//! that is still too large the file is saved without a selection.

use std::borrow::Cow;
use std::sync::Arc;

use arty_core::selection::full_mask;
use arty_core::{MaskPixels, MaskView, Selection, TILE_SIZE, TileCoord};

use crate::error::LoadWarning;
use crate::limits::{MAX_SELM_BYTES, lz4_max_raw};

pub const SELM_VERSION: u8 = 1;
const CODEC_STORED: u8 = 0;
const CODEC_LZ4: u8 = 1;
const KIND_FULL: u8 = 0;
const KIND_U8: u8 = 1;
const KIND_BIT: u8 = 2;
const HEADER_LEN: usize = 20;
const RECORD_LEN: usize = 9;
const U8_LEN: usize = TILE_SIZE * TILE_SIZE;
const BIT_LEN: usize = TILE_SIZE * TILE_SIZE / 8;
/// Smaller payloads are stored: lz4 would not pay for its header.
const LZ4_MIN: usize = 64;

/// The last encoding, keyed by `Document::selection_rev` (autosave rewrites
/// the manifest on every commit). The key also holds the selection itself,
/// so a revision number reused by another document never hits.
#[derive(Default)]
pub struct SelmCache {
    key: Option<(u64, u32, u32, Selection)>,
    body: Option<Arc<[u8]>>,
    save: SelectionSave,
}

/// How the selection went into the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionSave {
    /// There was no selection to save.
    #[default]
    None,
    Exact,
    /// Over `MAX_SELM_BYTES`: soft edges were made hard to fit.
    Binarized,
    /// Still too large: saved without the selection.
    Dropped,
}

/// The `SELM` body for `sel` on a `w`×`h` page, if there is one to write.
pub fn encode(sel: &Selection, rev: u64, w: u32, h: u32, c: &mut SelmCache) -> (Option<Arc<[u8]>>, SelectionSave) {
    if sel.is_empty() {
        *c = SelmCache::default();
        return (None, SelectionSave::None);
    }
    if let Some((r, cw, ch, s)) = &c.key
        && (*r, *cw, *ch) == (rev, w, h)
        && s.shares_storage(sel)
    {
        return (c.body.clone(), c.save);
    }
    let (body, save) = encode_uncached(sel, w, h);
    *c = SelmCache { key: Some((rev, w, h, sel.clone())), body: body.clone(), save };
    (body, save)
}

fn encode_uncached(sel: &Selection, w: u32, h: u32) -> (Option<Arc<[u8]>>, SelectionSave) {
    match encode_body(sel, w, h, false) {
        Body::Empty => return (None, SelectionSave::None),
        Body::Fits(b) => return (Some(b.into()), SelectionSave::Exact),
        Body::TooLarge => {}
    }
    match encode_body(sel, w, h, true) {
        Body::Empty => (None, SelectionSave::None),
        Body::Fits(b) => (Some(b.into()), SelectionSave::Binarized),
        Body::TooLarge => (None, SelectionSave::Dropped),
    }
}

enum Body {
    /// No page tile is selected (after binarizing).
    Empty,
    Fits(Vec<u8>),
    TooLarge,
}

enum Stored<'a> {
    Full,
    U8(&'a MaskPixels),
    Bit(Box<[u8; BIT_LEN]>),
}

fn encode_body(sel: &Selection, w: u32, h: u32, binarize: bool) -> Body {
    let (tw, th) = page_tiles(w, h);
    let mut tiles: Vec<(TileCoord, Stored<'_>)> = Vec::with_capacity(sel.tile_count());
    for (c, _) in sel.tiles() {
        if c.x < 0 || c.y < 0 || i64::from(c.x) >= tw || i64::from(c.y) >= th {
            continue;
        }
        let stored = match sel.get(c) {
            MaskView::Empty => continue,
            MaskView::Full => Stored::Full,
            MaskView::Partial(m) if binarize => {
                let bits = pack_bits(m, |v| v >= 128);
                match (bits.iter().all(|&b| b == 0), bits.iter().all(|&b| b == 0xFF)) {
                    (true, _) => continue,
                    (_, true) => Stored::Full,
                    _ => Stored::Bit(bits),
                }
            }
            MaskView::Partial(m) if m.as_flattened().iter().all(|&v| v == 0 || v == 255) => {
                Stored::Bit(pack_bits(m, |v| v == 255))
            }
            MaskView::Partial(m) => Stored::U8(m),
        };
        tiles.push((c, stored));
    }
    if tiles.is_empty() {
        return Body::Empty;
    }
    tiles.sort_unstable_by_key(|(c, _)| (c.y, c.x));
    let n_u8 = tiles.iter().filter(|t| matches!(t.1, Stored::U8(_))).count();
    let n_bit = tiles.iter().filter(|t| matches!(t.1, Stored::Bit(_))).count();
    let raw_len = tiles.len() * RECORD_LEN + n_u8 * U8_LEN + n_bit * BIT_LEN;
    let Ok(raw_len32) = u32::try_from(raw_len) else { return Body::TooLarge };
    let mut raw = Vec::with_capacity(raw_len);
    for (c, t) in &tiles {
        raw.extend_from_slice(&c.x.to_le_bytes());
        raw.extend_from_slice(&c.y.to_le_bytes());
        raw.push(match t {
            Stored::Full => KIND_FULL,
            Stored::U8(_) => KIND_U8,
            Stored::Bit(_) => KIND_BIT,
        });
    }
    for (_, t) in &tiles {
        if let Stored::U8(m) = t {
            for row in m.iter() {
                let mut prev = 0u8;
                raw.extend(row.iter().map(|&v| {
                    let d = v.wrapping_sub(prev);
                    prev = v;
                    d
                }));
            }
        }
    }
    for (_, t) in &tiles {
        if let Stored::Bit(b) = t {
            raw.extend_from_slice(&b[..]);
        }
    }
    debug_assert_eq!(raw.len(), raw_len);
    let packed = (raw.len() > LZ4_MIN).then(|| lz4_flex::block::compress(&raw)).filter(|p| p.len() < raw.len());
    let (codec, payload) = match &packed {
        Some(p) => (CODEC_LZ4, p.as_slice()),
        None => (CODEC_STORED, raw.as_slice()),
    };
    if HEADER_LEN + payload.len() > MAX_SELM_BYTES {
        return Body::TooLarge;
    }
    let mut body = Vec::with_capacity(HEADER_LEN + payload.len());
    body.extend_from_slice(&[SELM_VERSION, codec, 0, 0]);
    body.extend_from_slice(&w.to_le_bytes());
    body.extend_from_slice(&h.to_le_bytes());
    body.extend_from_slice(&(tiles.len() as u32).to_le_bytes());
    body.extend_from_slice(&raw_len32.to_le_bytes());
    body.extend_from_slice(payload);
    Body::Fits(body)
}

fn page_tiles(w: u32, h: u32) -> (i64, i64) {
    (i64::from(w.div_ceil(TILE_SIZE as u32)), i64::from(h.div_ceil(TILE_SIZE as u32)))
}

fn pack_bits(m: &MaskPixels, on: impl Fn(u8) -> bool) -> Box<[u8; BIT_LEN]> {
    let mut b = Box::new([0u8; BIT_LEN]);
    for (y, row) in m.iter().enumerate() {
        for (x, &v) in row.iter().enumerate() {
            if on(v) {
                b[y * TILE_SIZE / 8 + x / 8] |= 1 << (x % 8);
            }
        }
    }
    b
}

/// The selection in a `SELM` body, or `None` (with a warning) when it does
/// not fit a `w`×`h` page or is damaged.
pub fn decode(b: &[u8], w: u32, h: u32, warn: &mut Vec<LoadWarning>) -> Option<Selection> {
    match decode_body(b, w, h) {
        Ok(sel) => Some(sel),
        Err(reason) => {
            warn.push(LoadWarning::SelectionDropped { reason });
            None
        }
    }
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn decode_body(b: &[u8], w: u32, h: u32) -> Result<Selection, &'static str> {
    if b.len() < HEADER_LEN {
        return Err("truncated");
    }
    if b[0] != SELM_VERSION {
        return Err("unknown version");
    }
    let codec = b[1];
    let (pw, ph, n, raw_len) = (le_u32(b, 4), le_u32(b, 8), le_u32(b, 12) as usize, le_u32(b, 16) as usize);
    if (pw, ph) != (w, h) {
        return Err("page size mismatch");
    }
    let (tw, th) = page_tiles(w, h);
    if n as u64 > (tw * th) as u64 {
        return Err("too many tiles");
    }
    if raw_len as u64 > n as u64 * (RECORD_LEN + U8_LEN) as u64 {
        return Err("bad length");
    }
    let payload = &b[HEADER_LEN..];
    let raw: Cow<'_, [u8]> = match codec {
        CODEC_STORED if payload.len() == raw_len => Cow::Borrowed(payload),
        CODEC_STORED => return Err("bad length"),
        CODEC_LZ4 => {
            if payload.is_empty() || raw_len as u64 > lz4_max_raw(payload.len() as u64) {
                return Err("bad length");
            }
            let mut buf = vec![0u8; raw_len];
            let got = lz4_flex::block::decompress_into(payload, &mut buf).map_err(|_| "lz4 data")?;
            if got != raw_len {
                return Err("bad length");
            }
            Cow::Owned(buf)
        }
        _ => return Err("unknown codec"),
    };
    if raw.len() < n * RECORD_LEN {
        return Err("bad length");
    }
    let (records, data) = raw.split_at(n * RECORD_LEN);
    let mut prev: Option<(i32, i32)> = None;
    let (mut n_u8, mut n_bit) = (0usize, 0usize);
    for r in records.chunks_exact(RECORD_LEN) {
        let (tx, ty) = (le_u32(r, 0) as i32, le_u32(r, 4) as i32);
        if tx < 0 || ty < 0 || i64::from(tx) >= tw || i64::from(ty) >= th {
            return Err("off-page tile");
        }
        if prev.is_some_and(|p| p >= (ty, tx)) {
            return Err("tile order");
        }
        prev = Some((ty, tx));
        match r[8] {
            KIND_FULL => {}
            KIND_U8 => n_u8 += 1,
            KIND_BIT => n_bit += 1,
            _ => return Err("bad tile kind"),
        }
    }
    if data.len() != n_u8 * U8_LEN + n_bit * BIT_LEN {
        return Err("bad length");
    }
    let (mut u8s, mut bits) = (data[..n_u8 * U8_LEN].chunks_exact(U8_LEN), data[n_u8 * U8_LEN..].chunks_exact(BIT_LEN));
    let mut sel = Selection::new();
    for r in records.chunks_exact(RECORD_LEN) {
        let c = TileCoord::new(le_u32(r, 0) as i32, le_u32(r, 4) as i32);
        let mut m: MaskPixels = [[0; TILE_SIZE]; TILE_SIZE];
        match r[8] {
            KIND_FULL => {
                sel.insert_tile(c, full_mask().clone());
                continue;
            }
            KIND_U8 => {
                let src = u8s.next().ok_or("bad length")?;
                for (row, d) in m.iter_mut().zip(src.chunks_exact(TILE_SIZE)) {
                    let mut acc = 0u8;
                    for (v, &d) in row.iter_mut().zip(d) {
                        acc = acc.wrapping_add(d);
                        *v = acc;
                    }
                }
            }
            _ => {
                let src = bits.next().ok_or("bad length")?;
                for (y, row) in m.iter_mut().enumerate() {
                    for (x, v) in row.iter_mut().enumerate() {
                        *v = if src[y * TILE_SIZE / 8 + x / 8] >> (x % 8) & 1 != 0 { 255 } else { 0 };
                    }
                }
            }
        }
        sel.insert_tile(c, Arc::new(m));
    }
    Ok(sel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout() {
        let sel = Selection::all(100, 70);
        let (body, save) = encode(&sel, 1, 100, 70, &mut SelmCache::default());
        let body = body.unwrap();
        assert_eq!(save, SelectionSave::Exact);
        assert_eq!(&body[..4], &[1, 0, 0, 0]);
        assert_eq!(le_u32(&body, 4), 100);
        assert_eq!(le_u32(&body, 8), 70);
        assert_eq!(le_u32(&body, 12), 4);
        assert_eq!(le_u32(&body, 16), 4 * 9);
        let back = decode(&body, 100, 70, &mut Vec::new()).unwrap();
        assert_eq!(back.tile_count(), 4);
        assert!(matches!(back.get(TileCoord::new(1, 1)), MaskView::Full));
    }
}
