//! Layer thumbnails: each raster layer box-filtered down to a few dozen
//! pixels, folders composited from their children's thumbnails, shown over
//! a checkerboard.
//!
//! Work follows the Studio's [`ContentEpochs`]: a layer is looked at again
//! only after an edit that may have changed it, and re-filtered only when
//! its tiles really differ from the ones its thumbnail was made from.
//! Refreshing stops at a per-frame time budget and pauses during strokes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arty_core::tile::{clear_tile, new_tile};
use arty_core::{
    BlendMode, CompositeScratch, Document, Frame, LayerContent, LayerId, TILE_SIZE, TileCoord, TileGrid, TilePixels,
    TileRef, fix15,
};
use egui::{Color32, ColorImage, TextureHandle, TextureId, TextureOptions};
use rayon::prelude::*;

use crate::studio::{ContentEpochs, Studio};

/// Largest thumbnail side in pixels. A thumbnail fits in one tile, so folder
/// thumbnails go through the document's own compositor.
pub const THUMB_MAX: usize = TILE_SIZE;

/// Raster filtering per frame; at least one layer is refreshed regardless.
const FRAME_BUDGET: Duration = Duration::from_millis(4);

/// Pixel size of a page-shaped thumbnail fitting `box_px` (pixels).
pub fn thumb_size(page: [u32; 2], box_px: [f32; 2]) -> [usize; 2] {
    let (w, h) = (page[0].max(1) as f32, page[1].max(1) as f32);
    let s = (box_px[0] / w).min(box_px[1] / h).min(THUMB_MAX as f32 / w.max(h)).min(1.0);
    [((w * s).round() as usize).max(1), ((h * s).round() as usize).max(1)]
}

/// Box-filter the page area of `grid` down to `size` pixels into the
/// top-left of `out` (the rest is cleared). Output pixel `(u, v)` is the
/// mean of the page pixels whose `x·w/W` and `y·h/H` floor to `u` and `v`,
/// so every page pixel counts exactly once; premultiplied values average
/// directly. Output rows are filtered in parallel.
pub fn downscale(grid: &TileGrid, page: [u32; 2], size: [usize; 2], out: &mut TilePixels) {
    clear_tile(out);
    let (pw, ph) = (page[0].max(1) as usize, page[1].max(1) as usize);
    let (tw, th) = (size[0].clamp(1, THUMB_MAX.min(pw)), size[1].clamp(1, THUMB_MAX.min(ph)));
    if grid.is_empty() {
        return;
    }
    // First page pixel of output column/row `i`.
    let start = |i: usize, page: usize, n: usize| (i * page).div_ceil(n);
    out[..th].par_iter_mut().enumerate().for_each(|(v, row)| {
        let (y0, y1) = (start(v, ph, th), start(v + 1, ph, th));
        let mut acc = [[0u64; 4]; THUMB_MAX];
        for ty in y0 / TILE_SIZE..y1.div_ceil(TILE_SIZE) {
            let oy = ty * TILE_SIZE;
            let (ly0, ly1) = (y0.saturating_sub(oy), (y1 - oy).min(TILE_SIZE));
            for tx in 0..pw.div_ceil(TILE_SIZE) {
                let Some(tile) = grid.get(TileCoord::new(tx as i32, ty as i32)) else { continue };
                let (ox, end) = (tx * TILE_SIZE, (tx * TILE_SIZE + TILE_SIZE).min(pw));
                for u in ox * tw / pw..=(end - 1) * tw / pw {
                    let (x0, x1) = (start(u, pw, tw).max(ox) - ox, start(u + 1, pw, tw).min(end) - ox);
                    // One tile's share fits u32: 64·64·(1 << 15) < 2^32.
                    let mut sum = [0u32; 4];
                    for line in &tile[ly0..ly1] {
                        for p in &line[x0..x1] {
                            for c in 0..4 {
                                sum[c] += p[c] as u32;
                            }
                        }
                    }
                    for c in 0..4 {
                        acc[u][c] += sum[c] as u64;
                    }
                }
            }
        }
        let rows = (y1 - y0) as u64;
        for u in 0..tw {
            let n = (start(u + 1, pw, tw) - start(u, pw, tw)) as u64 * rows;
            if n > 0 {
                row[u] = acc[u].map(|s| ((s + n / 2) / n) as u16);
            }
        }
    });
}

