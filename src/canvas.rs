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
    pub color: [f32; 3],
    pub width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorDisplayMode {
    Rasterized,
    SplineMesh,
}

impl Default for VectorDisplayMode {
    fn default() -> Self {
        Self::Rasterized
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorLayer {
    pub strokes: Vec<VectorStroke>,
    pub display_mode: VectorDisplayMode,
}

pub type MaskTile = [u8; 4096]; // 64x64 single-channel alpha mask values

#[derive(Clone)]
pub struct LayerMask {
    pub tiles: AHashMap<(i32, i32), Box<MaskTile>>,
    pub enabled: bool,
    /// When true, the mask transforms with the layer (e.g. move/rotate).
    /// Reserved for future transform support.
    #[allow(dead_code)]
    pub linked: bool,
}

impl Default for LayerMask {
    fn default() -> Self {
        Self {
            tiles: AHashMap::default(),
            enabled: true,
            linked: true,
        }
    }
}

#[derive(Clone)]
pub struct Layer {
    pub id: u32,
    pub name: String,
    pub opacity: f32, // 0.0 to 1.0
    pub visible: bool,
    pub locked: bool,
    pub lock_alpha: bool,
    pub is_clipping: bool,
    #[allow(dead_code)]
    pub selection_source: bool,
    pub blend_mode: BlendMode,
    pub tiles: TileMap,
    pub kind: LayerType,
    pub vector_data: Option<VectorLayer>,

    pub mask: Option<LayerMask>,
    pub temp_mask_tiles: TileMap,

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
            locked: false,
            lock_alpha: false,
            is_clipping: false,
            selection_source: false,
            blend_mode: BlendMode::Normal,
            tiles: TileMap::default(),
            kind: LayerType::Raster,
            vector_data: None,
            mask: None,
            temp_mask_tiles: TileMap::default(),
            in_atomic: false,
            dirty_tiles: HashSet::default(),
            thumbnail_dirty: true,
        }
    }

    pub fn add_mask(&mut self) {
        if self.mask.is_none() {
            self.mask = Some(LayerMask::default());
            self.thumbnail_dirty = true;
        }
    }

    pub fn delete_mask(&mut self) {
        if self.mask.is_some() {
            self.mask = None;
            self.temp_mask_tiles.clear();
            self.thumbnail_dirty = true;
        }
    }

    pub fn apply_mask(&mut self) {
        if let Some(mask) = self.mask.take() {
            if mask.enabled {
                for (coords, tile) in &mut self.tiles {
                    if let Some(mask_tile) = mask.tiles.get(coords) {
                        for y in 0..64 {
                            for x in 0..64 {
                                let mask_val = mask_tile[y * 64 + x] as f32 / 255.0;
                                tile.pixels[y][x][3] = (tile.pixels[y][x][3] as f32 * mask_val) as u16;
                            }
                        }
                        tile.is_dirty = true;
                    }
                }
            }
            self.temp_mask_tiles.clear();
            self.thumbnail_dirty = true;
        }
    }

    pub fn invert_mask(&mut self) {
        if let Some(ref mut mask) = self.mask {
            for mask_tile in mask.tiles.values_mut() {
                for val in mask_tile.iter_mut() {
                    *val = 255 - *val;
                }
            }
            // Also sync the inverted mask values to temp_mask_tiles if they exist
            for (coords, temp_tile) in &mut self.temp_mask_tiles {
                if let Some(mask_tile) = mask.tiles.get(coords) {
                    for y in 0..64 {
                        for x in 0..64 {
                            let v = mask_tile[y * 64 + x];
                            let pixel_val = (v as u32 * 32768 / 255) as u16;
                            temp_tile.pixels[y][x] = [pixel_val, pixel_val, pixel_val, pixel_val];
                        }
                    }
                    temp_tile.is_dirty = true;
                }
            }
            self.thumbnail_dirty = true;
        }
    }

    pub fn sync_mask_tile_from_temp(&mut self, coords: (i32, i32)) {
        if let Some(ref mut mask) = self.mask {
            if let Some(temp_tile) = self.temp_mask_tiles.get(&coords) {
                let mask_tile = mask.tiles.entry(coords).or_insert_with(|| Box::new([255; 4096]));
                for y in 0..64 {
                    for x in 0..64 {
                        let val = temp_tile.pixels[y][x][3];
                        mask_tile[y * 64 + x] = ((val as u32 * 255 + 16384) / 32768) as u8;
                    }
                }
            } else {
                mask.tiles.remove(&coords);
            }
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

        let tile = self.tiles.entry((tx, ty)).or_default();
        tile.is_dirty = true;

        // If lock_alpha is enabled and we are starting a dab, we will pass the lock_alpha
        // parameter down to the brush state, which is handled natively by Hokusai's blend mode.
        &mut tile.pixels
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
            if self.mask.is_some() {
                for &coords in &tiles {
                    if self.temp_mask_tiles.contains_key(&coords) {
                        self.sync_mask_tile_from_temp(coords);
                    }
                }
            }
        }
        tiles
    }
}

pub type SelectionTile = [u8; 4096];

#[derive(Clone)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_delete_mask() {
        let mut layer = Layer::new(1, "Test".to_string());
        assert!(layer.mask.is_none());

        layer.add_mask();
        assert!(layer.mask.is_some());
        let mask = layer.mask.as_ref().unwrap();
        assert!(mask.enabled);
        assert!(mask.linked);
        assert!(mask.tiles.is_empty());

        layer.delete_mask();
        assert!(layer.mask.is_none());
    }

    #[test]
    fn test_invert_mask() {
        let mut layer = Layer::new(1, "Test".to_string());
        layer.add_mask();
        let mask = layer.mask.as_mut().unwrap();

        // Insert a tile with known values
        let mut tile = Box::new([0u8; 4096]);
        tile[0] = 0;
        tile[1] = 128;
        tile[2] = 255;
        mask.tiles.insert((0, 0), tile);

        let _ = mask;
        layer.invert_mask();

        let mask = layer.mask.as_ref().unwrap();
        let inverted = mask.tiles.get(&(0, 0)).unwrap();
        assert_eq!(inverted[0], 255);
        assert_eq!(inverted[1], 127);
        assert_eq!(inverted[2], 0);
    }

    #[test]
    fn test_apply_mask_multiplies_alpha() {
        let mut layer = Layer::new(1, "Test".to_string());
        layer.add_mask();

        // Create a color tile with full alpha
        let mut color_tile = Tile::new();
        for y in 0..64 {
            for x in 0..64 {
                color_tile.pixels[y][x] = [32768, 0, 0, 32768]; // full red, full alpha
            }
        }
        color_tile.is_dirty = true;
        layer.tiles.insert((0, 0), color_tile);

        // Create a mask tile with 50% opacity
        let mut mask_tile = Box::new([0u8; 4096]);
        for i in 0..4096 {
            mask_tile[i] = 128; // ~50%
        }
        layer.mask.as_mut().unwrap().tiles.insert((0, 0), mask_tile);

        layer.apply_mask();

        // After apply, mask should be None
        assert!(layer.mask.is_none());

        // Alpha should be ~50% of original (32768 * 128/255 ≈ 16449)
        let tile = layer.tiles.get(&(0, 0)).unwrap();
        assert!((tile.pixels[0][3][3] as f32 - 16449.0).abs() < 10.0);
    }
}
