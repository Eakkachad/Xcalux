//! On-disk structures of the v2 container: file header, record header,
//! commit, tile entry and layer record, plus the bounds-checked
//! [`ByteReader`] every parser goes through.
//!
//! All integers are little-endian. Decoders never trust a length or offset:
//! they return `IoError::Corrupt` instead of panicking or over-allocating.

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

use std::time::SystemTime;

use arty_core::TileCoord;

use crate::error::IoError;
use crate::limits::MAX_NAME_LEN;

pub const MAGIC: [u8; 4] = *b"ARTY";
pub const FORMAT_MAJOR: u32 = 2;
pub const FORMAT_MINOR: u32 = 0;
pub const HEADER_LEN: usize = 64;
/// CR LF ^Z LF: any text-mode translation of the file breaks it.
pub const EOL_GUARD: [u8; 4] = [0x0D, 0x0A, 0x1A, 0x0A];
/// `optional_flags` bit: this is a recovery (autosave) file.
pub const OPT_RECOVERY_FILE: u32 = 1;
/// `required_flags` bits this version understands.
pub const KNOWN_REQUIRED_FLAGS: u32 = 0;

pub const REC_MAGIC: [u8; 4] = *b"ArRc";
pub const RECORD_HEADER_LEN: usize = 24;
pub const COMMIT_PAYLOAD_LEN: usize = 32;
/// A commit record is always the last 56 bytes of a cleanly written file.
pub const COMMIT_RECORD_LEN: usize = RECORD_HEADER_LEN + COMMIT_PAYLOAD_LEN;

pub const TILE_ENTRY_LEN: usize = 32;
/// Raw tile bytes: `TilePixels` as little-endian u16.
pub const TILE_BYTES: usize = 32768;
/// Tile coordinate domain: every tile reachable from i32 pixels.
pub const T_MIN: i32 = -(1 << 25);
pub const T_MAX: i32 = (1 << 25) - 1;

/// Fixed bytes of a LAYR record after its `rec_len` field, before the name.
pub const LAYER_RECORD_FIXED: usize = 30;
pub const LAYER_KIND_RASTER: u8 = 0;
pub const LAYER_KIND_FOLDER: u8 = 1;
pub const LF_VISIBLE: u8 = 1;
pub const LF_CLIP: u8 = 1 << 1;
pub const LF_LOCK_ALPHA: u8 = 1 << 2;
pub const LF_LOCKED: u8 = 1 << 3;
pub const LF_EXPANDED: u8 = 1 << 4;

// ----- byte access -----------------------------------------------------------

/// Bounds-checked little-endian reader. Every read returns `Corrupt`
/// ("truncated …") instead of panicking when the input runs out.
#[derive(Clone)]
pub struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
    /// File offset of `buf[0]`, for error reports.
    base: u64,
    what: &'static str,
}

impl<'a> ByteReader<'a> {
    /// `what` names the structure in errors; `base` is its file offset.
    pub fn new(buf: &'a [u8], what: &'static str, base: u64) -> Self {
        Self { buf, pos: 0, base, what }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// File offset of the next byte.
    pub fn offset(&self) -> u64 {
        self.base.saturating_add(self.pos as u64)
    }

    fn truncated(&self) -> IoError {
        IoError::corrupt(self.what, self.offset())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], IoError> {
        let b = self.pos.checked_add(n).and_then(|end| self.buf.get(self.pos..end)).ok_or_else(|| self.truncated())?;
        self.pos = self.pos.saturating_add(n);
        Ok(b)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], IoError> {
        let b = self.bytes(N)?;
        b.try_into().map_err(|_| self.truncated())
    }

    pub fn skip(&mut self, n: usize) -> Result<(), IoError> {
        self.bytes(n).map(|_| ())
    }

    /// Everything not read yet.
    pub fn rest(&mut self) -> &'a [u8] {
        let b = self.buf.get(self.pos..).unwrap_or_default();
        self.pos = self.buf.len();
        b
    }

    /// A reader over the next `n` bytes, which this reader skips.
    pub fn sub(&mut self, n: usize, what: &'static str) -> Result<ByteReader<'a>, IoError> {
        let base = self.offset();
        Ok(ByteReader::new(self.bytes(n)?, what, base))
    }

    pub fn u8(&mut self) -> Result<u8, IoError> {
        self.array::<1>().map(|[b]| b)
    }

    pub fn u16(&mut self) -> Result<u16, IoError> {
        self.array().map(u16::from_le_bytes)
    }

    pub fn u32(&mut self) -> Result<u32, IoError> {
        self.array().map(u32::from_le_bytes)
    }

    pub fn u64(&mut self) -> Result<u64, IoError> {
        self.array().map(u64::from_le_bytes)
    }

    pub fn i32(&mut self) -> Result<i32, IoError> {
        self.array().map(i32::from_le_bytes)
    }

    pub fn f32(&mut self) -> Result<f32, IoError> {
        self.array().map(f32::from_le_bytes)
    }
}

/// Writes consecutive fields into a fixed-size buffer.
struct Put<'a> {
    rest: &'a mut [u8],
}