/// Display-only coverage boost: `α' = √α`, colour scaled to match. A box
/// mean turns a thin ink line into a few percent of a thumbnail pixel, so
/// line art would vanish without it.
#[inline]
fn boost_coverage(p: [u16; 4]) -> [u32; 4] {
    if p[3] == 0 {
        return [0; 4];
    }
    let a = p[3] as f32 / fix15::ONE as f32;
    let k = a.sqrt() / a;
    let a2 = (a.sqrt() * fix15::ONE as f32) as u32;
    let c = |v: u16| ((v as f32 * k) as u32).min(a2);
    [c(p[0]), c(p[1]), c(p[2]), a2]
}

/// `px` (top-left `size`) over a light checkerboard of `cell`-pixel squares.
fn over_checker(px: &TilePixels, size: [usize; 2], cell: usize) -> ColorImage {
    let (light, dark) = (fix15::from_u8(255) as u32, fix15::from_u8(204) as u32);
    let cell = cell.max(1);
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    for (y, line) in px[..size[1]].iter().enumerate() {
        for (x, p) in line[..size[0]].iter().enumerate() {
            let bg = if (x / cell + y / cell).is_multiple_of(2) { light } else { dark };
            let p = boost_coverage(*p);
            let under = fix15::mul(bg, fix15::ONE - p[3]);
            let ch = |c: usize| fix15::to_u8((p[c] + under).min(fix15::ONE) as u16);
            pixels.push(Color32::from_rgb(ch(0), ch(1), ch(2)));
        }
    }
    ColorImage::new(size, pixels)
}

struct Thumb {
    /// Thumbnail pixels in the top-left `size` corner.
    px: TileRef,
    size: [usize; 2],
    page: [u32; 2],
    /// Raster layers: the tiles `px` was filtered from. Tiles are
    /// copy-on-write, so pointer equality means unchanged pixels.
    source: Option<TileGrid>,
    /// Raster: (pixel, structure) epochs last checked. Folder: (tree epoch,
    /// raster generation) last composited.
    seen: (u64, u64),
    texture: Option<TextureHandle>,
    upload: bool,
}

impl Thumb {
    fn new(size: [usize; 2], page: [u32; 2]) -> Self {
        Self { px: new_tile(), size, page, source: None, seen: (0, 0), texture: None, upload: true }
    }
}

fn same_tiles(a: &TileGrid, b: &TileGrid) -> bool {
    a.len() == b.len() && a.iter().all(|(c, t)| b.get_ref(c).is_some_and(|u| Arc::ptr_eq(t, u)))
}

/// Thumbnail pixels and textures per layer.
#[derive(Default)]
pub struct ThumbCache {
    entries: HashMap<LayerId, Thumb>,
    /// Bumped whenever a raster thumbnail changes; folders recomposite
    /// when it moves.
    generation: u64,
    /// Structure epoch at which deleted layers were last dropped.
    pruned_at: u64,
    /// `Studio::doc_epoch` the entries belong to.
    doc_epoch: u64,
    order: Vec<LayerId>,
    scratch: CompositeScratch,
}

impl ThumbCache {
    /// Forget every thumbnail when the document is replaced. Called every
    /// frame, not only while the Layers tab is drawn: raster sources would
    /// otherwise keep the old document's tiles alive.
    pub fn sync_doc(&mut self, doc_epoch: u64) {
        if self.doc_epoch != doc_epoch {
            self.entries.clear();
            self.doc_epoch = doc_epoch;
        }
    }

