//! Per-tile flattening of the layer tree.
//!
//! Only dirty tiles are recomposited, each independently, so callers can
//! fan tiles out across threads (one [`CompositeScratch`] per worker).
//! Steady-state compositing performs no heap allocation.
//!
//! Clipping: a layer with clipping layers stacked on it renders as an
//! isolated group. A pass-through folder used as a clip base is therefore
//! composited like a Normal folder (as CSP and Photoshop do), in every tile.
//! A pass-through *clipping* folder stays pass-through: its children blend
//! against the clip group they are clipped to.

use crate::blend::{BlendMode, blend_tile, blend_tile_atop, lerp_tile};
use crate::document::Document;
use crate::fix15::{self, ONE, ONE_U16};
use crate::frame::{Cov, copy_unmasked, mask_into, mask_tile, mask_toward, mostly_full, over_color};
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
                // Whether a pass-through folder is isolated must not depend
                // on the tile. Other bases look the same with or without the
                // group, so skip it where no clip has pixels.
                let pass_through = base.is_folder() && base.props.blend == BlendMode::PassThrough;
                if !clips.is_empty()
                    && (pass_through || clips.iter().any(|id| self.contributes(&self.layers[id], c)))
                {
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
                LayerContent::Folder { children, frame: None, .. } => {
                    children.iter().any(|id| self.contributes(&self.layers[id], c))
                }
                LayerContent::Folder { children, frame: Some(f), .. } => {
                    (f.content(c) != Cov::None && children.iter().any(|id| self.contributes(&self.layers[id], c)))
                        || f.border(c) != Cov::None
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
            LayerContent::Folder { children, frame: Some(f), .. } => {
                let (cov, line) = (f.content(c), f.border(c));
                if op <= 0.0 || (cov == Cov::None && line == Cov::None) {
                    return;
                }
                let pass_through = layer.props.blend == BlendMode::PassThrough;
                let (tmp, rest) = scratch.split_first_mut().expect("scratch depth");
                let has_children = cov != Cov::None && !children.is_empty();
                if pass_through && op >= 1.0 {
                    // Straight into `dst`; a partial tile keeps the backdrop
                    // where the mask is not full, or (mostly outside the
                    // panels) composites aside and blends in only inside.
                    if has_children {
                        if let Cov::Partial(m) = cov {
                            if mostly_full(m) {
                                copy_unmasked(tmp, dst, m);
                                self.composite_stack(children, c, dst, rest);
                                mask_toward(dst, tmp, m);
                            } else {
                                **tmp = *dst;
                                self.composite_stack(children, c, tmp, rest);
                                mask_into(dst, tmp, m);
                            }
                        } else {
                            self.composite_stack(children, c, dst, rest);
                        }
                    }
                    over_color(dst, f.shape().border.color, line);
                    return;
                }
                if pass_through {
                    **tmp = *dst;
                } else {
                    clear_tile(tmp);
                }
                if has_children {
                    self.composite_stack(children, c, tmp, rest);
                    if let Cov::Partial(m) = cov {
                        if pass_through { mask_toward(tmp, dst, m) } else { mask_tile(tmp, m) }
                    }
                }
                over_color(tmp, f.shape().border.color, line);
                if !pass_through {
                    blend_tile(dst, tmp, op, layer.props.blend);
                } else if op >= 1.0 {
                    *dst = **tmp;
                } else {
                    lerp_tile(dst, tmp, op);
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
            // Clips to a frame folder show inside its panels and border.
            LayerContent::Folder { children, frame: Some(f), .. } => {
                let (cov, line) = (f.content(c), f.border(c));
                if cov == Cov::None && line == Cov::None {
                    return false;
                }
                clear_tile(out);
                if cov != Cov::None {
                    self.composite_stack(children, c, out, scratch);
                    if let Cov::Partial(m) = cov {
                        mask_tile(out, m);
                    }
                }
                over_color(out, f.shape().border.color, line);
                true
            }
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
                // A frame folder clip renders isolated (its mask needs a
                // group of its own).
                LayerContent::Folder { children, frame: None, .. } if clip.props.blend == BlendMode::PassThrough => {
                    let op = clip.props.opacity;
                    if children.is_empty() || op <= 0.0 {
                        continue;
                    }
                    // The children blend against the group itself. Since
                    // atop(s, d) = αd·over(s, d/αd), composite them over the
                    // un-premultiplied (opaque) group, then restore its alpha.
                    let (tmp, rest2) = rest.split_first_mut().expect("scratch depth");
                    unpremultiply_opaque(tmp, group);
                    self.composite_stack(children, c, tmp, rest2);
                    premultiply_by_alpha(tmp, group);
                    if op >= 1.0 {
                        **group = **tmp;
                    } else {
                        lerp_tile(group, tmp, op);
                    }
                }
                LayerContent::Folder { .. } => {
                    let (tmp, rest2) = rest.split_first_mut().expect("scratch depth");
                    if self.render_isolated(clip, c, tmp, rest2) {
                        blend_tile_atop(group, tmp, clip.props.opacity, clip.props.blend);
                    }
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

/// `out = src / αsrc` with alpha 1; fully transparent pixels become
/// transparent black.
fn unpremultiply_opaque(out: &mut TilePixels, src: &TilePixels) {
    for (o, s) in out.as_flattened_mut().iter_mut().zip(src.as_flattened()) {
        let a = s[3] as u32;
        *o = if a == 0 {
            [0; 4]
        } else {
            let un = |v: u16| ((v as u32 * ONE + a / 2) / a).min(ONE) as u16;
            [un(s[0]), un(s[1]), un(s[2]), ONE_U16]
        };
    }
}

/// Scale `px` to the coverage of `alpha`: `rgb·αalpha`, alpha = `αalpha`.
fn premultiply_by_alpha(px: &mut TilePixels, alpha: &TilePixels) {
    for (p, m) in px.as_flattened_mut().iter_mut().zip(alpha.as_flattened()) {
        let a = m[3] as u32;
        for v in &mut p[..3] {
            *v = fix15::mul(*v as u32, a) as u16;
        }
        p[3] = m[3];
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
        let b = doc.add_raster_layer().unwrap();
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
        let clip = doc.add_raster_layer().unwrap();
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
        let clip = doc.add_raster_layer().unwrap();
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
        let folder = doc.add_folder().unwrap();
        let mut p = props(&doc, folder);
        p.blend = BlendMode::Normal;
        p.opacity = 0.5;
        doc.set_props(folder, p);
        let inner = doc.add_raster_layer().unwrap();
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
        let folder = doc.add_folder().unwrap();
        let inner = doc.add_raster_layer().unwrap();
        doc.move_layer(inner, Some(folder), 0);
        paint(&mut doc, inner, 0, [0, 0, O / 2, O / 2]);
        let mut p = props(&doc, inner);
        p.blend = BlendMode::Multiply;
        doc.set_props(inner, p);
        let nested = flatten(&doc)[0][0];

        let mut flat = Document::new(64, 64, 72);
        let b = flat.active();
        paint(&mut flat, b, 0, RED);
        let top = flat.add_raster_layer().unwrap();
        paint(&mut flat, top, 0, [0, 0, O / 2, O / 2]);
        let mut p = props(&flat, top);
        p.blend = BlendMode::Multiply;
        flat.set_props(top, p);
        assert_eq!(nested, flatten(&flat)[0][0]);
    }

    const GRAY: [u16; 4] = [O / 2, O / 2, O / 2, O];

    fn fill(doc: &mut Document, id: LayerId, x: i32, color: [u16; 4]) {
        fill_tile(doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(x, 0)), color);
    }

    fn edit(doc: &mut Document, id: LayerId, f: impl FnOnce(&mut LayerProps)) {
        let mut p = props(doc, id);
        f(&mut p);
        doc.set_props(id, p);
    }

    fn render(doc: &Document, x: i32) -> Box<TilePixels> {
        let mut out = new_tile_box();
        doc.composite_tile(TileCoord::new(x, 0), &mut out, &mut CompositeScratch::new());
        out
    }

    /// `[red, folder{multiply gray}]` over two tiles; returns the folder.
    fn red_under_multiply_folder(doc: &mut Document) -> LayerId {
        let bottom = doc.active();
        let folder = doc.add_folder().unwrap();
        let inner = doc.add_raster_layer().unwrap();
        doc.move_layer(inner, Some(folder), 0);
        edit(doc, inner, |p| p.blend = BlendMode::Multiply);
        for x in 0..2 {
            fill(doc, bottom, x, RED);
            fill(doc, inner, x, GRAY);
        }
        doc.set_active(folder);
        folder
    }

    #[test]
    fn clipped_pass_through_folder_renders_the_same_in_every_tile() {
        let mut doc = Document::new(128, 64, 72);
        red_under_multiply_folder(&mut doc);
        // A clip layer whose only tile, at x = 0, is fully erased.
        let clip = doc.add_raster_layer().unwrap();
        edit(&mut doc, clip, |p| p.clip = true);
        fill(&mut doc, clip, 0, [0; 4]);
        let (with_clip_tile, without) = (render(&doc, 0), render(&doc, 1));
        assert_eq!(with_clip_tile, without);
        // Rule: the clip base is isolated, i.e. the multiply child is drawn
        // over transparency and lands as plain gray.
        assert_eq!(without[5][5], GRAY);
    }

    #[test]
    fn pass_through_clip_folder_blends_children_against_base() {
        for opacity in [1.0, 0.5] {
            // base, clip folder { multiply gray }
            let mut nested = Document::new(64, 64, 72);
            let base = nested.active();
            fill(&mut nested, base, 0, RED);
            paint(&mut nested, base, 1, [O / 2, 0, 0, O / 2]);
            let folder = nested.add_folder().unwrap();
            edit(&mut nested, folder, |p| {
                p.clip = true;
                p.opacity = opacity;
            });
            let inner = nested.add_raster_layer().unwrap();
            nested.move_layer(inner, Some(folder), 0);
            edit(&mut nested, inner, |p| p.blend = BlendMode::Multiply);
            fill(&mut nested, inner, 0, GRAY);

            // base, multiply gray clipped directly
            let mut direct = Document::new(64, 64, 72);
            let base = direct.active();
            fill(&mut direct, base, 0, RED);
            paint(&mut direct, base, 1, [O / 2, 0, 0, O / 2]);
            let top = direct.add_raster_layer().unwrap();
            edit(&mut direct, top, |p| {
                p.clip = true;
                p.opacity = opacity;
                p.blend = BlendMode::Multiply;
            });
            fill(&mut direct, top, 0, GRAY);

            let (a, b) = (flatten(&nested), flatten(&direct));
            if opacity >= 1.0 {
                assert_eq!(a[5][5], [O / 2, 0, 0, O], "red multiplied by gray");
            }
            for (pa, pb) in a.as_flattened().iter().zip(b.as_flattened()) {
                for ch in 0..4 {
                    assert!((pa[ch] as i32 - pb[ch] as i32).abs() <= 2, "opacity {opacity}: {pa:?} vs {pb:?}");
                }
            }
        }
    }

    /// Composite every page tile into `cache`, or only the dirty ones.
    fn refresh(doc: &mut Document, cache: &mut Vec<Box<TilePixels>>) {
        let mut dirty = Vec::new();
        let all = doc.dirty_mut().drain_into(&mut dirty) || cache.is_empty();
        let n = doc.tiles_wide() as i32;
        cache.resize_with(n as usize, new_tile_box);
        for x in 0..n {
            if all || dirty.contains(&TileCoord::new(x, 0)) {
                cache[x as usize] = render(doc, x);
            }
        }
    }

    fn assert_cache_fresh(doc: &mut Document, cache: &mut Vec<Box<TilePixels>>) {
        refresh(doc, cache);
        for (x, tile) in cache.iter().enumerate() {
            assert_eq!(**tile, *render(doc, x as i32), "stale tile {x}");
        }
    }

    #[test]
    fn fr07_contributes_truth_table() {
        use crate::frame::{BorderStyle, Frame, FrameShape, Panel};
        use crate::geom::RectF;
        // Page of 4×1 tiles; the panel fills tile 1 and borders tile 0.
        let mut doc = Document::new(256, 64, 72);
        let folder = doc.add_folder().unwrap();
        let inner = doc.add_raster_layer().unwrap();
        doc.move_layer(inner, Some(folder), 0);
        let at = |x| TileCoord::new(x, 0);
        let contributes = |doc: &Document| -> [bool; 4] {
            let l = doc.layer(folder).unwrap();
            [0, 1, 2, 3].map(|x| doc.contributes(l, at(x)))
        };
        fill(&mut doc, inner, 1, BLUE);
        fill(&mut doc, inner, 3, BLUE);
        assert_eq!(contributes(&doc), [false, true, false, true], "plain folder: where children have pixels");

        let panel = Panel::rect(RectF { x: 60.0, y: 0.0, w: 70.0, h: 64.0 }).unwrap();
        let shape =
            |width| FrameShape { panels: vec![panel.clone()], border: BorderStyle { width, color: [0, 0, 0, O] } };
        doc.set_frame(folder, Some(Frame::build(shape(0.0), 256, 64)));
        // Tile 0 has content but no child pixels, tile 3 child pixels but no content.
        assert_eq!(contributes(&doc), [false, true, false, false]);
        doc.set_frame(folder, Some(Frame::build(shape(8.0), 256, 64)));
        assert_eq!(contributes(&doc), [true, true, true, false], "the border alone contributes");
        doc.paint_target(inner).unwrap().0.clear();
        assert_eq!(contributes(&doc), [true, true, true, false], "even with no children");
        edit(&mut doc, folder, |p| p.opacity = 0.0);
        assert_eq!(contributes(&doc), [false; 4]);
        edit(&mut doc, folder, |p| {
            p.opacity = 1.0;
            p.visible = false;
        });
        assert_eq!(contributes(&doc), [false; 4]);
    }

    #[test]
    fn clip_toggle_invalidates_regrouped_tiles() {
        // Root [base, x, z]: `x` only has pixels in tile 0, `z` clips to
        // `x` or, once `x` clips too, to `base`.
        let mut doc = Document::new(128, 64, 72);
        let base = doc.active();
        let x = doc.add_raster_layer().unwrap();
        let z = doc.add_raster_layer().unwrap();
        edit(&mut doc, z, |p| p.clip = true);
        for t in 0..2 {
            fill(&mut doc, base, t, RED);
            fill(&mut doc, z, t, BLUE);
        }
        fill(&mut doc, x, 0, GRAY);
        let mut cache = Vec::new();
        assert_cache_fresh(&mut doc, &mut cache);
        for clip in [true, false] {
            edit(&mut doc, x, |p| p.clip = clip);
            assert_cache_fresh(&mut doc, &mut cache);
        }

        // A clip layer above a pass-through folder changes how the folder
        // renders everywhere, not just where the clip has pixels.
        let mut doc = Document::new(128, 64, 72);
        red_under_multiply_folder(&mut doc);
        let clip = doc.add_raster_layer().unwrap();
        fill(&mut doc, clip, 0, BLUE);
        let mut cache = Vec::new();
        assert_cache_fresh(&mut doc, &mut cache);
        for on in [true, false] {
            edit(&mut doc, clip, |p| p.clip = on);
            assert_cache_fresh(&mut doc, &mut cache);
        }
    }
}