impl<'a> Put<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { rest: buf }
    }

    fn bytes(&mut self, b: &[u8]) {
        let rest = std::mem::take(&mut self.rest);
        match rest.split_at_mut_checked(b.len()) {
            Some((head, tail)) => {
                head.copy_from_slice(b);
                self.rest = tail;
            }
            None => debug_assert!(false, "encoder buffer too small"),
        }
    }

    fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }

    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

/// `crc32` of the first `n` bytes of `b` (all of them when shorter).
fn crc_prefix(b: &[u8], n: usize) -> u32 {
    crc32fast::hash(b.get(..n).unwrap_or(b))
}

// ----- file header -----------------------------------------------------------

/// The 64-byte file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub minor: u32,
    pub required_flags: u32,
    pub optional_flags: u32,
    /// Version of the ARTY build that wrote the file (`major<<16|minor<<8|patch`).
    pub creator: u32,
    /// Random per physical file write.
    pub file_uuid: [u8; 16],
}

impl Header {
    pub fn new(optional_flags: u32, file_uuid: [u8; 16]) -> Self {
        Self { minor: FORMAT_MINOR, required_flags: 0, optional_flags, creator: creator_version(), file_uuid }
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        let mut w = Put::new(&mut b);
        w.bytes(&MAGIC);
        w.u32(FORMAT_MAJOR);
        w.u32(self.minor);
        w.u32(HEADER_LEN as u32);
        w.u32(self.required_flags);
        w.u32(self.optional_flags);
        w.bytes(&EOL_GUARD);
        w.u32(self.creator);
        w.bytes(&self.file_uuid);
        w.bytes(&[0; 12]);
        let crc = crc_prefix(&b, 60);
        if let Some(tail) = b.get_mut(60..) {
            tail.copy_from_slice(&crc.to_le_bytes());
        }
        b
    }

    /// Parse and validate a v2 header. Callers sniff first; a v1 file is
    /// `NotArty` here.
    pub fn decode(b: &[u8]) -> Result<Header, IoError> {
        let mut r = ByteReader::new(b, "truncated file header", 0);
        if r.array::<4>().ok() != Some(MAGIC) {
            return Err(IoError::NotArty);
        }
        let major = r.u32().map_err(|_| IoError::NotArty)?;
        if major > FORMAT_MAJOR {
            return Err(IoError::NewerFormat { major });
        }
        if major != FORMAT_MAJOR {
            return Err(IoError::NotArty);
        }
        let minor = r.u32()?;
        let header_len = r.u32()?;
        let required_flags = r.u32()?;
        let optional_flags = r.u32()?;
        let eol: [u8; 4] = r.array()?;
        let creator = r.u32()?;
        let file_uuid: [u8; 16] = r.array()?;
        r.skip(12)?;
        let crc = r.u32()?;
        // Checked before the CRC, which text-mode damage also breaks, so
        // the user gets the more useful message.
        if eol != EOL_GUARD {
            return Err(IoError::corrupt("text-mode damage", 24));
        }
        if crc_prefix(b, 60) != crc {
            return Err(IoError::corrupt("file header crc", 60));
        }
        if header_len as usize != HEADER_LEN {
            return Err(IoError::corrupt("file header length", 12));
        }
        if required_flags & !KNOWN_REQUIRED_FLAGS != 0 {
            return Err(IoError::UnsupportedFeature { tag: *b"RQFL" });
        }
        Ok(Header { minor, required_flags, optional_flags, creator, file_uuid })
    }
}

/// This build's version as stored in `Header::creator`.
pub fn creator_version() -> u32 {
    let part = |s: &str| s.parse::<u8>().unwrap_or(u8::MAX);
    u32::from_le_bytes([
        part(env!("CARGO_PKG_VERSION_PATCH")),
        part(env!("CARGO_PKG_VERSION_MINOR")),
        part(env!("CARGO_PKG_VERSION_MAJOR")),
        0,
    ])
}

/// A fresh random file uuid.
pub fn new_file_uuid() -> Result<[u8; 16], IoError> {
    let mut u = [0u8; 16];
    getrandom::fill(&mut u)
        .map_err(|e| IoError::Io { op: "generate file id", source: std::io::Error::other(e.to_string()) })?;
    Ok(u)
}

