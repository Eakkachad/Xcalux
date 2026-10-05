//! The `PSET` section: the page setup (SAFE_TO_COPY, 44-byte body; later
//! minor versions may append bytes).

use arty_core::PageSetup;

use crate::error::LoadWarning;

/// The `PSET` body, when the document has a page setup.
pub fn encode(_p: Option<&PageSetup>) -> Option<Vec<u8>> {
    // FRAMES
    None
}

/// The page setup in a `PSET` body, sanitized for a `w`×`h` page; `None`
/// (with `PageSetupDropped`) when invalid.
pub fn decode(_b: &[u8], _w: u32, _h: u32, _warn: &mut Vec<LoadWarning>) -> Option<PageSetup> {
    // FRAMES
    None
}
