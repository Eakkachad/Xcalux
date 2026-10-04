//! Per-tile flattening of the layer tree.
//!
//! Only dirty tiles are recomposited, each independently, so callers can
//! fan tiles out across threads (one [`CompositeScratch`] per worker).
//! Steady-state compositing performs no heap allocation.

use crate::blend::{BlendMode, blend_tile, blend_tile_atop, lerp_tile};
use crate::document::Document;
use crate::layer::{Layer, LayerContent, LayerId};
use crate::tile::{TileCoord, TilePixels, clear_tile, fill_tile, new_tile_box};

/// Per-thread temporary tiles for isolated groups (folders, clip groups).
pub struct CompositeScratch {
    bufs: Vec<Box<TilePixels>>,
}

impl Default for CompositeScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeScratch {
    pub fn new() -> Self {
        let mut s = Self { bufs: Vec::new() };
        s.reserve_depth(3);
        s
    }

    /// Make room for a tree `depth` levels deep. Each level may hold a clip
    /// group and an isolated folder at once.
    pub fn reserve_depth(&mut self, depth: usize) {
        let need = depth * 2 + 2;
        while self.bufs.len() < need {
            self.bufs.push(new_tile_box());
        }
    }
}

impl Document {
    /// Flatten all visible layers at tile `c` into `out` (paper included).
    pub fn composite_tile(&self, c: TileCoord, out: &mut TilePixels, scratch: &mut CompositeScratch) {
        scratch.reserve_depth(self.tree_depth());
        match self.paper() {
            Some(p) => fill_tile(out, p),
            None => clear_tile(out),
        }
        self.composite_stack(&self.root, c, out, &mut scratch.bufs);
    }

    fn composite_stack(&self, ids: &[LayerId], c: TileCoord, dst: &mut TilePixels, scratch: &mut [Box<TilePixels>]) {
        let mut i = 0;
        while i < ids.len() {
            let base = &self.layers[&ids[i]];
            // Clipping layers directly above `base` form its clip group.
            let mut end = i + 1;
            while end < ids.len() && self.layers[&ids[end]].props.clip {
                end += 1;
            }
            if base.props.visible {
                let clips = &ids[i + 1..end];
                if clips.iter().any(|id| self.contributes(&self.layers[id], c)) {
                    self.composite_clip_group(base, clips, c, dst, scratch);
                } else {
                    self.composite_layer(base, c, dst, scratch);
                }
            }
            i = end;
        }
    }

    /// Whether a layer could change pixels at `c`.
    fn contributes(&self, layer: &Layer, c: TileCoord) -> bool {
        layer.props.visible
            && layer.props.opacity > 0.0
            && match &layer.content {
                LayerContent::Raster(g) => g.get(c).is_some(),
                LayerContent::Folder { children, .. } => {
                    children.iter().any(|id| self.contributes(&self.layers[id], c))
                }
            }
    }

    fn composite_layer(&self, layer: &Layer, c: TileCoord, dst: &mut TilePixels, scratch: &mut [Box<TilePixels>]) {
        let op = layer.props.opacity;
        match &layer.content {
            LayerContent::Raster(grid) => {
                if let Some(t) = grid.get(c) {
                    blend_tile(dst, t, op, layer.props.blend);
                }
            }
            LayerContent::Folder { children, .. } => {
                if children.is_empty() || op <= 0.0 {
                    return;
                }
                if layer.props.blend == BlendMode::PassThrough {
                    if op >= 1.0 {
                        self.composite_stack(children, c, dst, scratch);
                    } else {
                        let (tmp, rest) = scratch.split_first_mut().expect("scratch depth");
                        **tmp = *dst;
                        self.composite_stack(children, c, tmp, rest);
                        lerp_tile(dst, tmp, op);
                    }
                } else {
                    let (tmp, rest) = scratch.split_first_mut().expect("scratch depth");
                    clear_tile(tmp);
                    self.composite_stack(children, c, tmp, rest);
                    blend_tile(dst, tmp, op, layer.props.blend);
                }
            }
        }
    }

    /// Render `layer` alone into `out` (cleared first). Returns `false` when
    /// it has nothing at this tile.
    fn render_isolated(&self, layer: &Layer, c: TileCoord, out: &mut TilePixels, scratch: &mut [Box<TilePixels>]) -> bool {
        match &layer.content {
            LayerContent::Raster(grid) => match grid.get(c) {
                Some(t) => {
                    *out = *t;
                    true
                }
                None => false,
            },
            LayerContent::Folder { children, .. } => {
                clear_tile(out);
                self.composite_stack(children, c, out, scratch);
                true
            }
        }
    }

