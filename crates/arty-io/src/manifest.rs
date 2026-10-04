//! The Manifest record: document sections (`DOC `, `LAYR`, `META`, `VIEW`,
//! `THUM`, `LEXT`, and sections from newer versions) as a tagged stream.
//!
//! Payload: `codec` u32 (0 stored, 1 lz4), `raw_len` u32, then the body,
//! which decodes to exactly `raw_len` bytes of `{tag, flags, len, bytes}`.
//! Unknown sections are refused (CRITICAL), kept for re-saving
//! (SAFE_TO_COPY) or skipped, and the latter two make the load lossy.

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

use std::borrow::Cow;

use ahash::AHashSet;

use crate::error::{IoError, LoadWarning, tag_str};
use crate::format::{ByteReader, LayerRecord};
use crate::limits::{
    MAX_DPI, MAX_EXTRA_TOTAL, MAX_LAYER_COUNT, MAX_LEXT_ENTRIES, MAX_LEXT_ENTRY, MAX_LEXT_TOTAL, MAX_MANIFEST_RAW,
    MAX_META_ENTRIES, MAX_META_VALUE, MAX_PAGE_SIDE, MAX_SECTIONS, MAX_THUMB_SIDE, MAX_VIEW_BYTES, lz4_max_raw,
};

/// Section flag: a reader that does not know the tag must refuse the file.
pub const SEC_CRITICAL: u32 = 1;
/// Section flag: a reader that does not know the tag keeps it and writes
/// it back unchanged.
pub const SEC_SAFE_TO_COPY: u32 = 1 << 1;
/// LEXT entry flag: a reader that does not know the entry loses data.
pub const LEXT_CRITICAL: u32 = 1;

pub const TAG_DOC: [u8; 4] = *b"DOC ";
pub const TAG_LAYR: [u8; 4] = *b"LAYR";
pub const TAG_META: [u8; 4] = *b"META";
pub const TAG_VIEW: [u8; 4] = *b"VIEW";
pub const TAG_THUM: [u8; 4] = *b"THUM";
pub const TAG_LEXT: [u8; 4] = *b"LEXT";
/// Further pages of a book (reserved; v2.0 opens page 1 only).
pub const TAG_PAGE: [u8; 4] = *b"PAGE";
/// Tags this version reads. Unknown-section rules apply to all others.
pub const KNOWN_TAGS: [[u8; 4]; 7] = [TAG_DOC, TAG_LAYR, TAG_META, TAG_VIEW, TAG_THUM, TAG_LEXT, TAG_PAGE];

pub const MANIFEST_STORED: u32 = 0;
pub const MANIFEST_LZ4: u32 = 1;
/// Manifests larger than this are lz4-compressed.
const LZ4_THRESHOLD: usize = 4096;

pub const DOC_LEN: usize = 40;
const PAPER_TRANSPARENT: u32 = 0;
const PAPER_COLOUR: u32 = 1;

/// An unknown SAFE_TO_COPY section, kept to be written back on save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSection {
    pub tag: [u8; 4],
    pub flags: u32,
    pub bytes: Vec<u8>,
}

/// One `LEXT` entry: extension data attached to a layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerExt {
    pub layer: u32,
    pub tag: [u8; 4],
    pub flags: u32,
    pub bytes: Vec<u8>,
}

/// The `DOC ` section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocFields {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    /// Not yet sanitized when read.
    pub paper: Option<[u16; 4]>,
    pub active: u32,
    pub next_id: u32,
    pub layer_count: u32,
}