// ----- records ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordKind {
    Segment = 1,
    Manifest = 2,
    Commit = 3,
    TileTable = 4,
}

impl RecordKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Segment),
            2 => Some(Self::Manifest),
            3 => Some(Self::Commit),
            4 => Some(Self::TileTable),
            _ => None,
        }
    }
}

/// The 24-byte header in front of every record. `kind` stays raw: scans
/// skip kinds they do not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    pub kind: u8,
    pub payload_len: u64,
    /// crc32 of the payload; 0 for Segments (each blob has its own CRCs).
    pub payload_crc: u32,
}

impl RecordHeader {
    pub fn for_payload(kind: RecordKind, payload: &[u8]) -> Self {
        let payload_crc = if kind == RecordKind::Segment { 0 } else { crc32fast::hash(payload) };
        Self { kind: kind as u8, payload_len: payload.len() as u64, payload_crc }
    }

    pub fn encode(&self) -> [u8; RECORD_HEADER_LEN] {
        let mut b = [0u8; RECORD_HEADER_LEN];
        let mut w = Put::new(&mut b);
        w.bytes(&REC_MAGIC);
        w.u8(self.kind);
        w.u8(0);
        w.u16(0);
        w.u64(self.payload_len);
        w.u32(self.payload_crc);
        let crc = crc_prefix(&b, 20);
        if let Some(tail) = b.get_mut(20..) {
            tail.copy_from_slice(&crc.to_le_bytes());
        }
        b
    }

    /// Parse the header of the record at file offset `at`. Checks the magic
    /// and header CRC only; the kind and length are the caller's to judge.
    pub fn decode(b: &[u8], at: u64) -> Result<RecordHeader, IoError> {
        let mut r = ByteReader::new(b, "truncated record header", at);
        if r.array::<4>()? != REC_MAGIC {
            return Err(IoError::corrupt("record magic", at));
        }
        let kind = r.u8()?;
        r.skip(3)?;
        let payload_len = r.u64()?;
        let payload_crc = r.u32()?;
        if crc_prefix(b, 20) != r.u32()? {
            return Err(IoError::corrupt("record header crc", at));
        }
        Ok(RecordHeader { kind, payload_len, payload_crc })
    }

    pub fn record_kind(&self) -> Option<RecordKind> {
        RecordKind::from_u8(self.kind)
    }

    /// File offset just past the payload of this record at `at`.
    pub fn end(&self, at: u64) -> Result<u64, IoError> {
        at.checked_add(RECORD_HEADER_LEN as u64)
            .and_then(|p| p.checked_add(self.payload_len))
            .ok_or(IoError::corrupt("record length", at))
    }

    /// Check a payload read for this header (CRC for every kind but Segment).
    pub fn check_payload(&self, payload: &[u8], at: u64) -> Result<(), IoError> {
        if payload.len() as u64 != self.payload_len {
            return Err(IoError::corrupt("record length", at));
        }
        if self.kind != RecordKind::Segment as u8 && crc32fast::hash(payload) != self.payload_crc {
            return Err(IoError::corrupt("record payload crc", at));
        }
        Ok(())
    }
}

/// The commit record: names the manifest that is the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    pub manifest_offset: u64,
    /// 0 when this is the first commit of the file.
    pub prev_commit_offset: u64,
    pub commit_seq: u64,
    pub unix_ms: u64,
}

impl Commit {
    /// The whole 56-byte record (header and payload).
    pub fn encode_record(&self) -> [u8; COMMIT_RECORD_LEN] {
        let mut payload = [0u8; COMMIT_PAYLOAD_LEN];
        let mut w = Put::new(&mut payload);
        w.u64(self.manifest_offset);
        w.u64(self.prev_commit_offset);
        w.u64(self.commit_seq);
        w.u64(self.unix_ms);
        let mut b = [0u8; COMMIT_RECORD_LEN];
        let mut w = Put::new(&mut b);
        w.bytes(&RecordHeader::for_payload(RecordKind::Commit, &payload).encode());
        w.bytes(&payload);
        b
    }

    /// Parse the commit record at file offset `at` from its 56 bytes,
    /// checking both CRCs and that the offsets point backwards. Whether the
    /// manifest record itself is valid is the reader's to check.
    pub fn decode_record(b: &[u8], at: u64) -> Result<Commit, IoError> {
        let h = RecordHeader::decode(b, at)?;
        if h.kind != RecordKind::Commit as u8 || h.payload_len != COMMIT_PAYLOAD_LEN as u64 {
            return Err(IoError::corrupt("not a commit record", at));
        }
        let mut r = ByteReader::new(b, "truncated commit", at);
        r.skip(RECORD_HEADER_LEN)?;
        let payload = r.bytes(COMMIT_PAYLOAD_LEN)?;
        h.check_payload(payload, at)?;
        let mut r = ByteReader::new(payload, "truncated commit", at);
        let c = Commit { manifest_offset: r.u64()?, prev_commit_offset: r.u64()?, commit_seq: r.u64()?, unix_ms: r.u64()? };
        let manifest_fits = c.manifest_offset.checked_add(RECORD_HEADER_LEN as u64).is_some_and(|end| end <= at);
        if c.manifest_offset < HEADER_LEN as u64 || !manifest_fits || c.prev_commit_offset >= c.manifest_offset {
            return Err(IoError::corrupt("commit offsets", at));
        }
        Ok(c)
    }
}