    /// Bring thumbnails up to date within this frame's budget. Does nothing
    /// during a stroke. Returns `true` when work is left for later frames.
    pub fn update(&mut self, studio: &Studio, size: [usize; 2]) -> bool {
        !studio.engine.is_stroking() && self.refresh(&studio.doc, studio.epochs(), size, FRAME_BUDGET)
    }

    /// Refresh stale thumbnails, top layer first, until `budget` is spent
    /// (one raster layer always gets through).
    pub fn refresh(&mut self, doc: &Document, epochs: &ContentEpochs, size: [usize; 2], budget: Duration) -> bool {
        if self.pruned_at != epochs.structure() {
            self.entries.retain(|id, _| doc.layer(*id).is_some());
            self.pruned_at = epochs.structure();
        }
        let page = [doc.width(), doc.height()];
        let mut order = std::mem::take(&mut self.order);
        order.clear();
        collect_top_down(doc, doc.root(), &mut order);

        let start = Instant::now();
        let mut filtered = false;
        let mut more = false;
        for &id in &order {
            let Some(grid) = doc.layer(id).and_then(|l| l.raster()) else { continue };
            let seen = (epochs.pixels(id), epochs.structure());
            if let Some(t) = self.entries.get_mut(&id)
                && t.size == size
                && t.page == page
            {
                if t.seen == seen {
                    continue;
                }
                if t.source.as_ref().is_some_and(|s| same_tiles(s, grid)) {
                    t.seen = seen;
                    continue;
                }
            }
            if filtered && start.elapsed() >= budget {
                more = true;
                break;
            }
            let t = self.entries.entry(id).or_insert_with(|| Thumb::new(size, page));
            downscale(grid, page, size, Arc::make_mut(&mut t.px));
            t.source = Some(grid.clone());
            (t.size, t.page, t.seen, t.upload) = (size, page, seen, true);
            self.generation += 1;
            filtered = true;
        }

        // Folders wait for their children so they composite once.
        if !more {
            let seen = (epochs.tree(), self.generation);
            for &id in &order {
                let Some(children) = doc.layer(id).and_then(|l| l.children()) else { continue };
                if self.entries.get(&id).is_some_and(|t| t.seen == seen && t.size == size && t.page == page) {
                    continue;
                }
                let mini = self.mirror(doc, id, children, size);
                let t = self.entries.entry(id).or_insert_with(|| Thumb::new(size, page));
                mini.composite_tile(TileCoord::new(0, 0), Arc::make_mut(&mut t.px), &mut self.scratch);
                // An id that was a raster layer must not pin its old tiles.
                t.source = None;
                (t.size, t.page, t.seen, t.upload) = (size, page, seen, true);
            }
        }
        self.order = order;
        more
    }

    /// A thumbnail-sized document holding `children` (with their settings
    /// and current thumbnails) inside one Normal folder, on transparent
    /// paper: compositing its tile renders the folder's content in
    /// isolation, exactly as the page compositor would. A frame folder's
    /// mirror carries its frame, scaled to the thumbnail.
    fn mirror(&self, doc: &Document, folder: LayerId, children: &[LayerId], size: [usize; 2]) -> Document {
        let mut mini = Document::new(size[0] as u32, size[1] as u32, doc.dpi());
        mini.set_paper(None);
        let group = mini.add_folder().expect("a new document has free ids");
        let mut props = mini.layer(group).expect("just added").props.clone();
        props.blend = BlendMode::Normal;
        mini.set_props(group, props);
        mirror_frame(doc, folder, group, &mut mini);
        self.mirror_into(doc, children, group, &mut mini);
        mini
    }