impl DocFields {
    pub fn encode(&self) -> [u8; DOC_LEN] {
        let mut b = Vec::with_capacity(DOC_LEN);
        for v in [self.width, self.height, self.dpi, if self.paper.is_some() { PAPER_COLOUR } else { PAPER_TRANSPARENT }] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for c in self.paper.unwrap_or_default() {
            b.extend_from_slice(&c.to_le_bytes());
        }
        for v in [self.active, self.next_id, self.layer_count, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.try_into().unwrap_or([0; DOC_LEN])
    }

    /// Parse and range-check (`len ≥ 40`; trailing bytes are a later minor's).
    pub fn decode(body: &[u8], at: u64) -> Result<DocFields, IoError> {
        let bad = |what| IoError::corrupt(what, at);
        if body.len() < DOC_LEN {
            return Err(bad("DOC section length"));
        }
        let mut r = ByteReader::new(body, "truncated DOC section", at);
        let (width, height, dpi, paper_mode) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        let paper = [r.u16()?, r.u16()?, r.u16()?, r.u16()?];
        let (active, next_id, layer_count) = (r.u32()?, r.u32()?, r.u32()?);
        let side = 1..=MAX_PAGE_SIDE;
        if !side.contains(&width) || !side.contains(&height) {
            return Err(bad("page size"));
        }
        if !(1..=MAX_DPI).contains(&dpi) {
            return Err(bad("page dpi"));
        }
        let paper = match paper_mode {
            PAPER_TRANSPARENT => None,
            PAPER_COLOUR => Some(paper),
            _ => return Err(bad("paper mode")),
        };
        if layer_count > MAX_LAYER_COUNT {
            return Err(IoError::limit("layers", layer_count.into(), MAX_LAYER_COUNT.into()));
        }
        Ok(DocFields { width, height, dpi, paper, active, next_id, layer_count })
    }
}

/// Builds a manifest body section by section.
#[derive(Default)]
pub struct SectionWriter {
    raw: Vec<u8>,
}

impl SectionWriter {
    pub fn push(&mut self, tag: [u8; 4], flags: u32, body: &[u8]) {
        debug_assert!(body.len() <= MAX_MANIFEST_RAW as usize);
        self.raw.extend_from_slice(&tag);
        self.raw.extend_from_slice(&flags.to_le_bytes());
        self.raw.extend_from_slice(&(body.len() as u32).to_le_bytes());
        self.raw.extend_from_slice(body);
    }

    /// The decoded manifest body.
    pub fn into_raw(self) -> Vec<u8> {
        self.raw
    }
}

/// `META` body. Keys longer than 255 bytes and values longer than 4096
/// bytes are cut at a char boundary; entries past 64 are dropped.
pub fn meta_body(entries: &[(&str, &str)]) -> Vec<u8> {
    let entries = entries.get(..MAX_META_ENTRIES).unwrap_or(entries);
    let mut b = Vec::new();
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (k, v) in entries {
        let (k, v) = (truncate_str(k, u8::MAX.into()), truncate_str(v, MAX_META_VALUE));
        b.push(k.len() as u8);
        b.extend_from_slice(k.as_bytes());
        b.extend_from_slice(&(v.len() as u16).to_le_bytes());
        b.extend_from_slice(v.as_bytes());
    }
    b
}

/// `LEXT` body.
pub fn lext_body<'a>(entries: impl ExactSizeIterator<Item = &'a LayerExt>) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        b.extend_from_slice(&e.layer.to_le_bytes());
        b.extend_from_slice(&e.tag);
        b.extend_from_slice(&e.flags.to_le_bytes());
        b.extend_from_slice(&(e.bytes.len() as u32).to_le_bytes());
        b.extend_from_slice(&e.bytes);
    }
    b
}

/// The longest prefix of `s` of at most `max` bytes ending at a char boundary.
pub fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let end = (0..=max).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    s.get(..end).unwrap_or_default()
}