// ----- tile entries ----------------------------------------------------------

/// How a tile is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TileCodec {
    /// 32768 raw bytes.
    Raw = 0,
    /// Byte-plane shuffle + lz4 block.
    Lz4Shuf = 1,
    /// One pixel value in the entry, no blob.
    Solid = 2,
    /// Vertical delta + zigzag + lz4 block.
    Lz4Vdelta = 3,
}

impl TileCodec {
    /// `None` for 4 and up (4 is reserved for zstd).
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Raw),
            1 => Some(Self::Lz4Shuf),
            2 => Some(Self::Solid),
            3 => Some(Self::Lz4Vdelta),
            _ => None,
        }
    }

    /// True when the tile has stored bytes in a Segment.
    pub fn has_blob(self) -> bool {
        self != Self::Solid
    }
}

/// One 32-byte TileTable entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEntry {
    pub coord: TileCoord,
    pub codec: TileCodec,
    pub stored_len: u32,
    /// crc32 of the 32768 decoded bytes (0 for SOLID).
    pub raw_crc: u32,
    /// crc32 of the stored bytes (0 for SOLID).
    pub stored_crc: u32,
    /// Blob offset, or the packed pixel for SOLID.
    pub offset: u64,
}

/// `r | g<<16 | b<<32 | a<<48`.
pub fn pack_solid(px: [u16; 4]) -> u64 {
    let [r, g, b, a] = px.map(u16::to_le_bytes);
    u64::from_le_bytes([r[0], r[1], g[0], g[1], b[0], b[1], a[0], a[1]])
}

pub fn unpack_solid(v: u64) -> [u16; 4] {
    let [r0, r1, g0, g1, b0, b1, a0, a1] = v.to_le_bytes();
    [u16::from_le_bytes([r0, r1]), u16::from_le_bytes([g0, g1]), u16::from_le_bytes([b0, b1]), u16::from_le_bytes([a0, a1])]
}

pub fn coord_in_domain(c: TileCoord) -> bool {
    (T_MIN..=T_MAX).contains(&c.x) && (T_MIN..=T_MAX).contains(&c.y)
}

impl TileEntry {
    pub fn solid(coord: TileCoord, px: [u16; 4]) -> Self {
        Self { coord, codec: TileCodec::Solid, stored_len: 0, raw_crc: 0, stored_crc: 0, offset: pack_solid(px) }
    }

    /// The pixel of a SOLID entry (not yet sanitized).
    pub fn solid_value(&self) -> Option<[u16; 4]> {
        (self.codec == TileCodec::Solid).then(|| unpack_solid(self.offset))
    }

    pub fn encode(&self) -> [u8; TILE_ENTRY_LEN] {
        let mut b = [0u8; TILE_ENTRY_LEN];
        let mut w = Put::new(&mut b);
        w.bytes(&self.coord.x.to_le_bytes());
        w.bytes(&self.coord.y.to_le_bytes());
        w.u8(self.codec as u8);
        w.u8(0);
        w.u16(0);
        w.u32(self.stored_len);
        w.u32(self.raw_crc);
        w.u32(self.stored_crc);
        w.u64(self.offset);
        b
    }

    /// Parse the first 32 bytes of an entry of the table record at
    /// `table_offset` (entries may be longer in later minors). Validates
    /// everything an entry can check alone, including that a blob lies
    /// between the file header and the table.
    pub fn decode(b: &[u8], table_offset: u64) -> Result<TileEntry, IoError> {
        let bad = |what| IoError::corrupt(what, table_offset);
        let mut r = ByteReader::new(b, "truncated tile entry", table_offset);
        let coord = TileCoord::new(r.i32()?, r.i32()?);
        let codec = TileCodec::from_u8(r.u8()?).ok_or(bad("unknown tile codec"))?;
        if r.u8()? != 0 {
            return Err(bad("tile entry flags"));
        }
        r.skip(2)?;
        let e = TileEntry { coord, codec, stored_len: r.u32()?, raw_crc: r.u32()?, stored_crc: r.u32()?, offset: r.u64()? };
        if !coord_in_domain(coord) {
            return Err(bad("tile coordinate out of range"));
        }
        let len_ok = match codec {
            TileCodec::Raw => e.stored_len as usize == TILE_BYTES,
            TileCodec::Lz4Shuf | TileCodec::Lz4Vdelta => (1..TILE_BYTES as u32).contains(&e.stored_len),
            TileCodec::Solid => e.stored_len == 0,
        };
        if !len_ok {
            return Err(bad("tile stored length"));
        }
        if codec.has_blob() {
            let in_range = e.offset >= HEADER_LEN as u64 && e.blob_end().is_some_and(|end| end <= table_offset);
            if !in_range {
                return Err(bad("tile blob range"));
            }
        }
        Ok(e)
    }