    fn mirror_into(&self, doc: &Document, ids: &[LayerId], parent: LayerId, mini: &mut Document) {
        for (i, id) in ids.iter().enumerate() {
            let Some(layer) = doc.layer(*id) else { continue };
            let copy = if layer.is_folder() { mini.add_folder() } else { mini.add_raster_layer() };
            let Some(copy) = copy else { continue };
            mini.move_layer(copy, Some(parent), i);
            mini.set_props(copy, layer.props.clone());
            match &layer.content {
                LayerContent::Raster(_) => {
                    if let (Some(t), Some((grid, _))) = (self.entries.get(id), mini.paint_target(copy)) {
                        grid.replace(TileCoord::new(0, 0), Some(t.px.clone()));
                    }
                }
                LayerContent::Folder { children, .. } => {
                    mirror_frame(doc, *id, copy, mini);
                    self.mirror_into(doc, children, copy, mini);
                }
            }
        }
    }

    /// Texture of a layer's thumbnail, uploading it first if it changed.
    /// `cell` is the checker square size in pixels.
    pub fn texture(&mut self, ctx: &egui::Context, id: LayerId, cell: usize) -> Option<TextureId> {
        let t = self.entries.get_mut(&id)?;
        if t.upload || t.texture.is_none() {
            let image = over_checker(&t.px, t.size, cell);
            match &mut t.texture {
                Some(tex) => tex.set(image, TextureOptions::LINEAR),
                None => t.texture = Some(ctx.load_texture(format!("layer-thumb-{}", id.0), image, TextureOptions::LINEAR)),
            }
            t.upload = false;
        }
        Some(t.texture.as_ref()?.id())
    }

    #[cfg(test)]
    fn pixels(&self, id: LayerId) -> Option<&TilePixels> {
        self.entries.get(&id).map(|t| &*t.px)
    }
}

/// Give `copy` in the thumbnail document `mini` the frame of `id`, scaled
/// to the thumbnail.
fn mirror_frame(doc: &Document, id: LayerId, copy: LayerId, mini: &mut Document) {
    if let Some(f) = doc.frame(id) {
        let s = (mini.width() as f32 / doc.width() as f32).min(mini.height() as f32 / doc.height() as f32);
        mini.set_frame(copy, Some(Frame::build(f.shape().scaled(s), mini.width(), mini.height())));
    }
}