/// The record payload for a manifest body: lz4 when larger than 4 KiB and
/// smaller compressed.
pub fn encode_payload(raw: &[u8]) -> Vec<u8> {
    let packed = (raw.len() > LZ4_THRESHOLD).then(|| lz4_flex::block::compress(raw)).filter(|p| p.len() < raw.len());
    let (codec, body) = match &packed {
        Some(p) => (MANIFEST_LZ4, p.as_slice()),
        None => (MANIFEST_STORED, raw),
    };
    let mut out = Vec::with_capacity(body.len().saturating_add(8));
    out.extend_from_slice(&codec.to_le_bytes());
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// The decoded body of a manifest payload read from the record at `at`.
/// The output buffer is sized only after `raw_len` passed its limits.
pub fn decode_payload(payload: &[u8], at: u64) -> Result<Cow<'_, [u8]>, IoError> {
    let mut r = ByteReader::new(payload, "truncated manifest", at);
    let codec = r.u32()?;
    let raw_len = r.u32()?;
    let body = r.rest();
    if u64::from(raw_len) > MAX_MANIFEST_RAW {
        return Err(IoError::limit("manifest size", raw_len.into(), MAX_MANIFEST_RAW));
    }
    match codec {
        MANIFEST_STORED if body.len() as u64 == u64::from(raw_len) => Ok(Cow::Borrowed(body)),
        MANIFEST_LZ4 if !body.is_empty() && u64::from(raw_len) <= lz4_max_raw(body.len() as u64) => {
            let mut buf = vec![0u8; raw_len as usize];
            let n = lz4_flex::block::decompress_into(body, &mut buf).map_err(|_| IoError::corrupt("manifest lz4 data", at))?;
            if n != buf.len() {
                return Err(IoError::corrupt("manifest decoded length", at));
            }
            Ok(Cow::Owned(buf))
        }
        MANIFEST_STORED | MANIFEST_LZ4 => Err(IoError::corrupt("manifest length", at)),
        _ => Err(IoError::corrupt("manifest codec", at)),
    }
}

/// A thumbnail: `(w, h, RGBA8 straight sRGB)`.
pub type Thumb<'a> = (u16, u16, &'a [u8]);

/// A parsed manifest, borrowing names and opaque bytes from its body.
#[derive(Debug)]
pub struct ManifestView<'a> {
    pub doc: DocFields,
    /// `LAYR` records in file order (pre-order, siblings bottom → top).
    /// Tree rules are the reader's to check.
    pub layers: Vec<LayerRecord<'a>>,
    pub meta: Vec<(String, String)>,
    pub view: Option<&'a [u8]>,
    pub thumb: Option<Thumb<'a>>,
    pub layer_ext: Vec<LayerExt>,
    /// Unknown SAFE_TO_COPY sections.
    pub extras: Vec<AppSection>,
    pub warnings: Vec<LoadWarning>,
    /// Why a resave would lose data, if it would.
    pub lossy: Vec<String>,
}

impl ManifestView<'_> {
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Parse a manifest body. `at` is the manifest record's file offset (used
/// in errors); `max_layers` is the load limit.
pub fn parse(raw: &[u8], at: u64, max_layers: u32) -> Result<ManifestView<'_>, IoError> {
    let bad = |what| IoError::corrupt(what, at);
    let mut r = ByteReader::new(raw, "truncated manifest section", at);
    let mut seen = AHashSet::new();
    let mut doc = None;
    let mut layers = None;
    let mut out = ManifestView {
        doc: DocFields { width: 0, height: 0, dpi: 0, paper: None, active: 0, next_id: 0, layer_count: 0 },
        layers: Vec::new(),
        meta: Vec::new(),
        view: None,
        thumb: None,
        layer_ext: Vec::new(),
        extras: Vec::new(),
        warnings: Vec::new(),
        lossy: Vec::new(),
    };
    let mut extra_total = 0u64;
    while !r.is_empty() {
        if seen.len() >= MAX_SECTIONS {
            return Err(IoError::limit("manifest sections", seen.len().saturating_add(1) as u64, MAX_SECTIONS as u64));
        }
        let tag: [u8; 4] = r.array()?;
        let flags = r.u32()?;
        let len = r.u32()?;
        let body = r.bytes(len as usize)?;
        if !seen.insert(tag) {
            return Err(bad("duplicate manifest section"));
        }
        match tag {
            TAG_DOC => doc = Some(DocFields::decode(body, at)?),
            TAG_LAYR => layers = Some(parse_layers(body, at, max_layers)?),
            TAG_META => out.meta = parse_meta(body, at)?,
            TAG_VIEW => {
                if body.len() > MAX_VIEW_BYTES {
                    return Err(IoError::limit("VIEW section", body.len() as u64, MAX_VIEW_BYTES as u64));
                }
                out.view = Some(body);
            }
            TAG_THUM => out.thumb = parse_thumb(body, at)?,
            TAG_LEXT => {
                out.layer_ext = parse_lext(body, at)?;
                if out.layer_ext.iter().any(|e| e.flags & LEXT_CRITICAL != 0) {
                    out.lossy.push("it has layer data this version cannot read".into());
                }
            }
            TAG_PAGE => {
                let pages = count_pages(body);
                out.warnings.push(LoadWarning::ExtraPagesIgnored(pages));
                out.lossy.push(format!("{pages} more pages were not loaded"));
            }
            _ if flags & SEC_CRITICAL != 0 => return Err(IoError::UnsupportedFeature { tag }),
            _ => {
                out.warnings.push(LoadWarning::SkippedSection { tag });
                if flags & SEC_SAFE_TO_COPY != 0 {
                    extra_total = extra_total.saturating_add(body.len() as u64);
                    if extra_total > MAX_EXTRA_TOTAL {
                        return Err(IoError::limit("unknown sections", extra_total, MAX_EXTRA_TOTAL));
                    }
                    out.extras.push(AppSection { tag, flags, bytes: body.to_vec() });
                } else {
                    out.lossy.push(format!("it has data ({}) this version cannot keep", tag_str(&tag)));
                }
            }
        }
    }
    out.doc = doc.ok_or(bad("missing DOC section"))?;
    out.layers = layers.ok_or(bad("missing LAYR section"))?;
    if out.layers.len() != out.doc.layer_count as usize {
        return Err(bad("layer count does not match DOC"));
    }
    Ok(out)
}