    /// File offset just past the blob.
    pub fn blob_end(&self) -> Option<u64> {
        self.offset.checked_add(u64::from(self.stored_len))
    }
}

// ----- layer records ---------------------------------------------------------

/// One layer of the `LAYR` section, as stored. Semantic checks (ids,
/// parents, tables) belong to the manifest parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerRecord<'a> {
    pub id: u32,
    /// 0 = top level, else an earlier folder record.
    pub parent_id: u32,
    pub kind: u8,
    /// `LF_*` bits.
    pub flags: u8,
    pub blend: u8,
    /// f32 bits, kept exact.
    pub opacity_bits: u32,
    pub tile_count: u32,
    /// TileTable record offset; 0 when `tile_count == 0`.
    pub table_offset: u64,
    /// UTF-8 as written (not validated here), at most `MAX_NAME_LEN` bytes.
    pub name: &'a [u8],
}

impl LayerRecord<'_> {
    /// Append `rec_len` and the record. The writer truncates names to
    /// `MAX_NAME_LEN` beforehand.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        debug_assert!(self.name.len() <= MAX_NAME_LEN);
        let name = self.name.get(..MAX_NAME_LEN).unwrap_or(self.name);
        let name_len = name.len() as u16;
        out.extend_from_slice(&(LAYER_RECORD_FIXED as u16).saturating_add(name_len).to_le_bytes());
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&self.parent_id.to_le_bytes());
        out.extend_from_slice(&[self.kind, self.flags, self.blend, 0]);
        out.extend_from_slice(&self.opacity_bits.to_le_bytes());
        out.extend_from_slice(&self.tile_count.to_le_bytes());
        out.extend_from_slice(&self.table_offset.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name);
    }
}

