//! `REFL` LEXT entries: reference layers (empty body, flags 0).

use arty_core::{Document, Layer};

use crate::error::LoadWarning;
use crate::manifest::LayerExt;

/// One entry per layer with `props.reference` set.
pub fn encode_all(_doc: &Document) -> Vec<LayerExt> {
    // FILL
    Vec::new()
}

/// Set `props.reference` from the entries and remove them from `ext`.
pub fn apply(_layers: &mut [Layer], _ext: &mut Vec<LayerExt>, _warn: &mut Vec<LoadWarning>) {
    // FILL
}
