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

pub mod codec;
pub mod error;
pub mod format;
pub mod limits;
pub mod names;
pub mod readat;
pub mod sink;

pub use codec::{BlobCodec, CodecScratch, TileClass};
pub use error::{IoError, LoadWarning};
pub use format::FileIdentity;
pub use limits::LoadLimits;
pub use readat::ReadAt;
pub use sink::Sink;

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