fn parse_layers(body: &[u8], at: u64, max_layers: u32) -> Result<Vec<LayerRecord<'_>>, IoError> {
    let mut r = ByteReader::new(body, "truncated LAYR section", at);
    let count = r.u32()?;
    let limit = max_layers.min(MAX_LAYER_COUNT);
    if count > limit {
        return Err(IoError::limit("layers", count.into(), limit.into()));
    }
    // Each record takes at least 32 bytes, so this bounds the allocation
    // by the section length too.
    if (count as usize).saturating_mul(32) > r.remaining() {
        return Err(IoError::corrupt("truncated LAYR section", at));
    }
    let mut layers = Vec::with_capacity(count as usize);
    for _ in 0..count {
        layers.push(LayerRecord::decode(&mut r)?);
    }
    if !r.is_empty() {
        return Err(IoError::corrupt("LAYR section length", at));
    }
    Ok(layers)
}

fn parse_meta(body: &[u8], at: u64) -> Result<Vec<(String, String)>, IoError> {
    let mut r = ByteReader::new(body, "truncated META section", at);
    let count = usize::from(r.u16()?);
    if count > MAX_META_ENTRIES {
        return Err(IoError::limit("META entries", count as u64, MAX_META_ENTRIES as u64));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let klen = r.u8()?;
        let key = String::from_utf8_lossy(r.bytes(klen.into())?).into_owned();
        let vlen = usize::from(r.u16()?);
        if vlen > MAX_META_VALUE {
            return Err(IoError::limit("META value", vlen as u64, MAX_META_VALUE as u64));
        }
        let val = String::from_utf8_lossy(r.bytes(vlen)?).into_owned();
        out.push((key, val));
    }
    Ok(out)
}

/// `None` for a thumbnail format this version does not know.
fn parse_thumb(body: &[u8], at: u64) -> Result<Option<Thumb<'_>>, IoError> {
    let mut r = ByteReader::new(body, "truncated THUM section", at);
    let (w, h, fmt) = (r.u16()?, r.u16()?, r.u32()?);
    if w > MAX_THUMB_SIDE || h > MAX_THUMB_SIDE {
        return Err(IoError::limit("thumbnail side", w.max(h).into(), MAX_THUMB_SIDE.into()));
    }
    let px = r.bytes(usize::from(w).saturating_mul(h.into()).saturating_mul(4))?;
    Ok((fmt == 0).then_some((w, h, px)))
}

