//! Frame border folders (CSP コマ枠): convex panels that mask a folder's
//! children and draw a border inside each panel.
//!
//! A [`Frame`] is immutable: built once per edit at edit time (allocating),
//! shared by `Arc`, and read per tile by the compositor without allocating.

use std::sync::Arc;

use crate::document::Document;
use crate::geom::{Pt, RectF};
use crate::layer::LayerId;
use crate::tile::{TILE_SIZE, TileCoord};

pub const MAX_PANELS: usize = 1024;
pub const MAX_PANEL_VERTS: usize = 64;

/// A convex panel: 3..=64 vertices, area > 0, counter-clockwise. Vertices
/// may lie outside the canvas within `[-page, 2·page]` (bleed panels, 裁ち切り).
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pts: Vec<Pt>,
}

impl Panel {
    /// A panel from `pts` if they form a valid one (made CCW).
    pub fn new(_pts: Vec<Pt>) -> Option<Panel> {
        // FRAMES
        None
    }

    pub fn rect(_r: RectF) -> Option<Panel> {
        // FRAMES
        None
    }

    pub fn points(&self) -> &[Pt] {
        &self.pts
    }

    pub fn area(&self) -> f32 {
        // FRAMES
        0.0
    }

    pub fn contains(&self, _p: Pt) -> bool {
        // FRAMES
        false
    }

    /// The part on the side `n·x ≥ d`.
    pub fn clip_half_plane(&self, _n: Pt, _d: f32) -> Option<Panel> {
        // FRAMES
        None
    }

    /// Every edge moved in by `w` (miter joins).
    pub fn inset(&self, _w: f32) -> Option<Panel> {
        // FRAMES
        None
    }

    /// The two pieces of a cut along the line `a`→`b` with a gap of `gap`.
    pub fn split(&self, _a: Pt, _b: Pt, _gap: f32) -> (Option<Panel>, Option<Panel>) {
        // FRAMES
        (None, None)
    }

    /// Vertex `i` moved to `p`; `None` if that breaks convexity.
    pub fn with_vertex(&self, _i: usize, _p: Pt) -> Option<Panel> {
        // FRAMES
        None
    }

    /// Edge `i` moved `d` px along its outward normal, its neighbours
    /// keeping their angle.
    pub fn with_edge_offset(&self, _i: usize, _d: f32) -> Option<Panel> {
        // FRAMES
        None
    }

    /// A vertex `p` inserted on edge `i`.
    pub fn with_inserted_vertex(&self, _i: usize, _p: Pt) -> Option<Panel> {
        // FRAMES
        None
    }

    pub fn without_vertex(&self, _i: usize) -> Option<Panel> {
        // FRAMES
        None
    }

