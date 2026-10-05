//! `FRAM` LEXT entries: frame border panels of a folder (flags 0, so older
//! readers keep them byte for byte).
//!
//! ```text
//! u8 ver=1, u8 flags=0, u16 panel_count ≤ 1024, f32 border_width,
//! [u16; 4] color (fix15 premultiplied),
//! per panel: u8 n (3..=64), u8 0, n × (f32 x, f32 y)
//! ```
//!
//! An entry of another version stays opaque in `layer_ext` (kept for
//! saving). A bad panel is dropped with `FramePanelDropped`; an unreadable
//! entry, one on a raster layer or a missing id, or a second one for the
//! same folder is dropped with `FrameDropped`.

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

use std::sync::Arc;

use ahash::{AHashMap, AHashSet};
use arty_core::frame::{MAX_PANEL_VERTS, MAX_PANELS};
use arty_core::{BorderStyle, Document, Frame, FrameShape, Layer, LayerContent, LayerId, Panel};
use rayon::prelude::*;

use crate::codec::sanitize_pixel;
use crate::error::LoadWarning;
use crate::format::ByteReader;
use crate::manifest::{LEXT_FRAM, LayerExt};

pub const FRAM_VERSION: u8 = 1;
/// Widest border kept (px).
pub const MAX_BORDER_WIDTH: f32 = 1000.0;

/// One entry per folder whose frame is `Some`, in tree order.
pub fn encode_all(doc: &Document) -> Vec<LayerExt> {
    fn walk(doc: &Document, ids: &[LayerId], out: &mut Vec<LayerExt>) {
        for &id in ids {
            if let Some(f) = doc.frame(id) {
                out.push(LayerExt { layer: id.0, tag: LEXT_FRAM, flags: 0, bytes: encode(f.shape()) });
            }
            if let Some(children) = doc.layer(id).and_then(Layer::children) {
                walk(doc, children, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(doc, doc.root(), &mut out);
    out
}

/// The entry body of `s` (at most [`MAX_PANELS`] panels).
pub fn encode(s: &FrameShape) -> Vec<u8> {
    let panels = s.panels.get(..MAX_PANELS).unwrap_or(&s.panels);
    let mut b = Vec::new();
    b.extend_from_slice(&[FRAM_VERSION, 0]);
    b.extend_from_slice(&(panels.len() as u16).to_le_bytes());
    b.extend_from_slice(&s.border.width.to_le_bytes());
    for c in s.border.color {
        b.extend_from_slice(&c.to_le_bytes());
    }
    for p in panels {
        b.extend_from_slice(&[p.points().len() as u8, 0]);
        for [x, y] in p.points() {
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
        }
    }
    b
}

/// Attach decoded frames to their folders and remove those entries from
/// `ext`. Entries this version cannot read stay in `ext` (kept for saving).
pub fn apply(layers: &mut [Layer], ext: &mut Vec<LayerExt>, w: u32, h: u32, warn: &mut Vec<LoadWarning>) {
    if !ext.iter().any(|e| e.tag == LEXT_FRAM) {
        return;
    }
    let index: AHashMap<u32, usize> = layers.iter().enumerate().map(|(i, l)| (l.id.0, i)).collect();
    let mut seen = AHashSet::new();
    let mut decoded: Vec<(usize, FrameShape)> = Vec::new();
    let mut kept = Vec::with_capacity(ext.len());
    for e in ext.drain(..) {
        if e.tag != LEXT_FRAM || e.bytes.first() != Some(&FRAM_VERSION) {
            kept.push(e);
            continue;
        }
        let layer = e.layer;
        let found = match index.get(&layer) {
            None => Err("no such layer"),
            Some(&i) if !layers.get(i).is_some_and(Layer::is_folder) => Err("not a folder"),
            Some(_) if !seen.insert(layer) => Err("a second frame for the folder"),
            Some(&i) => decode(&e.bytes, w, h).map(|d| (i, d)),
        };
        match found {
            Ok((i, (shape, bad))) => {
                if bad > 0 {
                    warn.push(LoadWarning::FramePanelDropped { layer, count: bad });
                }
                decoded.push((i, shape));
            }
            Err(reason) => warn.push(LoadWarning::FrameDropped { layer, reason }),
        }
    }
    *ext = kept;
    let frames: Vec<(usize, Arc<Frame>)> = decoded.into_par_iter().map(|(i, s)| (i, Frame::build(s, w, h))).collect();
    for (i, f) in frames {
        if let Some(Layer { content: LayerContent::Folder { frame, .. }, .. }) = layers.get_mut(i) {
            *frame = Some(f);
        }
    }
}

/// The shape in an entry body and how many of its panels were dropped.
pub fn decode(b: &[u8], w: u32, h: u32) -> Result<(FrameShape, u32), &'static str> {
    let damaged = |_| "damaged";
    let mut r = ByteReader::new(b, "truncated FRAM entry", 0);
    if r.u8().map_err(damaged)? != FRAM_VERSION {
        return Err("unknown version");
    }
    r.skip(1).map_err(damaged)?;
    let count = r.u16().map_err(damaged)? as usize;
    if count > MAX_PANELS {
        return Err("too many panels");
    }
    let width = r.f32().map_err(damaged)?;
    if !(0.0..=MAX_BORDER_WIDTH).contains(&width) {
        return Err("bad border width");
    }
    let mut color = [0u16; 4];
    for c in &mut color {
        *c = r.u16().map_err(damaged)?;
    }
    // Clamp to fix15 and keep it premultiplied.
    let ([r_, g, b_, a], _) = sanitize_pixel(color);
    let color = [r_.min(a), g.min(a), b_.min(a), a];

    // Bleed panels may reach one page past each edge.
    let (fw, fh) = (w as f32, h as f32);
    let in_range = |x: f32, y: f32| (-fw..=2.0 * fw).contains(&x) && (-fh..=2.0 * fh).contains(&y);
    let mut panels = Vec::with_capacity(count);
    let mut bad = 0u32;
    for _ in 0..count {
        let n = r.u8().map_err(damaged)? as usize;
        r.skip(1).map_err(damaged)?;
        let mut pts = Vec::with_capacity(n);
        let mut ok = (3..=MAX_PANEL_VERTS).contains(&n);
        for _ in 0..n {
            let (x, y) = (r.f32().map_err(damaged)?, r.f32().map_err(damaged)?);
            ok &= in_range(x, y);
            pts.push([x, y]);
        }
        match Panel::new(pts).filter(|_| ok) {
            Some(p) => panels.push(p),
            None => bad = bad.saturating_add(1),
        }
    }
    Ok((FrameShape { panels, border: BorderStyle { width, color } }, bad))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arty_core::RectF;

    #[test]
    fn entry_layout() {
        let p = Panel::rect(RectF { x: 1.0, y: 2.0, w: 30.0, h: 40.0 }).unwrap();
        let s = FrameShape { panels: vec![p], border: BorderStyle { width: 3.5, color: [1, 2, 3, 4] } };
        let b = encode(&s);
        assert_eq!(b.len(), 16 + 2 + 4 * 8);
        assert_eq!(&b[..4], &[1, 0, 1, 0]);
        assert_eq!(&b[4..8], &3.5f32.to_le_bytes());
        assert_eq!(&b[8..16], &[1, 0, 2, 0, 3, 0, 4, 0]);
        assert_eq!(&b[16..18], &[4, 0]);
        assert_eq!(decode(&b, 100, 100), Ok((s, 0)));
        assert_eq!(decode(&b[..b.len() - 1], 100, 100), Err("damaged"));
    }
}
