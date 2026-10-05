//! The `SELM` section: the document's selection (SAFE_TO_COPY).
//!
//! An unreadable selection is dropped with a warning; it never fails the
//! load or makes it lossy.

use std::sync::Arc;

use arty_core::Selection;

use crate::error::LoadWarning;

/// The last encoding, keyed by `Document::selection_rev` (autosave rewrites
/// the manifest on every commit).
#[derive(Default)]
pub struct SelmCache {
    // SEL-CORE
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
pub fn encode(_sel: &Selection, _rev: u64, _w: u32, _h: u32, _c: &mut SelmCache) -> (Option<Arc<[u8]>>, SelectionSave) {
    // SEL-CORE
    (None, SelectionSave::None)
}

/// The selection in a `SELM` body, or `None` (with a warning) when it does
/// not fit a `w`×`h` page or is damaged.
pub fn decode(_b: &[u8], _w: u32, _h: u32, _warn: &mut Vec<LoadWarning>) -> Option<Selection> {
    // SEL-CORE
    None
}