impl<'a> LayerRecord<'a> {
    /// Read one record (with its `rec_len` prefix). Bytes past the name
    /// within `rec_len` are ignored, so later minors can extend records.
    pub fn decode(r: &mut ByteReader<'a>) -> Result<LayerRecord<'a>, IoError> {
        let rec_len = r.u16()?;
        let mut b = r.sub(rec_len.into(), "truncated layer record")?;
        let id = b.u32()?;
        let parent_id = b.u32()?;
        let [kind, flags, blend, _] = b.array()?;
        let opacity_bits = b.u32()?;
        let tile_count = b.u32()?;
        let table_offset = b.u64()?;
        let name_len = b.u16()?;
        if usize::from(name_len) > MAX_NAME_LEN {
            return Err(IoError::limit("layer name length", name_len.into(), MAX_NAME_LEN as u64));
        }
        let name = b.bytes(name_len.into())?;
        Ok(LayerRecord { id, parent_id, kind, flags, blend, opacity_bits, tile_count, table_offset, name })
    }
}

// ----- identity --------------------------------------------------------------

/// What we remember about a file we read or wrote, to notice when someone
/// else changed it before we copy from or overwrite it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub file_uuid: [u8; 16],
    pub len: u64,
    pub commit_offset: u64,
    pub commit_seq: u64,
    /// `None` where the file system has no modification time.
    pub mtime: Option<SystemTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_iso_hdlc() {
        assert_eq!(crc32fast::hash(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn header_round_trips() {
        let h = Header { minor: 9, required_flags: 0, optional_flags: OPT_RECOVERY_FILE | 0x80, creator: 0x0002_0300, file_uuid: [7; 16] };
        let b = h.encode();
        assert_eq!(Header::decode(&b).unwrap(), h);
        assert_eq!(&b[..4], b"ARTY");
        assert_eq!(&b[24..28], &EOL_GUARD);
        assert_eq!(Header::new(0, [1; 16]).creator, creator_version());
        let part = |s: &str| s.parse::<u32>().unwrap();
        let want = part(env!("CARGO_PKG_VERSION_MAJOR")) << 16
            | part(env!("CARGO_PKG_VERSION_MINOR")) << 8
            | part(env!("CARGO_PKG_VERSION_PATCH"));
        assert_eq!(creator_version(), want);
    }

    #[test]
    fn header_rejections() {
        let good = Header::new(0, [3; 16]).encode();
        let with = |f: &dyn Fn(&mut [u8; 64])| {
            let mut b = good;
            f(&mut b);
            let crc = crc32fast::hash(&b[..60]);
            b[60..].copy_from_slice(&crc.to_le_bytes());
            Header::decode(&b)
        };
        assert!(matches!(Header::decode(b"PNG\0abcd"), Err(IoError::NotArty)));
        assert!(matches!(Header::decode(b"AR"), Err(IoError::NotArty)));
        assert!(matches!(with(&|b| b[4] = 3), Err(IoError::NewerFormat { major: 3 })));
        assert!(matches!(with(&|b| b[4] = 1), Err(IoError::NotArty)));
        assert!(matches!(with(&|b| b[25] = 0x0D), Err(IoError::Corrupt { what: "text-mode damage", .. })));
        assert!(matches!(with(&|b| b[12] = 65), Err(IoError::Corrupt { what: "file header length", .. })));
        assert!(matches!(with(&|b| b[16] = 1), Err(IoError::UnsupportedFeature { .. })));
        assert!(with(&|b| b[20] = 0xFF).is_ok(), "unknown optional flags are ignored");
        assert!(with(&|b| b[50] = 1).is_ok(), "reserved bytes are ignored");
        let mut b = good;
        b[40] ^= 1;
        assert!(matches!(Header::decode(&b), Err(IoError::Corrupt { what: "file header crc", .. })));
        assert!(matches!(Header::decode(&good[..63]), Err(IoError::Corrupt { .. })));
    }

    #[test]
    fn record_header_round_trips() {
        let h = RecordHeader::for_payload(RecordKind::Manifest, b"payload");
        let b = h.encode();
        let d = RecordHeader::decode(&b, 100).unwrap();
        assert_eq!(d, h);
        assert_eq!(d.record_kind(), Some(RecordKind::Manifest));
        assert_eq!(d.end(100).unwrap(), 100 + 24 + 7);
        d.check_payload(b"payload", 100).unwrap();
        assert!(d.check_payload(b"paylaod", 100).is_err());
        assert!(d.check_payload(b"payloa", 100).is_err());
        assert_eq!(RecordHeader::for_payload(RecordKind::Segment, b"xyz").payload_crc, 0);

        let mut bad = b;
        bad[10] ^= 1;
        assert!(matches!(RecordHeader::decode(&bad, 0), Err(IoError::Corrupt { what: "record header crc", .. })));
        bad = b;
        bad[0] = b'X';
        assert!(matches!(RecordHeader::decode(&bad, 0), Err(IoError::Corrupt { what: "record magic", .. })));
        let huge = RecordHeader { kind: 1, payload_len: u64::MAX - 10, payload_crc: 0 };
        assert!(huge.end(64).is_err());
        // Unknown kinds parse; scans decide to skip them.
        let odd = RecordHeader { kind: 77, payload_len: 0, payload_crc: 0 };
        assert_eq!(RecordHeader::decode(&odd.encode(), 0).unwrap().record_kind(), None);
    }

    #[test]
    fn commit_round_trips_and_checks_offsets() {
        let c = Commit { manifest_offset: 64, prev_commit_offset: 0, commit_seq: 3, unix_ms: 1_700_000_000_000 };
        let at = 64 + 24 + 100;
        let b = c.encode_record();
        assert_eq!(Commit::decode_record(&b, at).unwrap(), c);
        assert!(Commit::decode_record(&b, 64 + 23).is_err(), "manifest header must end before the commit");
        for bad in [
            Commit { manifest_offset: 63, ..c },
            Commit { prev_commit_offset: 64, ..c },
            Commit { manifest_offset: u64::MAX, ..c },
        ] {
            assert!(Commit::decode_record(&bad.encode_record(), at).is_err(), "{bad:?}");
        }
        let mut flipped = b;
        flipped[40] ^= 1;
        assert!(matches!(Commit::decode_record(&flipped, at), Err(IoError::Corrupt { what: "record payload crc", .. })));
        let not_commit = RecordHeader { kind: 2, payload_len: 32, payload_crc: 0 }.encode();
        let mut b2 = b;
        b2[..24].copy_from_slice(&not_commit);
        assert!(Commit::decode_record(&b2, at).is_err());
    }

    #[test]
    fn tile_entry_round_trips_and_validates() {
        let table = 1_000_000;
        let blob = TileEntry {
            coord: TileCoord::new(T_MIN, T_MAX),
            codec: TileCodec::Lz4Shuf,
            stored_len: 1234,
            raw_crc: 0xDEAD_BEEF,
            stored_crc: 0x1234_5678,
            offset: 5000,
        };
        assert_eq!(TileEntry::decode(&blob.encode(), table).unwrap(), blob);
        let px = [0x8000, 1, 0xFFFF, 0x1234];
        let solid = TileEntry::solid(TileCoord::new(-1, 2), px);
        assert_eq!(solid.offset, 0x1234_FFFF_0001_8000);
        let back = TileEntry::decode(&solid.encode(), table).unwrap();
        assert_eq!(back.solid_value(), Some(px));
        assert_eq!(blob.solid_value(), None);
        // Longer entries (later minors) use the first 32 bytes.
        let mut long = blob.encode().to_vec();
        long.extend_from_slice(&[0xAA; 16]);
        assert_eq!(TileEntry::decode(&long, table).unwrap(), blob);

        let bad = [
            TileEntry { coord: TileCoord::new(T_MIN - 1, 0), ..blob },
            TileEntry { coord: TileCoord::new(0, i32::MAX), ..blob },
            TileEntry { stored_len: 0, ..blob },
            TileEntry { stored_len: TILE_BYTES as u32, ..blob },
            TileEntry { codec: TileCodec::Raw, ..blob },
            TileEntry { codec: TileCodec::Solid, ..blob },
            TileEntry { offset: 63, ..blob },
            TileEntry { offset: table - 1233, ..blob },
            TileEntry { offset: u64::MAX, ..blob },
        ];
        for e in bad {
            assert!(TileEntry::decode(&e.encode(), table).is_err(), "{e:?}");
        }
        let raw = TileEntry { codec: TileCodec::Raw, stored_len: TILE_BYTES as u32, offset: table - TILE_BYTES as u64, ..blob };
        assert!(TileEntry::decode(&raw.encode(), table).is_ok());
        for (byte, value) in [(8, 4), (8, 0xFF), (9, 1)] {
            let mut b = blob.encode();
            b[byte] = value;
            assert!(TileEntry::decode(&b, table).is_err(), "byte {byte} = {value}");
        }
        let mut b = blob.encode();
        b[10] = 0xFF;
        assert!(TileEntry::decode(&b, table).is_ok(), "reserved bytes are ignored");
    }

    #[test]
    fn layer_record_round_trips() {
        let rec = LayerRecord {
            id: 7,
            parent_id: 3,
            kind: LAYER_KIND_FOLDER,
            flags: LF_VISIBLE | LF_EXPANDED,
            blend: 13,
            opacity_bits: 0.25f32.to_bits(),
            tile_count: 0,
            table_offset: 0,
            name: "Folder ✓".as_bytes(),
        };
        let mut out = Vec::new();
        rec.encode_into(&mut out);
        let max_name = [b'x'; MAX_NAME_LEN];
        let long = LayerRecord { name: &max_name, kind: LAYER_KIND_RASTER, tile_count: 9, table_offset: 99, ..rec };
        long.encode_into(&mut out);
        assert_eq!(u16::from_le_bytes([out[0], out[1]]) as usize, LAYER_RECORD_FIXED + rec.name.len());

        let mut r = ByteReader::new(&out, "layr", 0);
        assert_eq!(LayerRecord::decode(&mut r).unwrap(), rec);
        assert_eq!(LayerRecord::decode(&mut r).unwrap(), long);
        assert!(r.is_empty());

        // A longer record (later minor) skips the extra bytes.
        let mut ext = Vec::new();
        rec.encode_into(&mut ext);
        let len = u16::from_le_bytes([ext[0], ext[1]]) + 5;
        ext[..2].copy_from_slice(&len.to_le_bytes());
        ext.extend_from_slice(&[1, 2, 3, 4, 5, 0xEE]);
        let mut r = ByteReader::new(&ext, "layr", 0);
        assert_eq!(LayerRecord::decode(&mut r).unwrap(), rec);
        assert_eq!(r.rest(), &[0xEE]);

        // rec_len shorter than the name, and an over-long name.
        let mut short = Vec::new();
        rec.encode_into(&mut short);
        short[0] -= 1;
        assert!(LayerRecord::decode(&mut ByteReader::new(&short, "layr", 0)).is_err());
        let mut huge = Vec::new();
        LayerRecord { name: b"", ..rec }.encode_into(&mut huge);
        let n = huge.len();
        huge[n - 2..].copy_from_slice(&(MAX_NAME_LEN as u16 + 1).to_le_bytes());
        huge[..2].copy_from_slice(&u16::MAX.to_le_bytes());
        huge.resize(2 + u16::MAX as usize, 0);
        assert!(matches!(
            LayerRecord::decode(&mut ByteReader::new(&huge, "layr", 0)),
            Err(IoError::LimitExceeded { .. })
        ));
    }

    /// Every decoder fails cleanly on every truncation of valid input.
    #[test]
    fn decoders_never_panic_on_short_input() {
        let header = Header::new(0, [9; 16]).encode();
        let record = RecordHeader::for_payload(RecordKind::TileTable, b"abc").encode();
        let commit = Commit { manifest_offset: 64, prev_commit_offset: 0, commit_seq: 1, unix_ms: 0 }.encode_record();
        let entry = TileEntry::solid(TileCoord::new(0, 0), [1; 4]).encode();
        let mut layer = Vec::new();
        LayerRecord {
            id: 1,
            parent_id: 0,
            kind: 0,
            flags: 0,
            blend: 0,
            opacity_bits: 0,
            tile_count: 0,
            table_offset: 0,
            name: b"name",
        }
        .encode_into(&mut layer);
        for n in 0..header.len() {
            assert!(Header::decode(&header[..n]).is_err());
        }
        for n in 0..record.len() {
            assert!(RecordHeader::decode(&record[..n], 0).is_err());
        }
        for n in 0..commit.len() {
            assert!(Commit::decode_record(&commit[..n], 1000).is_err());
        }
        for n in 0..entry.len() {
            assert!(TileEntry::decode(&entry[..n], 1000).is_err());
        }
        for n in 0..layer.len() {
            assert!(LayerRecord::decode(&mut ByteReader::new(&layer[..n], "layr", 0)).is_err());
        }
        let mut r = ByteReader::new(&[1, 2, 3], "x", u64::MAX);
        assert!(r.u32().is_err());
        assert!(r.bytes(usize::MAX).is_err());
        assert!(r.sub(4, "y").is_err());
        assert_eq!(r.u16().unwrap(), 0x0201);
        assert!(r.u16().is_err());
        assert_eq!(r.u8().unwrap(), 3);
        assert!(r.u8().is_err() && r.f32().is_err() && r.i32().is_err() && r.u64().is_err());
        assert_eq!(r.offset(), u64::MAX, "offsets saturate");

        // Random garbage of every length up to 300 bytes.
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for len in 0..300 {
            let buf: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                })
                .collect();
            let _ = Header::decode(&buf);
            let _ = RecordHeader::decode(&buf, 0);
            let _ = Commit::decode_record(&buf, 500);
            let _ = TileEntry::decode(&buf, 500);
            let mut r = ByteReader::new(&buf, "layr", 0);
            while LayerRecord::decode(&mut r).is_ok() {}
        }
    }

    /// Byte-exact layout of the smallest file: header, an empty stored
    /// manifest, and a commit. Changing any of this breaks old files.
    #[test]
    fn golden_minimal_file() {
        let mut file = Vec::new();
        let header = Header { minor: 0, required_flags: 0, optional_flags: 0, creator: 0x0002_0000, file_uuid: [0x11; 16] };
        file.extend_from_slice(&header.encode());
        let manifest_payload = [0u8; 8]; // codec 0 (stored), raw_len 0, no sections
        let manifest_offset = file.len() as u64;
        file.extend_from_slice(&RecordHeader::for_payload(RecordKind::Manifest, &manifest_payload).encode());
        file.extend_from_slice(&manifest_payload);
        let commit = Commit { manifest_offset, prev_commit_offset: 0, commit_seq: 1, unix_ms: 0x0123_4567_89AB };
        file.extend_from_slice(&commit.encode_record());

        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let expected = concat!(
            // header: magic, major 2, minor 0, len 64, required 0, optional 0
            "41525459", "02000000", "00000000", "40000000", "00000000", "00000000",
            // eol guard, creator 2.0.0, uuid, reserved
            "0d0a1a0a", "00000200", "11111111111111111111111111111111", "000000000000000000000000",
            "988ce407", // header crc
            // manifest record: magic, kind 2, flags/reserved, payload_len 8, payload crc, header crc
            "41725263", "02", "000000", "0800000000000000", "69df2265", "85a68839",
            "0000000000000000", // payload: codec 0, raw_len 0
            // commit record: magic, kind 3, payload_len 32, payload crc, header crc
            "41725263", "03", "000000", "2000000000000000", "5fa9091e", "507c46bd",
            "4000000000000000", "0000000000000000", "0100000000000000", "ab89674523010000",
        );
        assert_eq!(hex(&file), expected);
        assert_eq!(file.len(), 64 + 24 + 8 + 56);
        let commit_at = (file.len() - COMMIT_RECORD_LEN) as u64;
        assert_eq!(Commit::decode_record(&file[commit_at as usize..], commit_at).unwrap(), commit);
        assert_eq!(Header::decode(&file).unwrap(), header);
    }
}
