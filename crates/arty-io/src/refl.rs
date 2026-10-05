//! `REFL` LEXT entries: reference layers (empty body, flags 0).
//!
//! The flag lives in LEXT rather than a LAYR bit because a v2.0 reader
//! drops unknown LAYR bits but keeps LEXT entries byte for byte.

use ahash::AHashMap;
use arty_core::{Document, Layer};

use crate::error::LoadWarning;
use crate::manifest::{LEXT_REFL, LayerExt};

/// One entry per layer with `props.reference` set.
pub fn encode_all(doc: &Document) -> Vec<LayerExt> {
    let mut ids: Vec<u32> = doc_layers(doc).filter(|l| l.props.reference).map(|l| l.id.0).collect();
    // Layer map order is unspecified; keep saves deterministic.
    ids.sort_unstable();
    ids.into_iter().map(|layer| LayerExt { layer, tag: LEXT_REFL, flags: 0, bytes: Vec::new() }).collect()
}

/// Set `props.reference` from the entries and remove them from `ext`.
/// Entries for ids with no layer stay (the writer's live-id filter drops
/// them on the next save). A body is ignored: later versions may append
/// data, but the flag itself is what this version understands.
pub fn apply(layers: &mut [Layer], ext: &mut Vec<LayerExt>, _warn: &mut Vec<LoadWarning>) {
    if !ext.iter().any(|e| e.tag == LEXT_REFL) {
        return;
    }
    let index: AHashMap<u32, usize> = layers.iter().enumerate().map(|(i, l)| (l.id.0, i)).collect();
    ext.retain(|e| {
        if e.tag != LEXT_REFL {
            return true;
        }
        match index.get(&e.layer) {
            Some(&i) => {
                layers[i].props.reference = true;
                false
            }
            None => true,
        }
    });
}

fn doc_layers(doc: &Document) -> impl Iterator<Item = &Layer> {
    let mut stack: Vec<_> = doc.root().to_vec();
    std::iter::from_fn(move || {
        let id = stack.pop()?;
        let l = doc.layer(id)?;
        if let Some(c) = l.children() {
            stack.extend_from_slice(c);
        }
        Some(l)
    })
}
