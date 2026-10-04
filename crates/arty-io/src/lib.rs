//! `.arty` v2 files: a 64-byte header, then append-only CRC-framed records
//! (tile Segments, TileTables, a Manifest, and a Commit that is always
//! last). The newest valid Commit is the document.
//!
//! This crate has no UI dependencies. Parsing never trusts the file: every
//! length is checked against [`limits`] before allocating, and the parsing
//! modules deny indexing, unwraps and unchecked arithmetic.

#![forbid(unsafe_code)]

#[cfg(target_endian = "big")]
compile_error!("arty-io reads and writes tiles as little-endian bytes in place");

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

pub mod codec;
pub mod error;
pub mod format;
mod index;
#[cfg(feature = "legacy")]
pub mod legacy;
pub mod limits;
pub mod manifest;
pub mod names;
pub mod readat;
pub mod reader;
pub mod sink;
pub mod table;
pub mod writer;

pub use codec::{BlobCodec, CodecScratch, TileClass};
pub use error::{IoError, LoadWarning};
pub use format::FileIdentity;
#[cfg(feature = "legacy")]
pub use legacy::import_v1;
pub use limits::LoadLimits;
pub use manifest::{AppSection, LayerExt};
pub use readat::ReadAt;
pub use reader::{FileInfo, LoadOptions, Loaded, load, load_from, read_info};
pub use sink::Sink;
pub use writer::{CommitMeta, Compaction, FileWriter, SaveExtras, SaveOptions, SaveStats, Session, SessionId, Verify};

/// Values of [`Progress::phase`].
pub mod phase {
    pub const IDLE: u8 = 0;
    /// Reading the manifest and tile tables.
    pub const READ: u8 = 1;
    pub const DECODE: u8 = 2;
    /// Classifying tiles and planning blobs.
    pub const PLAN: u8 = 3;
    pub const ENCODE: u8 = 4;
    pub const VERIFY: u8 = 5;
}

/// Progress of the running load or save, shared with the UI. `done` and
/// `total` count tiles in the current phase.
#[derive(Debug, Default)]
pub struct Progress {
    pub phase: AtomicU8,
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// Set by the UI; loads stop at the next tile run, saves at the next
    /// batch, with `IoError::Cancelled`.
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn begin(&self, phase: u8, total: u64) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.phase.store(phase, Ordering::Relaxed);
    }

    pub fn advance(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// What the first bytes of a file say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    V2 { minor: u32 },
    /// A v1 file (ARTY 0.1), imported read-only.
    LegacyV1,
    /// An `.arty` file from a newer major version.
    Newer { major: u32 },
    Unknown,
}

/// Identify a file from its first bytes (12 are enough).
pub fn sniff(head: &[u8]) -> FileKind {
    let word = |at: usize| head.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    if head.get(..4) != Some(&format::MAGIC[..]) {
        return FileKind::Unknown;
    }
    match word(4) {
        Some(1) => FileKind::LegacyV1,
        Some(format::FORMAT_MAJOR) => FileKind::V2 { minor: word(8).unwrap_or(0) },
        Some(major) if major > format::FORMAT_MAJOR => FileKind::Newer { major },
        _ => FileKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_kinds() {
        let v2 = format::Header::new(0, [0; 16]).encode();
        assert_eq!(sniff(&v2), FileKind::V2 { minor: 0 });
        assert_eq!(sniff(b"ARTY\x01\0\0\0\x18\0\0\0\0\0\0\0"), FileKind::LegacyV1);
        assert_eq!(sniff(b"ARTY\x02\0\0\0\x09\0\0\0"), FileKind::V2 { minor: 9 });
        assert_eq!(sniff(b"ARTY\x03\0\0\0"), FileKind::Newer { major: 3 });
        assert_eq!(sniff(b"ARTY\0\0\0\0"), FileKind::Unknown);
        assert_eq!(sniff(b"ARTY\x02"), FileKind::Unknown);
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n"), FileKind::Unknown);
        assert_eq!(sniff(b""), FileKind::Unknown);
    }
}