fn parse_lext(body: &[u8], at: u64) -> Result<Vec<LayerExt>, IoError> {
    let mut r = ByteReader::new(body, "truncated LEXT section", at);
    let n = r.u32()?;
    if n > MAX_LEXT_ENTRIES {
        return Err(IoError::limit("LEXT entries", n.into(), MAX_LEXT_ENTRIES.into()));
    }
    let mut total = 0u64;
    let mut out = Vec::new();
    for _ in 0..n {
        let layer = r.u32()?;
        let tag = r.array()?;
        let flags = r.u32()?;
        let len = r.u32()?;
        if u64::from(len) > MAX_LEXT_ENTRY {
            return Err(IoError::limit("LEXT entry", len.into(), MAX_LEXT_ENTRY));
        }
        total = total.saturating_add(len.into());
        if total > MAX_LEXT_TOTAL {
            return Err(IoError::limit("LEXT section", total, MAX_LEXT_TOTAL));
        }
        out.push(LayerExt { layer, tag, flags, bytes: r.bytes(len as usize)?.to_vec() });
    }
    Ok(out)
}

/// Pages in a `PAGE` body: one per nested `DOC ` section (at least one).
fn count_pages(body: &[u8]) -> u32 {
    let mut r = ByteReader::new(body, "PAGE", 0);
    let mut pages = 0u32;
    while let (Ok(tag), Ok(_flags), Ok(len)) = (r.array::<4>(), r.u32(), r.u32()) {
        if r.skip(len as usize).is_err() {
            break;
        }
        if tag == TAG_DOC {
            pages = pages.saturating_add(1);
        }
    }
    pages.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{LAYER_KIND_FOLDER, LAYER_KIND_RASTER, LF_VISIBLE};

    fn layer(id: u32, parent_id: u32, kind: u8, name: &str) -> LayerRecord<'_> {
        LayerRecord {
            id,
            parent_id,
            kind,
            flags: LF_VISIBLE,
            blend: 0,
            opacity_bits: 1f32.to_bits(),
            tile_count: 0,
            table_offset: 0,
            name: name.as_bytes(),
        }
    }

    fn layr(records: &[LayerRecord<'_>]) -> Vec<u8> {
        let mut b = (records.len() as u32).to_le_bytes().to_vec();
        for rec in records {
            rec.encode_into(&mut b);
        }
        b
    }

    fn doc(layer_count: u32) -> DocFields {
        DocFields { width: 100, height: 50, dpi: 350, paper: Some([1, 2, 3, 0x8000]), active: 2, next_id: 9, layer_count }
    }

    fn minimal(extra: impl FnOnce(&mut SectionWriter)) -> Vec<u8> {
        let mut w = SectionWriter::default();
        w.push(TAG_DOC, SEC_CRITICAL, &doc(2).encode());
        w.push(TAG_LAYR, SEC_CRITICAL, &layr(&[layer(1, 0, LAYER_KIND_FOLDER, "F"), layer(2, 1, LAYER_KIND_RASTER, "R")]));
        extra(&mut w);
        w.into_raw()
    }

    #[test]
    fn sections_round_trip() {
        let ext = [
            LayerExt { layer: 2, tag: *b"ABCD", flags: 0, bytes: vec![1, 2, 3] },
            LayerExt { layer: 9, tag: *b"WXYZ", flags: 0, bytes: vec![] },
        ];
        let thumb = [7u8; 2 * 3 * 4];
        let mut th = Vec::new();
        th.extend_from_slice(&2u16.to_le_bytes());
        th.extend_from_slice(&3u16.to_le_bytes());
        th.extend_from_slice(&0u32.to_le_bytes());
        th.extend_from_slice(&thumb);
        let raw = minimal(|w| {
            w.push(TAG_META, 0, &meta_body(&[("app", "ARTY"), ("title", "ページ")]));
            w.push(TAG_VIEW, SEC_SAFE_TO_COPY, &[1, 2, 3, 4]);
            w.push(TAG_THUM, 0, &th);
            w.push(TAG_LEXT, SEC_SAFE_TO_COPY, &lext_body(ext.iter()));
            w.push(*b"ZZZZ", SEC_SAFE_TO_COPY, b"kept");
        });
        let m = parse(&raw, 0, MAX_LAYER_COUNT).unwrap();
        assert_eq!(m.doc, doc(2));
        assert_eq!(m.layers, [layer(1, 0, LAYER_KIND_FOLDER, "F"), layer(2, 1, LAYER_KIND_RASTER, "R")]);
        assert_eq!(m.meta("title"), Some("ページ"));
        assert_eq!(m.view, Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(m.thumb, Some((2, 3, &thumb[..])));
        assert_eq!(m.layer_ext, ext);
        assert_eq!(m.extras, [AppSection { tag: *b"ZZZZ", flags: SEC_SAFE_TO_COPY, bytes: b"kept".to_vec() }]);
        assert_eq!(m.warnings, [LoadWarning::SkippedSection { tag: *b"ZZZZ" }]);
        assert!(m.lossy.is_empty(), "a kept section is not lossy");

        assert_eq!(decode_payload(&encode_payload(&raw), 0).unwrap().as_ref(), raw.as_slice());
    }

    #[test]
    fn large_manifests_compress() {
        let big = minimal(|w| w.push(TAG_VIEW, SEC_SAFE_TO_COPY, &[0u8; 20_000]));
        let p = encode_payload(&big);
        assert_eq!(u32::from_le_bytes(p[..4].try_into().unwrap()), MANIFEST_LZ4);
        assert!(p.len() < 2000);
        assert_eq!(decode_payload(&p, 0).unwrap().as_ref(), big.as_slice());
        let small = minimal(|_| {});
        assert_eq!(encode_payload(&small)[..4], MANIFEST_STORED.to_le_bytes());
    }

    #[test]
    fn unknown_sections_follow_their_flags() {
        let crit = minimal(|w| w.push(*b"VECT", SEC_CRITICAL | SEC_SAFE_TO_COPY, b"x"));
        assert!(matches!(parse(&crit, 0, 100), Err(IoError::UnsupportedFeature { tag }) if tag == *b"VECT"));
        let skip = minimal(|w| w.push(*b"GUID", 0, b"x"));
        let m = parse(&skip, 0, 100).unwrap();
        assert!(m.extras.is_empty() && m.lossy.len() == 1);
        assert_eq!(m.warnings, [LoadWarning::SkippedSection { tag: *b"GUID" }]);
        let lext = minimal(|w| {
            w.push(TAG_LEXT, SEC_SAFE_TO_COPY, &lext_body([LayerExt { layer: 1, tag: *b"TEXT", flags: LEXT_CRITICAL, bytes: vec![] }].iter()))
        });
        assert_eq!(parse(&lext, 0, 100).unwrap().lossy.len(), 1);

        let mut nested = SectionWriter::default();
        nested.push(TAG_DOC, SEC_CRITICAL, &doc(0).encode());
        nested.push(TAG_LAYR, SEC_CRITICAL, &0u32.to_le_bytes());
        let page = minimal(|w| w.push(TAG_PAGE, 0, &nested.into_raw()));
        let m = parse(&page, 0, 100).unwrap();
        assert_eq!(m.warnings, [LoadWarning::ExtraPagesIgnored(1)]);
        assert_eq!(m.lossy.len(), 1);
    }

    #[test]
    fn rejects_bad_manifests() {
        let err = |raw: Vec<u8>| parse(&raw, 0, 100).unwrap_err();
        let dup = minimal(|w| {
            w.push(TAG_META, 0, &meta_body(&[]));
            w.push(TAG_META, 0, &meta_body(&[]));
        });
        assert!(matches!(err(dup), IoError::Corrupt { what: "duplicate manifest section", .. }));
        let mut w = SectionWriter::default();
        w.push(TAG_DOC, 0, &doc(0).encode());
        assert!(matches!(err(w.into_raw()), IoError::Corrupt { what: "missing LAYR section", .. }));
        let mut w = SectionWriter::default();
        w.push(TAG_DOC, 0, &doc(1).encode());
        w.push(TAG_LAYR, 0, &layr(&[]));
        assert!(matches!(err(w.into_raw()), IoError::Corrupt { what: "layer count does not match DOC", .. }));
        let mut w = SectionWriter::default();
        w.push(TAG_DOC, 0, &doc(0).encode()[..39]);
        w.push(TAG_LAYR, 0, &layr(&[]));
        assert!(matches!(err(w.into_raw()), IoError::Corrupt { what: "DOC section length", .. }));
        // A longer DOC (later minor) is fine.
        let mut w = SectionWriter::default();
        w.push(TAG_DOC, 0, &[&doc(0).encode()[..], &[0xEE; 8]].concat());
        w.push(TAG_LAYR, 0, &layr(&[]));
        assert!(parse(&w.into_raw(), 0, 100).is_ok());
        for bad in [
            DocFields { width: 0, ..doc(0) },
            DocFields { height: MAX_PAGE_SIDE + 1, ..doc(0) },
            DocFields { dpi: 0, ..doc(0) },
            DocFields { dpi: MAX_DPI + 1, ..doc(0) },
        ] {
            assert!(DocFields::decode(&bad.encode(), 0).is_err(), "{bad:?}");
        }
        let mut b = doc(0).encode();
        b[12] = 2;
        assert!(matches!(DocFields::decode(&b, 0), Err(IoError::Corrupt { what: "paper mode", .. })));
        b = DocFields { layer_count: 70_000, ..doc(0) }.encode();
        assert!(matches!(DocFields::decode(&b, 0), Err(IoError::LimitExceeded { .. })));
        // LAYR with trailing bytes, or a count larger than the records.
        let mut l = layr(&[layer(1, 0, 0, "a")]);
        l.push(0);
        assert!(parse_layers(&l, 0, 100).is_err());
        let mut l = layr(&[layer(1, 0, 0, "a")]);
        l[..4].copy_from_slice(&50_000u32.to_le_bytes());
        assert!(parse_layers(&l, 0, 100_000).is_err());
        assert!(matches!(parse_layers(&l, 0, 100), Err(IoError::LimitExceeded { .. })));
        // Over-limit optional sections.
        assert!(parse_meta(&[65, 0], 0).is_err());
        assert!(parse_thumb(&[0, 1, 1, 0, 0, 0, 0, 0], 0).is_err());
        assert!(parse_lext(&(MAX_LEXT_ENTRIES + 1).to_le_bytes(), 0).is_err());
        assert!(matches!(err(minimal(|w| w.push(TAG_VIEW, 0, &vec![0; MAX_VIEW_BYTES + 1]))), IoError::LimitExceeded { .. }));
        // Truncated section header and section body.
        let mut raw = minimal(|_| {});
        raw.extend_from_slice(b"ABCD\0\0");
        assert!(parse(&raw, 0, 100).is_err());
        let mut raw = minimal(|w| w.push(*b"ABCD", 0, b"12345"));
        raw.pop();
        assert!(parse(&raw, 0, 100).is_err());
    }

    #[test]
    fn rejects_bad_payloads() {
        let raw = minimal(|_| {});
        let mut p = encode_payload(&raw);
        p.push(0);
        assert!(decode_payload(&p, 0).is_err(), "stored body longer than raw_len");
        let mut bomb = MANIFEST_LZ4.to_le_bytes().to_vec();
        bomb.extend_from_slice(&(16u32 << 20).to_le_bytes());
        bomb.extend_from_slice(&[0; 10]);
        assert!(matches!(decode_payload(&bomb, 0), Err(IoError::Corrupt { what: "manifest length", .. })));
        let mut huge = MANIFEST_STORED.to_le_bytes().to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode_payload(&huge, 0), Err(IoError::LimitExceeded { .. })));
        let mut odd = 7u32.to_le_bytes().to_vec();
        odd.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(decode_payload(&odd, 0), Err(IoError::Corrupt { what: "manifest codec", .. })));
        assert!(decode_payload(&[1, 0, 0], 0).is_err());
        let big = minimal(|w| w.push(TAG_VIEW, 0, &[0u8; 20_000]));
        let mut p = encode_payload(&big);
        let n = p.len();
        p[n - 3] ^= 0x55;
        let _ = decode_payload(&p, 0); // garbage lz4: error or wrong bytes, never a panic
        assert_eq!(truncate_str("aé", 2), "a");
        assert_eq!(truncate_str("abc", 5), "abc");
    }
}
