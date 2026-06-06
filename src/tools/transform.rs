use crate::canvas::{Layer, Tile};
use ahash::AHashMap;

#[allow(dead_code)]
pub enum TransformTarget {
    ActiveLayer,
    Selection,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum InterpolationMode {
    Nearest,
    Bilinear,
    Bicubic,
}

#[allow(dead_code)]
pub struct TransformState {
    #[allow(dead_code)]
    pub active: bool,
    #[allow(dead_code)]
    pub target: TransformTarget,
    #[allow(dead_code)]
    pub matrix: [f32; 6], // affine: [a, b, c, d, e, f] -> x' = a*x + c*y + e, y' = b*x + d*y + f
    #[allow(dead_code)]
    pub interpolation: InterpolationMode,
    #[allow(dead_code)]
    pub source_snapshot: Option<LayerSnapshot>,
}

#[allow(dead_code)]
pub struct LayerSnapshot {
    #[allow(dead_code)]
    pub tiles: AHashMap<(i32, i32), crate::canvas::Tile>,
}

#[allow(dead_code)]
impl TransformState {
    pub fn new() -> Self {
        Self {
            active: false,
            target: TransformTarget::ActiveLayer,
            matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            interpolation: InterpolationMode::Bilinear,
            source_snapshot: None,
        }
    }

    pub fn snap_layer(&mut self, layer: &Layer) {
        let mut tiles = AHashMap::default();
        for (&coords, tile) in &layer.tiles {
            let mut new_tile = Tile::new();
            new_tile.pixels = tile.pixels.clone();
            new_tile.is_dirty = true;
            tiles.insert(coords, new_tile);
        }
        self.source_snapshot = Some(LayerSnapshot { tiles });
    }

    pub fn apply_transform(&self, layer: &mut Layer) -> Vec<(i32, i32)> {
        let snapshot = match &self.source_snapshot {
            Some(s) => s,
            None => return Vec::new(),
        };

        let [a, b, c, d, e, f] = self.matrix;

        // Determine bounds of source content
        let mut min_tx = i32::MAX;
        let mut min_ty = i32::MAX;
        let mut max_tx = i32::MIN;
        let mut max_ty = i32::MIN;

        for (&(tx, ty), _) in &snapshot.tiles {
            min_tx = min_tx.min(tx);
            min_ty = min_ty.min(ty);
            max_tx = max_tx.max(tx);
            max_ty = max_ty.max(ty);
        }

        if min_tx == i32::MAX {
            return Vec::new();
        }

        // Compute bounding box of transformed content
        let corners = [
            (min_tx as f32 * 64.0, min_ty as f32 * 64.0),
            ((max_tx + 1) as f32 * 64.0, min_ty as f32 * 64.0),
            (min_tx as f32 * 64.0, (max_ty + 1) as f32 * 64.0),
            ((max_tx + 1) as f32 * 64.0, (max_ty + 1) as f32 * 64.0),
        ];

        let mut bx0 = f32::MAX;
        let mut by0 = f32::MAX;
        let mut bx1 = f32::MIN;
        let mut by1 = f32::MIN;

        for &(sx, sy) in &corners {
            let dx = a * sx + c * sy + e;
            let dy = b * sx + d * sy + f;
            bx0 = bx0.min(dx);
            by0 = by0.min(dy);
            bx1 = bx1.max(dx);
            by1 = by1.max(dy);
        }

        let ttx0 = (bx0 as i32).div_euclid(64) - 1;
        let tty0 = (by0 as i32).div_euclid(64) - 1;
        let ttx1 = (bx1 as i32).div_euclid(64) + 1;
        let tty1 = (by1 as i32).div_euclid(64) + 1;

        layer.tiles.clear();
        let mut dirty_tiles = Vec::new();

        for ty in tty0..=tty1 {
            for tx in ttx0..=ttx1 {
                let mut new_tile = Tile::new();
                for ly in 0..64 {
                    for lx in 0..64 {
                        let dx = (tx * 64 + lx) as f32 + 0.5;
                        let dy = (ty * 64 + ly) as f32 + 0.5;

                        // Inverse transform
                        let det = a * d - b * c;
                        if det.abs() < 0.0001 {
                            continue;
                        }
                        let inv_det = 1.0 / det;
                        let sx = (d * (dx - e) - c * (dy - f)) * inv_det;
                        let sy = (a * (dy - f) - b * (dx - e)) * inv_det;

                        let color = match self.interpolation {
                            InterpolationMode::Nearest => sample_nearest(&snapshot.tiles, sx, sy),
                            InterpolationMode::Bilinear => sample_bilinear(&snapshot.tiles, sx, sy),
                            InterpolationMode::Bicubic => sample_bicubic(&snapshot.tiles, sx, sy),
                        };
                        new_tile.pixels[ly as usize][lx as usize] = color;
                    }
                }
                new_tile.is_dirty = true;
                layer.tiles.insert((tx, ty), new_tile);
                dirty_tiles.push((tx, ty));
            }
        }

        dirty_tiles
    }
}

#[allow(dead_code)]
fn sample_nearest(tiles: &AHashMap<(i32, i32), Tile>, sx: f32, sy: f32) -> [u16; 4] {
    let tx = (sx as i32).div_euclid(64);
    let ty = (sy as i32).div_euclid(64);
    let lx = sx.rem_euclid(64.0) as usize;
    let ly = sy.rem_euclid(64.0) as usize;

    if let Some(tile) = tiles.get(&(tx, ty)) {
        let lx = lx.min(63);
        let ly = ly.min(63);
        tile.pixels[ly][lx]
    } else {
        [0, 0, 0, 0]
    }
}

#[allow(dead_code)]
fn sample_bilinear(tiles: &AHashMap<(i32, i32), Tile>, sx: f32, sy: f32) -> [u16; 4] {
    let ix = sx.floor() as i32;
    let iy = sy.floor() as i32;
    let fx = sx - ix as f32;
    let fy = sy - iy as f32;

    let c00 = get_pixel(tiles, ix, iy);
    let c10 = get_pixel(tiles, ix + 1, iy);
    let c01 = get_pixel(tiles, ix, iy + 1);
    let c11 = get_pixel(tiles, ix + 1, iy + 1);

    let mut result = [0u16; 4];
    for i in 0..4 {
        let v00 = c00[i] as f32;
        let v10 = c10[i] as f32;
        let v01 = c01[i] as f32;
        let v11 = c11[i] as f32;
        let v0 = v00 * (1.0 - fx) + v10 * fx;
        let v1 = v01 * (1.0 - fx) + v11 * fx;
        result[i] = (v0 * (1.0 - fy) + v1 * fy) as u16;
    }
    result
}

#[allow(dead_code)]
fn sample_bicubic(tiles: &AHashMap<(i32, i32), Tile>, sx: f32, sy: f32) -> [u16; 4] {
    sample_bilinear(tiles, sx, sy)
}

#[allow(dead_code)]
fn get_pixel(tiles: &AHashMap<(i32, i32), Tile>, x: i32, y: i32) -> [u16; 4] {
    let tx = x.div_euclid(64);
    let ty = y.div_euclid(64);
    let lx = x.rem_euclid(64) as usize;
    let ly = y.rem_euclid(64) as usize;

    if let Some(tile) = tiles.get(&(tx, ty)) {
        let lx = lx.min(63);
        let ly = ly.min(63);
        tile.pixels[ly][lx]
    } else {
        [0, 0, 0, 0]
    }
}