    fn composite_clip_group(
        &self,
        base: &Layer,
        clips: &[LayerId],
        c: TileCoord,
        dst: &mut TilePixels,
        scratch: &mut [Box<TilePixels>],
    ) {
        let (group, rest) = scratch.split_first_mut().expect("scratch depth");
        if !self.render_isolated(base, c, group, rest) {
            return; // nothing to clip to
        }
        for id in clips {
            let clip = &self.layers[id];
            if !clip.props.visible {
                continue;
            }
            match &clip.content {
                LayerContent::Raster(g) => {
                    if let Some(t) = g.get(c) {
                        blend_tile_atop(group, t, clip.props.opacity, clip.props.blend);
                    }
                }
                LayerContent::Folder { .. } => {
                    let (tmp, rest2) = rest.split_first_mut().expect("scratch depth");
                    self.render_isolated(clip, c, tmp, rest2);
                    let mode = match clip.props.blend {
                        BlendMode::PassThrough => BlendMode::Normal,
                        m => m,
                    };
                    blend_tile_atop(group, tmp, clip.props.opacity, mode);
                }
            }
        }
        let mode = match base.props.blend {
            BlendMode::PassThrough => BlendMode::Normal,
            m => m,
        };
        blend_tile(dst, group, base.props.opacity, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fix15::ONE_U16 as O;
    use crate::layer::LayerProps;

    const RED: [u16; 4] = [O, 0, 0, O];
    const BLUE: [u16; 4] = [0, 0, O, O];

    fn paint(doc: &mut Document, id: LayerId, x: usize, color: [u16; 4]) {
        doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0))[0][x] = color;
    }

    fn props(doc: &Document, id: LayerId) -> LayerProps {
        doc.layer(id).unwrap().props.clone()
    }

    fn flatten(doc: &Document) -> Box<TilePixels> {
        let mut out = new_tile_box();
        doc.composite_tile(TileCoord::new(0, 0), &mut out, &mut CompositeScratch::new());
        out
    }

    #[test]
    fn empty_document_is_paper() {
        let doc = Document::new(64, 64, 72);
        assert_eq!(flatten(&doc)[10][10], [O; 4]);
    }

    #[test]
    fn upper_layer_covers_lower() {
        let mut doc = Document::new(64, 64, 72);
        let a = doc.active();
        let b = doc.add_raster_layer();
        paint(&mut doc, a, 0, RED);
        paint(&mut doc, b, 0, BLUE);
        paint(&mut doc, a, 1, RED);
        let out = flatten(&doc);
        assert_eq!(out[0][0], BLUE);
        assert_eq!(out[0][1], RED);
    }

    #[test]
    fn hidden_layer_and_its_clips_are_skipped() {
        let mut doc = Document::new(64, 64, 72);
        let base = doc.active();
        let clip = doc.add_raster_layer();
        paint(&mut doc, base, 0, RED);
        paint(&mut doc, clip, 0, BLUE);
        let mut p = props(&doc, clip);
        p.clip = true;
        doc.set_props(clip, p);
        let mut p = props(&doc, base);
        p.visible = false;
        doc.set_props(base, p);
        assert_eq!(flatten(&doc)[0][0], [O; 4]);
    }

    #[test]
    fn clipping_masks_to_base_alpha() {
        let mut doc = Document::new(64, 64, 72);
        let base = doc.active();
        let clip = doc.add_raster_layer();
        paint(&mut doc, base, 0, RED); // base only at x=0
        paint(&mut doc, clip, 0, BLUE);
        paint(&mut doc, clip, 1, BLUE); // outside base
        let mut p = props(&doc, clip);
        p.clip = true;
        doc.set_props(clip, p);
        let out = flatten(&doc);
        assert_eq!(out[0][0], BLUE);
        assert_eq!(out[0][1], [O; 4], "clip must not spill outside base");
    }

    #[test]
    fn isolated_folder_opacity_applies_to_group() {
        let mut doc = Document::new(64, 64, 72);
        let folder = doc.add_folder();
        let mut p = props(&doc, folder);
        p.blend = BlendMode::Normal;
        p.opacity = 0.5;
        doc.set_props(folder, p);
        let inner = doc.add_raster_layer();
        doc.move_layer(inner, Some(folder), 0);
        paint(&mut doc, inner, 0, [0, 0, 0, O]);
        let out = flatten(&doc);
        let half = O / 2;
        assert!((out[0][0][0] as i32 - half as i32).abs() <= 2, "{:?}", out[0][0]);
    }

    #[test]
    fn pass_through_folder_matches_flat_stack() {
        let mut doc = Document::new(64, 64, 72);
        let bottom = doc.active();
        paint(&mut doc, bottom, 0, RED);
        let folder = doc.add_folder();
        let inner = doc.add_raster_layer();
        doc.move_layer(inner, Some(folder), 0);
        paint(&mut doc, inner, 0, [0, 0, O / 2, O / 2]);
        let mut p = props(&doc, inner);
        p.blend = BlendMode::Multiply;
        doc.set_props(inner, p);
        let nested = flatten(&doc)[0][0];

        let mut flat = Document::new(64, 64, 72);
        let b = flat.active();
        paint(&mut flat, b, 0, RED);
        let top = flat.add_raster_layer();
        paint(&mut flat, top, 0, [0, 0, O / 2, O / 2]);
        let mut p = props(&flat, top);
        p.blend = BlendMode::Multiply;
        flat.set_props(top, p);
        assert_eq!(nested, flatten(&flat)[0][0]);
    }
}
