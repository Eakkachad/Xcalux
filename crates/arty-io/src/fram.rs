//! `FRAM` LEXT entries: frame border panels of a folder (flags 0, so older
//! readers keep them byte for byte).

use arty_core::{Document, Layer};

use crate::error::LoadWarning;
use crate::manifest::LayerExt;

/// One entry per folder whose frame is `Some`.
pub fn encode_all(_doc: &Document) -> Vec<LayerExt> {
    // FRAMES
    Vec::new()
}

/// Attach decoded frames to their folders and remove those entries from
/// `ext`. Entries this version cannot read stay in `ext` (kept for saving).
pub fn apply(_layers: &mut [Layer], _ext: &mut Vec<LayerExt>, _w: u32, _h: u32, _warn: &mut Vec<LoadWarning>) {
    // FRAMES
}