    pub fn translated(&self, _d: Pt) -> Panel {
        // FRAMES
        self.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderStyle {
    /// Px; 0 draws no border.
    pub width: f32,
    /// fix15 premultiplied.
    pub color: [u16; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameShape {
    pub panels: Vec<Panel>,
    pub border: BorderStyle,
}

impl FrameShape {
    /// Cut every panel the segment `a`→`b` crosses, with gutters `gap_h`
    /// (between pieces stacked vertically) and `gap_v` (side by side).
    /// Returns the new shape and the indices of the B pieces; `None` when
    /// the segment misses every panel.
    pub fn cut(&self, _a: Pt, _b: Pt, _gap_h: f32, _gap_v: f32) -> Option<(FrameShape, Vec<usize>)> {
        // FRAMES
        None
    }
}

pub type MaskTile = crate::selection::MaskPixels;

/// A frame's coverage of one tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cov<'a> {
    None,
    Full,
    Partial(&'a MaskTile),
}

pub struct Frame {
    shape: FrameShape,
    raster: FrameRaster,
}

/// Per page tile: 0 Outside, 1 Full, `k + 2` → `masks[k]`.
struct FrameRaster {
    tw: u32,
    th: u32,
    content: Box<[u32]>,
    border: Box<[u32]>,
    masks: Vec<Box<MaskTile>>,
}

impl FrameRaster {
    /// Every tile Outside.
    fn outside(tw: u32, th: u32) -> Self {
        let n = (tw * th) as usize;
        Self { tw, th, content: vec![0; n].into_boxed_slice(), border: vec![0; n].into_boxed_slice(), masks: Vec::new() }
    }

    fn index(&self, c: TileCoord) -> Option<usize> {
        (c.x >= 0 && c.y >= 0 && (c.x as u32) < self.tw && (c.y as u32) < self.th)
            .then(|| c.y as usize * self.tw as usize + c.x as usize)
    }

    fn cov(&self, codes: &[u32], c: TileCoord) -> Cov<'_> {
        match self.index(c).map(|i| codes[i]) {
            None | Some(0) => Cov::None,
            Some(1) => Cov::Full,
            Some(k) => self.masks.get(k as usize - 2).map_or(Cov::None, |m| Cov::Partial(m)),
        }
    }
}

impl Frame {
    /// Rasterize `shape` for a `page_w`×`page_h` page.
    pub fn build(shape: FrameShape, page_w: u32, page_h: u32) -> Arc<Frame> {
        // FRAMES: classify and rasterize the panels and borders.
        let tiles = |side: u32| side.div_ceil(TILE_SIZE as u32);
        Arc::new(Frame { shape, raster: FrameRaster::outside(tiles(page_w), tiles(page_h)) })
    }

    pub fn shape(&self) -> &FrameShape {
        &self.shape
    }

    /// Panel coverage of tile `c` (what shows of the children).
    pub fn content(&self, c: TileCoord) -> Cov<'_> {
        self.raster.cov(&self.raster.content, c)
    }

    /// Border coverage of tile `c`.
    pub fn border(&self, c: TileCoord) -> Cov<'_> {
        self.raster.cov(&self.raster.border, c)
    }

    /// Tiles where the frame shows anything (content or border).
    pub fn touched_tiles(&self) -> impl Iterator<Item = TileCoord> + '_ {
        let r = &self.raster;
        let tw = r.tw.max(1) as usize;
        r.content
            .iter()
            .zip(r.border.iter())
            .enumerate()
            .filter(|(_, (c, b))| **c != 0 || **b != 0)
            .map(move |(i, _)| TileCoord::new((i % tw) as i32, (i / tw) as i32))
    }

    /// Index of the topmost panel containing `p`.
    pub fn hit(&self, _p: Pt) -> Option<usize> {
        // FRAMES
        None
    }

    /// Page size in tiles this frame was built for.
    pub fn tiles(&self) -> (u32, u32) {
        (self.raster.tw, self.raster.th)
    }

    /// Bytes of partial-tile masks.
    pub fn mask_bytes(&self) -> usize {
        self.raster.masks.len() * std::mem::size_of::<MaskTile>()
    }

    /// A frame whose content is Full on `full` tiles and Outside elsewhere
    /// (tests of dirty marking and history).
    #[cfg(test)]
    pub(crate) fn with_full_tiles(shape: FrameShape, page_w: u32, page_h: u32, full: &[TileCoord]) -> Arc<Frame> {
        let tiles = |side: u32| side.div_ceil(TILE_SIZE as u32);
        let mut raster = FrameRaster::outside(tiles(page_w), tiles(page_h));
        for &c in full {
            if let Some(i) = raster.index(c) {
                raster.content[i] = 1;
            }
        }
        Arc::new(Frame { shape, raster })
    }
}

/// Add a frame folder holding `shape` and an empty raster child above the
/// active layer. Not recorded; callers wrap it in a structure edit.
pub fn add_frame_folder(_doc: &mut Document, _shape: FrameShape) -> Option<LayerId> {
    // FRAMES
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> FrameShape {
        FrameShape { panels: Vec::new(), border: BorderStyle { width: 4.0, color: [0, 0, 0, 1 << 15] } }
    }

    #[test]
    fn built_frame_is_sized_to_the_page() {
        let f = Frame::build(shape(), 130, 64);
        assert_eq!(f.tiles(), (3, 1));
        assert_eq!(f.shape(), &shape());
        assert_eq!(f.content(TileCoord::new(0, 0)), Cov::None);
        assert_eq!(f.border(TileCoord::new(9, 0)), Cov::None, "off the page");
        assert_eq!(f.touched_tiles().count(), 0);
        assert_eq!(f.mask_bytes(), 0);

        let f = Frame::with_full_tiles(shape(), 130, 130, &[TileCoord::new(2, 1), TileCoord::new(5, 5)]);
        assert_eq!(f.content(TileCoord::new(2, 1)), Cov::Full);
        assert_eq!(f.touched_tiles().collect::<Vec<_>>(), [TileCoord::new(2, 1)]);
    }
}
