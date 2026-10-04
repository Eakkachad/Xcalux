//! Builds v2 files record by record, for handcrafted and future-version
//! files the writer never produces.

use arty_io::format::{Commit, Header, LayerRecord, RecordHeader, RecordKind, TileEntry};
use arty_io::manifest::{DocFields, SEC_CRITICAL, SectionWriter, TAG_DOC, TAG_LAYR, encode_payload};

use super::UUID;

pub struct Raw(pub Vec<u8>);

impl Raw {
    pub fn new() -> Self {
        Raw(Header::new(0, UUID).encode().to_vec())
    }

    pub fn record(&mut self, kind: RecordKind, payload: &[u8]) -> u64 {
        let at = self.0.len() as u64;
        self.0.extend_from_slice(&RecordHeader::for_payload(kind, payload).encode());
        self.0.extend_from_slice(payload);
        at
    }

    /// A Segment with one blob; returns the blob's offset.
    pub fn blob(&mut self, bytes: &[u8]) -> u64 {
        self.record(RecordKind::Segment, bytes) + 24
    }

    /// A stored table, entries as given (no sorting).
    pub fn table(&mut self, layer: u32, entries: &[TileEntry]) -> u64 {
        self.table_sized(layer, entries, 32)
    }

    /// A stored table whose entries are `entry_size` bytes: the 32 known
    /// ones, then zeros (as a later minor version may write).
    pub fn table_sized(&mut self, layer: u32, entries: &[TileEntry], entry_size: u16) -> u64 {
        let mut p = Vec::new();
        p.extend_from_slice(&layer.to_le_bytes());
        p.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        p.extend_from_slice(&entry_size.to_le_bytes());
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&(entries.len() as u32 * u32::from(entry_size)).to_le_bytes());
        for e in entries {
            p.extend_from_slice(&e.encode());
            p.resize(p.len() + usize::from(entry_size) - 32, 0);
        }
        self.record(RecordKind::TileTable, &p)
    }

    pub fn manifest_raw(mut self, raw: &[u8]) -> Vec<u8> {
        let m = self.record(RecordKind::Manifest, &encode_payload(raw));
        self.0.extend_from_slice(&Commit { manifest_offset: m, prev_commit_offset: 0, commit_seq: 1, unix_ms: 0 }.encode_record());
        self.0
    }

    pub fn finish(self, doc: DocFields, layers: &[LayerRecord<'_>], extra: impl FnOnce(&mut SectionWriter)) -> Vec<u8> {
        let mut w = SectionWriter::default();
        w.push(TAG_DOC, SEC_CRITICAL, &doc.encode());
        let mut l = (layers.len() as u32).to_le_bytes().to_vec();
        for r in layers {
            r.encode_into(&mut l);
        }
        w.push(TAG_LAYR, SEC_CRITICAL, &l);
        extra(&mut w);
        self.manifest_raw(&w.into_raw())
    }
}
