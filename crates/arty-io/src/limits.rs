//! Format caps (fixed by the spec) and per-load limits (chosen by the app).
//!
//! Every count or length read from a file is checked against these before
//! anything is allocated from it.

const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// A Segment record holds at most this many blobs (writer cap).
pub const MAX_SEGMENT_BLOBS: usize = 1024;
/// A Segment record holds at most this many payload bytes (writer cap).
pub const MAX_SEGMENT_BYTES: u64 = 32 * MIB;

/// Decompressed manifest size.
pub const MAX_MANIFEST_RAW: u64 = 16 * MIB;
/// Manifest record payload size (codec + raw_len + body).
pub const MAX_MANIFEST_STORED: u64 = 16 * MIB + 8;
/// Sections per manifest.
pub const MAX_SECTIONS: usize = 1024;

/// Entries in one TileTable.
pub const MAX_TABLE_ENTRIES: u32 = 1_048_576;
pub const MIN_ENTRY_SIZE: u16 = 32;
pub const MAX_ENTRY_SIZE: u16 = 256;

pub const MAX_PAGE_SIDE: u32 = 65_536;
pub const MAX_DPI: u32 = 10_000;
/// Layers per document (same as `arty_core::MAX_LAYERS`).
pub const MAX_LAYER_COUNT: u32 = arty_core::MAX_LAYERS as u32;
/// Layer name bytes.
pub const MAX_NAME_LEN: usize = 4096;

/// Commits followed back by `fallback_to_previous`.
pub const MAX_FALLBACK_COMMITS: usize = 16;
/// Buffer of the backward magic scan.
pub const SCAN_WINDOW: usize = MIB as usize;

pub const MAX_META_ENTRIES: usize = 64;
pub const MAX_META_VALUE: usize = 4096;
pub const MAX_VIEW_BYTES: usize = 64 * KIB as usize;
pub const MAX_THUMB_SIDE: u16 = 256;
pub const MAX_LEXT_ENTRIES: u32 = 4096;
pub const MAX_LEXT_ENTRY: u64 = MIB;
pub const MAX_LEXT_TOTAL: u64 = 16 * MIB;
/// Unknown SAFE_TO_COPY sections kept for re-saving, in total.
pub const MAX_EXTRA_TOTAL: u64 = 16 * MIB;
/// Encoded `SELM` (selection) body. Larger selections are binarized, then
/// left out of the file.
pub const MAX_SELM_BYTES: usize = 8 << 20;
/// `PSET` (page setup) body.
pub const MAX_PSET_BYTES: usize = 4096;

/// Largest output an lz4 block of `stored` bytes can decode to
/// (255·stored + 64). Anything claiming more is a decompression bomb.
pub const fn lz4_max_raw(stored: u64) -> u64 {
    stored.saturating_mul(255).saturating_add(64)
}

/// Limits for one load. Exceeding one gives `IoError::LimitExceeded`, which
/// the app can retry with a higher limit after asking the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadLimits {
    /// Decoded pixel memory (32 KiB per unique tile).
    pub max_decoded_bytes: u64,
    /// Tile entries over all tables.
    pub max_entries: u64,
    pub max_layers: u32,
}

impl Default for LoadLimits {
    /// The app lowers `max_decoded_bytes` to 75% of physical RAM.
    fn default() -> Self {
        Self { max_decoded_bytes: 16 * GIB, max_entries: 8_388_608, max_layers: MAX_LAYER_COUNT }
    }
}
