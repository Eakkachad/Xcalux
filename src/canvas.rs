use ahash::AHashMap;
use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};
use hokusai::TiledSurface;
use std::collections::HashSet;

#[allow(dead_code)]
pub const TILE_PIXEL_COUNT: usize = TILE_SIZE * TILE_SIZE;

#[derive(Clone)]
pub struct Tile {
    pub pixels: Box<TilePixels>,
    pub is_dirty: bool,
    pub last_stroke_id: u32,
}

impl Default for Tile {
    fn default() -> Self {
        Self {
            pixels: empty_tile(),
            is_dirty: true,
            last_stroke_id: 0,
        }
    }
}

impl Tile {
    pub fn new() -> Self {
        Self::default()
    }
}

pub type TileMap = AHashMap<(i32, i32), Tile>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Luminosity,
    Shade,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerType {
    Raster,
    Folder { child_ids: Vec<u32> },
    Vector,
}

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VectorControlPoint {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorStroke {
    pub control_points: Vec<VectorControlPoint>,
    pub brush_preset_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorLayer {
    pub strokes: Vec<VectorStroke>,
}

#[derive(Clone)]
pub struct Layer {
    pub id: u32,
    pub name: String,
    pub opacity: f32, // 0.0 to 1.0
    pub visible: bool,
    pub lock_alpha: bool,
    pub is_clipping: bool,
    #[allow(dead_code)]
    pub selection_source: bool,
    pub blend_mode: BlendMode,
    pub tiles: TileMap,
    pub kind: LayerType,
    pub vector_data: Option<VectorLayer>,

    // History & dirty tracking fields
    pub in_atomic: bool,
    pub dirty_tiles: HashSet<(i32, i32)>,

    // Thumbnail dirty flag — set when any tile changes, cleared after thumbnail regenerated
    pub thumbnail_dirty: bool,
}

impl Layer {
    pub fn new(id: u32, name: String) -> Self {
        Self {
            id,
            name,
            opacity: 1.0,
            visible: true,
            lock_alpha: false,
            is_clipping: false,
            selection_source: false,
            blend_mode: BlendMode::Normal,
            tiles: TileMap::default(),
            kind: LayerType::Raster,
            vector_data: None,
            in_atomic: false,
            dirty_tiles: HashSet::default(),
            thumbnail_dirty: true,
        }
    }

    /// Generate a downscaled RGBA thumbnail (max_dim² max pixels, aspect-correct).
    /// Returns (pixels, width, height) as RGBA bytes.
    pub fn generate_thumbnail(&self, max_dim: u32) -> (Vec<u8>, u32, u32) {
        if self.tiles.is_empty() {
            return (vec![0u8; (max_dim * max_dim * 4) as usize], max_dim, max_dim);
        }

        let tx_min = self.tiles.keys().map(|&(tx, _)| tx).min().unwrap_or(0);
        let tx_max = self.tiles.keys().map(|&(tx, _)| tx).max().unwrap_or(0);
        let ty_min = self.tiles.keys().map(|&(_, ty)| ty).min().unwrap_or(0);
        let ty_max = self.tiles.keys().map(|&(_, ty)| ty).max().unwrap_or(0);

        let pix_w = ((tx_max - tx_min + 1) * 64) as u32;
        let pix_h = ((ty_max - ty_min + 1) * 64) as u32;

        let scale = (max_dim as f32 / pix_w as f32).min(max_dim as f32 / pix_h as f32).min(1.0);
        let thumb_w = (pix_w as f32 * scale).max(1.0) as u32;
        let thumb_h = (pix_h as f32 * scale).max(1.0) as u32;

        let mut result = vec![0u8; (thumb_w * thumb_h * 4) as usize];

        for (tile_key, tile) in &self.tiles {
            let base_x = (tile_key.0 - tx_min) * 64;
            let base_y = (tile_key.1 - ty_min) * 64;
            for ly in 0..64 {
                for lx in 0..64 {
                    let src_x = base_x + lx as i32;
                    let src_y = base_y + ly as i32;
                    let dst_x = (src_x as f32 * scale) as u32;
                    let dst_y = (src_y as f32 * scale) as u32;
                    if dst_x < thumb_w && dst_y < thumb_h {
                        let dst_idx = ((dst_y * thumb_w + dst_x) * 4) as usize;
                        let px = tile.pixels[ly][lx];
                        result[dst_idx] = (px[0].min(32768) as u32 * 255 / 32768) as u8;
                        result[dst_idx + 1] = (px[1].min(32768) as u32 * 255 / 32768) as u8;
                        result[dst_idx + 2] = (px[2].min(32768) as u32 * 255 / 32768) as u8;
                        result[dst_idx + 3] = (px[3].min(32768) as u32 * 255 / 32768) as u8;
                    }
                }
            }
        }

        (result, thumb_w, thumb_h)
    }
}

impl TiledSurface for Layer {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        if self.in_atomic {
            self.dirty_tiles.insert((tx, ty));
        }

        let tile = self.tiles.entry((tx, ty)).or_insert_with(Tile::new);
        tile.is_dirty = true;

        // If lock_alpha is enabled and we are starting a dab, we will pass the lock_alpha
        // parameter down to the brush state, which is handled natively by Hokusai's blend mode.
        &mut *tile.pixels
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tiles.get(&(tx, ty)).map(|t| &*t.pixels)
    }

    fn begin_atomic(&mut self) {
        self.in_atomic = true;
        self.dirty_tiles.clear();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.in_atomic = false;
        let tiles: Vec<(i32, i32)> = self.dirty_tiles.drain().collect();
        if !tiles.is_empty() {
            self.thumbnail_dirty = true;
        }
        tiles
    }
}

pub type SelectionTile = [u8; 4096];

pub struct SelectionMask {
    pub tiles: ahash::AHashMap<(i32, i32), Box<SelectionTile>>,
    pub is_active: bool,
}

impl SelectionMask {
    pub fn new() -> Self {
        Self {
            tiles: ahash::AHashMap::default(),
            is_active: false,
        }
    }

    pub fn get_value(&self, x: i32, y: i32) -> u8 {
        if !self.is_active {
            return 255;
        }
        let tx = x.div_euclid(64);
        let ty = y.div_euclid(64);
        let lx = x.rem_euclid(64) as usize;
        let ly = y.rem_euclid(64) as usize;
        if let Some(tile) = self.tiles.get(&(tx, ty)) {
            tile[ly * 64 + lx]
        } else {
            0
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}