/// Every layer, panel order (top → bottom), collapsed folders included.
fn collect_top_down(doc: &Document, ids: &[LayerId], out: &mut Vec<LayerId>) {
    for &id in ids.iter().rev() {
        out.push(id);
        if let Some(children) = doc.layer(id).and_then(|l| l.children()) {
            collect_top_down(doc, children, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arty_brush::InputSample;
    use arty_core::fix15::ONE_U16 as O;
    use arty_core::tile::{fill_tile, new_tile_box};

    const RED: [u16; 4] = [O, 0, 0, O];

    fn grid_with(tiles: &[(i32, i32, [u16; 4])]) -> TileGrid {
        let mut g = TileGrid::new();
        for &(x, y, v) in tiles {
            fill_tile(g.get_mut_or_create(TileCoord::new(x, y)), v);
        }
        g
    }

    fn shrink(grid: &TileGrid, page: [u32; 2], size: [usize; 2]) -> Box<TilePixels> {
        let mut out = new_tile_box();
        downscale(grid, page, size, &mut out);
        out
    }

    #[test]
    fn thumb_size_keeps_page_aspect() {
        assert_eq!(thumb_size([2894, 4093], [60.0, 44.0]), [31, 44]);
        assert_eq!(thumb_size([4000, 1000], [60.0, 44.0]), [60, 15]);
        // Capped at one tile, and never larger than the page.
        assert_eq!(thumb_size([1000, 1000], [200.0, 200.0]), [64, 64]);
        assert_eq!(thumb_size([20, 10], [60.0, 44.0]), [20, 10]);
        assert_eq!(thumb_size([800, 12800], [60.0, 44.0]), [3, 44]);
    }

    #[test]
    fn downscale_uniform_page_stays_uniform() {
        // Page size not a multiple of the tile or the thumbnail size.
        let page = [150, 100];
        let v = [O / 4, O / 8, O / 2, O / 2];
        let grid = grid_with(&[(0, 0, v), (1, 0, v), (2, 0, v), (0, 1, v), (1, 1, v), (2, 1, v)]);
        let out = shrink(&grid, page, [7, 5]);
        for y in 0..5 {
            for x in 0..7 {
                assert_eq!(out[y][x], v, "({x}, {y})");
            }
        }
        assert_eq!(out[0][7], [0; 4], "outside the thumbnail stays clear");
        assert_eq!(out[5][0], [0; 4]);
    }

    #[test]
    fn downscale_is_an_exact_box_mean() {
        // 64×64 page to 2×2: each output pixel averages a 32×32 block.
        let mut grid = TileGrid::new();
        let tile = grid.get_mut_or_create(TileCoord::new(0, 0));
        for row in &mut tile[..32] {
            row[..16].fill(RED); // half of the top-left block
        }
        row_fill(&mut tile[40], 32..64, [0, 0, O, O]); // one row of the bottom-right block
        let out = shrink(&grid, [64, 64], [2, 2]);
        assert_eq!(out[0][0], [O / 2, 0, 0, O / 2]);
        assert_eq!(out[0][1], [0; 4]);
        assert_eq!(out[1][0], [0; 4]);
        assert_eq!(out[1][1], [0, 0, O / 32, O / 32]);
    }

    fn row_fill(row: &mut [[u16; 4]; TILE_SIZE], xs: std::ops::Range<usize>, v: [u16; 4]) {
        row[xs].fill(v);
    }

    #[test]
    fn downscale_ignores_pixels_off_the_page() {
        // 100×100 page: tile (1, 1) is only partly on it, (2, 0) not at all.
        let grid = grid_with(&[(1, 1, RED), (2, 0, RED), (-1, 0, RED)]);
        let out = shrink(&grid, [100, 100], [1, 1]);
        let on_page = 36 * 36; // of 100·100 pixels
        let expect = |full: u16| ((full as u64 * on_page + 5000) / 10000) as u16;
        assert_eq!(out[0][0], [expect(O), 0, 0, expect(O)]);
    }

    #[test]
    fn downscale_matches_a_naive_mean_on_odd_sizes() {
        let page = [131u32, 77u32];
        let mut grid = TileGrid::new();
        for ty in 0..2 {
            for tx in 0..3 {
                let t = grid.get_mut_or_create(TileCoord::new(tx, ty));
                for (y, row) in t.iter_mut().enumerate() {
                    for (x, p) in row.iter_mut().enumerate() {
                        let a = ((x * 7 + y * 13 + (tx * 5 + ty * 3) as usize) % 97 * 337) as u16;
                        *p = [a / 2, a / 3, a, a];
                    }
                }
            }
        }
        let size = [9, 5];
        let out = shrink(&grid, page, size);
        for v in 0..size[1] {
            for u in 0..size[0] {
                let (mut sum, mut n) = ([0u64; 4], 0u64);
                for y in 0..page[1] as usize {
                    for x in 0..page[0] as usize {
                        if x * size[0] / page[0] as usize == u && y * size[1] / page[1] as usize == v {
                            let t = grid.get(TileCoord::new((x / 64) as i32, (y / 64) as i32)).unwrap();
                            for c in 0..4 {
                                sum[c] += t[y % 64][x % 64][c] as u64;
                            }
                            n += 1;
                        }
                    }
                }
                assert_eq!(out[v][u], sum.map(|s| ((s + n / 2) / n) as u16), "({u}, {v})");
            }
        }
    }

    #[test]
    fn boost_makes_faint_coverage_visible_and_keeps_extremes() {
        assert_eq!(boost_coverage([0; 4]), [0; 4]);
        let one = fix15::ONE as u16;
        assert_eq!(boost_coverage([one; 4]), [fix15::ONE; 4]);
        // 4% black ink coverage → 20% after the boost.
        let a = (0.04 * fix15::ONE as f32) as u16;
        let b = boost_coverage([0, 0, 0, a]);
        assert!((b[3] as f32 / fix15::ONE as f32 - 0.2).abs() < 0.01, "{b:?}");
        // Premultiplied colour never exceeds alpha.
        let c = boost_coverage([a, a / 2, 0, a]);
        assert!(c[0] <= c[3] && c[1] <= c[3]);
    }

    #[test]
    fn checkerboard_shows_through_transparency_only() {
        let mut px = new_tile_box();
        px[0][0] = RED;
        px[0][1] = [0; 4];
        let img = over_checker(&px, [3, 1], 1);
        assert_eq!(img.pixels[0], Color32::from_rgb(255, 0, 0));
        assert_eq!(img.pixels[1], Color32::from_rgb(204, 204, 204));
        assert_eq!(img.pixels[2], Color32::from_rgb(255, 255, 255));
    }

    // ----- invalidation ------------------------------------------------------

    const SIZE: [usize; 2] = [8, 8];
    const ALL: Duration = Duration::from_secs(60);

    fn paint(s: &mut Studio, id: LayerId, v: [u16; 4]) {
        s.doc.set_active(id);
        fill_tile(s.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0)), v);
        // Stands in for a stroke: an undoable pixel edit Studio knows about.
        s.clear_active_layer();
        s.undo();
    }

    fn refresh(cache: &mut ThumbCache, s: &Studio) -> u64 {
        let before = cache.generation;
        assert!(!cache.refresh(&s.doc, s.epochs(), SIZE, ALL));
        cache.generation - before
    }

    #[test]
    fn only_changed_layers_are_refiltered() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let a = s.doc.active();
        s.edit_structure(|d| d.add_raster_layer().unwrap() != a);
        let b = s.doc.active();
        let mut cache = ThumbCache::default();
        assert_eq!(refresh(&mut cache, &s), 2, "first pass fills every layer");
        assert_eq!(refresh(&mut cache, &s), 0, "nothing changed");

        paint(&mut s, a, RED);
        assert_eq!(refresh(&mut cache, &s), 1, "only the painted layer");
        assert_eq!(cache.pixels(a).unwrap()[0][0], RED);

        let mut p = s.doc.layer(b).unwrap().props.clone();
        p.visible = false;
        s.set_layer_props(b, p, false);
        assert_eq!(refresh(&mut cache, &s), 0, "settings don't touch raster thumbnails");

        s.edit_structure(|d| d.shift_layer(b, -1));
        assert_eq!(refresh(&mut cache, &s), 0, "moving keeps pixels");

        s.edit_structure(|d| {
            d.add_raster_layer().unwrap();
            true
        });
        assert_eq!(refresh(&mut cache, &s), 1, "only the new layer");

        // `a` is now on top of `b`; merging changes `b`'s pixels only.
        s.doc.set_active(a);
        s.edit_structure(|d| d.merge_down(a));
        assert_eq!(refresh(&mut cache, &s), 1);
        assert_eq!(cache.pixels(b).unwrap()[0][0], RED);
        assert!(cache.pixels(a).is_none(), "merged layer dropped");

        s.undo();
        assert_eq!(refresh(&mut cache, &s), 2, "undo restores both layers' pixels");
        assert_eq!(cache.pixels(b).unwrap()[0][0], [0; 4]);
    }

    #[test]
    fn replacing_the_document_redoes_every_thumbnail() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let mut cache = ThumbCache::default();
        assert_eq!(refresh(&mut cache, &s), 1);

        // An opened file reuses the old document's layer ids.
        let mut opened = Document::new(64, 64, 72);
        let id = opened.active();
        assert!(s.doc.layer(id).is_some());
        fill_tile(opened.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0)), RED);
        s.replace_document(opened);
        assert_eq!(refresh(&mut cache, &s), 1);
        assert_eq!(cache.pixels(id).unwrap()[0][0], RED);
    }

    #[test]
    fn a_replaced_document_frees_its_tiles_without_a_refresh() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let id = s.doc.active();
        fill_tile(s.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(0, 0)), RED);
        let mut cache = ThumbCache::default();
        cache.sync_doc(s.doc_epoch);
        assert_eq!(refresh(&mut cache, &s), 1);
        let old = Arc::downgrade(s.doc.layer(id).unwrap().raster().unwrap().get_ref(TileCoord::new(0, 0)).unwrap());

        // The Layers tab is closed: only sync_doc runs.
        s.replace_document(Document::new(64, 64, 72));
        assert!(old.upgrade().is_some(), "the thumbnail's source holds the old tile");
        cache.sync_doc(s.doc_epoch);
        assert!(old.upgrade().is_none(), "the old document's tile was freed");
        assert!(cache.pixels(id).is_none());
    }

    #[test]
    fn budget_spreads_work_over_frames() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        for _ in 0..3 {
            s.edit_structure(|d| {
                d.add_raster_layer().unwrap();
                true
            });
        }
        let mut cache = ThumbCache::default();
        let mut frames = 0;
        while cache.refresh(&s.doc, s.epochs(), SIZE, Duration::ZERO) {
            frames += 1;
        }
        assert_eq!((frames, cache.generation), (3, 4), "one layer per frame");
    }

    #[test]
    fn folder_thumbnail_composites_its_children() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let folder = {
            s.edit_structure(|d| {
                d.add_folder().unwrap();
                true
            });
            s.doc.active()
        };
        s.edit_structure(|d| {
            let inner = d.add_raster_layer().unwrap();
            d.move_layer(inner, Some(folder), 0)
        });
        let inner = s.doc.active();
        paint(&mut s, inner, RED);
        let mut cache = ThumbCache::default();
        refresh(&mut cache, &s);
        assert_eq!(cache.pixels(folder).unwrap()[0][0], RED);
        assert_eq!(cache.pixels(folder).unwrap()[0][SIZE[0]], [0; 4]);

        let mut p = s.doc.layer(inner).unwrap().props.clone();
        p.opacity = 0.5;
        s.set_layer_props(inner, p, false);
        refresh(&mut cache, &s);
        let half = cache.pixels(folder).unwrap()[0][0];
        assert!((half[3] as i32 - O as i32 / 2).abs() <= 1, "{half:?}");
        assert_eq!(cache.pixels(inner).unwrap()[0][0], RED, "a layer's own thumbnail ignores its settings");
    }

    #[test]
    fn frame_folder_thumbnail_is_masked_by_its_panels() {
        use arty_core::{BorderStyle, FrameShape, Panel, RectF};
        let mut s = Studio::new(Document::new(64, 64, 72));
        let panel = Panel::rect(RectF { x: 0.0, y: 0.0, w: 32.0, h: 64.0 }).unwrap();
        let shape = FrameShape { panels: vec![panel], border: BorderStyle { width: 0.0, color: [0, 0, 0, O] } };
        s.edit_structure(|d| arty_core::frame::add_frame_folder(d, shape).is_some());
        let inner = s.doc.active();
        let folder = s.doc.frame_folder_of(inner).unwrap();
        paint(&mut s, inner, RED);
        let mut cache = ThumbCache::default();
        refresh(&mut cache, &s);
        let px = cache.pixels(folder).unwrap();
        assert_eq!(px[3][1], RED, "inside the panel");
        assert_eq!(px[3][6], [0; 4], "outside the panel");
        assert_eq!(cache.pixels(inner).unwrap()[3][6], RED, "the child's own thumbnail is not masked");
    }

    fn ms(n: u32, mut f: impl FnMut()) -> f64 {
        f();
        let t = Instant::now();
        for _ in 0..n {
            f();
        }
        t.elapsed().as_secs_f64() * 1000.0 / n as f64
    }

    /// Timings for plans/bench (B003):
    /// `cargo test -p arty-app --release -- --ignored --nocapture bench_thumbnails`
    #[test]
    #[ignore]
    fn bench_thumbnails() {
        println!("rayon threads: {}", rayon::current_num_threads());
        for (name, page) in [("A4 350 dpi", [2894u32, 4093]), ("B4 600 dpi", [6071, 8598])] {
            // Every tile painted, each its own allocation.
            let mut grid = TileGrid::new();
            for ty in 0..page[1].div_ceil(64) {
                for tx in 0..page[0].div_ceil(64) {
                    let t = grid.get_mut_or_create(TileCoord::new(tx as i32, ty as i32));
                    fill_tile(t, [O / 4, O / 8, O / 16, O / 2]);
                    t[(ty % 64) as usize][(tx % 64) as usize] = RED;
                }
            }
            let size = thumb_size(page, [60.0, 44.0]);
            let mut out = new_tile_box();
            let par = ms(10, || downscale(&grid, page, size, &mut out));
            let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
            let one = pool.install(|| ms(5, || downscale(&grid, page, size, &mut out)));
            let copy = grid.clone();
            let check = ms(10, || assert!(same_tiles(&copy, &grid)));
            println!(
                "{name}: {} tiles -> {size:?} px: downscale {par:.2} ms (all threads), {one:.2} ms (1 thread); unchanged check {check:.3} ms",
                grid.len()
            );
        }

        // A folder of 30 one-tile layers: a settings change recomposites it.
        let mut s = Studio::new(Document::new(2894, 4093, 350));
        s.edit_structure(|d| {
            d.add_folder().unwrap();
            true
        });
        let folder = s.doc.active();
        for i in 0..30 {
            s.edit_structure(|d| {
                let l = d.add_raster_layer().unwrap();
                d.move_layer(l, Some(folder), i)
            });
            let id = s.doc.active();
            fill_tile(s.doc.paint_target(id).unwrap().0.get_mut_or_create(TileCoord::new(i as i32, i as i32)), RED);
        }
        let mut cache = ThumbCache::default();
        let size = thumb_size([2894, 4093], [60.0, 44.0]);
        cache.refresh(&s.doc, s.epochs(), size, ALL);
        let mut op = 1.0;
        let recomposite = ms(50, || {
            op = if op == 1.0 { 0.5 } else { 1.0 };
            let mut p = s.doc.layer(folder).unwrap().props.clone();
            p.opacity = op;
            s.set_layer_props(folder, p, false);
            let generation = cache.generation;
            cache.refresh(&s.doc, s.epochs(), size, ALL);
            assert_eq!(cache.generation, generation);
        });
        println!("folder of 30 layers: settings change -> refresh {recomposite:.3} ms");
    }

    #[test]
    fn no_refresh_during_a_stroke() {
        let mut s = Studio::new(Document::new(64, 64, 72));
        let mut cache = ThumbCache::default();
        assert!(!cache.update(&s, SIZE));
        let gen0 = cache.generation;
        let at = |x: f32| InputSample { x, y: 20.0, pressure: 1.0, time: x as f64 * 0.01, ..Default::default() };
        assert!(s.begin_stroke(at(4.0)));
        for x in 5..40 {
            s.feed_stroke(at(x as f32));
        }
        cache.update(&s, SIZE);
        assert_eq!(cache.generation, gen0, "stroke in progress");
        s.end_stroke();
        cache.update(&s, SIZE);
        assert_eq!(cache.generation, gen0 + 1);
        assert!(cache.pixels(s.doc.active()).unwrap()[2][3][3] > 0, "stroke shows up");
    }
}
