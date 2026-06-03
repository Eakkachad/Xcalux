use crate::canvas::{Layer, Tile};
use ahash::AHashMap;
use hokusai::tile::{empty_tile, TilePixels};

pub struct TileSnapshot {
    pub layer_id: u32,
    pub coords: (i32, i32),
    pub pixels: Option<Box<TilePixels>>, // None represents a tile that did not exist
}

pub struct UndoCommand {
    pub snapshots: Vec<TileSnapshot>,
}

pub struct ObjectPool {
    pool: Vec<Box<TilePixels>>,
}

impl ObjectPool {
    pub fn new() -> Self {
        Self { pool: Vec::new() }
    }

    pub fn alloc(&mut self) -> Box<TilePixels> {
        self.pool.pop().unwrap_or_else(empty_tile)
    }

    pub fn recycle(&mut self, tile: Box<TilePixels>) {
        // Keep a reasonable ceiling to prevent memory leaks, e.g. 512 tiles (16 MB)
        if self.pool.len() < 512 {
            self.pool.push(tile);
        }
    }
}

pub struct HistoryManager {
    pub undo_stack: Vec<UndoCommand>,
    pub redo_stack: Vec<UndoCommand>,
    pub max_depth: usize,
    pub pool: ObjectPool,
}

impl HistoryManager {
    pub fn new(max_depth: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_depth,
            pool: ObjectPool::new(),
        }
    }

    /// Allocates a tile from the pool or creates a new one.
    pub fn alloc_tile(&mut self) -> Box<TilePixels> {
        self.pool.alloc()
    }

    /// Recycles a tile back into the pool.
    pub fn recycle_tile(&mut self, tile: Box<TilePixels>) {
        self.pool.recycle(tile);
    }

    /// Push a new command to the undo stack, clearing the redo stack.
    pub fn push_command(&mut self, command: UndoCommand) {
        self.undo_stack.push(command);

        // Evict oldest commands exceeding max depth and recycle their memory
        while self.undo_stack.len() > self.max_depth {
            let evicted = self.undo_stack.remove(0);
            for snapshot in evicted.snapshots {
                if let Some(pixels) = snapshot.pixels {
                    self.pool.recycle(pixels);
                }
            }
        }

        // Clear the redo stack and recycle its buffers
        let expired_redo = std::mem::take(&mut self.redo_stack);
        for cmd in expired_redo {
            for snapshot in cmd.snapshots {
                if let Some(pixels) = snapshot.pixels {
                    self.pool.recycle(pixels);
                }
            }
        }
    }

    /// Perform an undo action. Returns whether undo was successful.
    pub fn undo(&mut self, layers: &mut AHashMap<u32, Layer>) -> bool {
        let Some(undo_cmd) = self.undo_stack.pop() else {
            return false;
        };

        let mut redo_snapshots = Vec::with_capacity(undo_cmd.snapshots.len());

        for mut snapshot in undo_cmd.snapshots {
            let Some(layer) = layers.get_mut(&snapshot.layer_id) else {
                continue;
            };

            // Capture the current state for the redo command
            let current_pixels = layer.tiles.remove(&snapshot.coords).map(|t| t.pixels);
            let redo_snapshot = TileSnapshot {
                layer_id: snapshot.layer_id,
                coords: snapshot.coords,
                pixels: current_pixels,
            };
            redo_snapshots.push(redo_snapshot);

            // Restore the snapshot state
            if let Some(pixels) = snapshot.pixels.take() {
                layer.tiles.insert(
                    snapshot.coords,
                    Tile {
                        pixels,
                        is_dirty: true,
                        last_stroke_id: 0,
                    },
                );
            }

            // Mark the coordinate as dirty to force a WGPU reload
            layer.dirty_tiles.insert(snapshot.coords);
        }

        self.redo_stack.push(UndoCommand {
            snapshots: redo_snapshots,
        });
        true
    }

    /// Perform a redo action. Returns whether redo was successful.
    pub fn redo(&mut self, layers: &mut AHashMap<u32, Layer>) -> bool {
        let Some(redo_cmd) = self.redo_stack.pop() else {
            return false;
        };

        let mut undo_snapshots = Vec::with_capacity(redo_cmd.snapshots.len());

        for mut snapshot in redo_cmd.snapshots {
            let Some(layer) = layers.get_mut(&snapshot.layer_id) else {
                continue;
            };

            // Capture the current state for the undo command
            let current_pixels = layer.tiles.remove(&snapshot.coords).map(|t| t.pixels);
            let undo_snapshot = TileSnapshot {
                layer_id: snapshot.layer_id,
                coords: snapshot.coords,
                pixels: current_pixels,
            };
            undo_snapshots.push(undo_snapshot);

            // Restore the snapshot state
            if let Some(pixels) = snapshot.pixels.take() {
                layer.tiles.insert(
                    snapshot.coords,
                    Tile {
                        pixels,
                        is_dirty: true,
                        last_stroke_id: 0,
                    },
                );
            }

            // Mark the coordinate as dirty to force a WGPU reload
            layer.dirty_tiles.insert(snapshot.coords);
        }

        self.undo_stack.push(UndoCommand {
            snapshots: undo_snapshots,
        });
        true
    }
}

fn sample_bilinear(tex: &[u8], w: u32, h: u32, u: f32, v: f32) -> f32 {
    if u < 0.0 || u > 1.0 || v < 0.0 || v > 1.0 {
        return 0.0;
    }
    let uf = u * (w - 1) as f32;
    let vf = v * (h - 1) as f32;
    let x0 = uf.floor() as u32;
    let y0 = vf.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = uf - x0 as f32;
    let ty = vf - y0 as f32;

    let p00 = tex[(y0 * w + x0) as usize] as f32 / 255.0;
    let p10 = tex[(y0 * w + x1) as usize] as f32 / 255.0;
    let p01 = tex[(y1 * w + x0) as usize] as f32 / 255.0;
    let p11 = tex[(y1 * w + x1) as usize] as f32 / 255.0;

    let r0 = p00 * (1.0 - tx) + p10 * tx;
    let r1 = p01 * (1.0 - tx) + p11 * tx;

    r0 * (1.0 - ty) + r1 * ty
}

pub struct StrokeSurface<'a> {
    pub layer: &'a mut Layer,
    pub history: &'a mut HistoryManager,
    pub snapshots: &'a mut Vec<TileSnapshot>,
    pub stroke_id: u32,

    pub canvas_width: u32,
    pub canvas_height: u32,
    pub lock_canvas_bounds: bool,
    pub selection_mask: Option<&'a crate::canvas::SelectionMask>,
    pub brush_texture: Option<&'a [u8]>,
    pub brush_texture_width: u32,
    pub brush_texture_height: u32,
    pub brush_texture_scale: f32,
}

impl<'a> hokusai::TiledSurface for StrokeSurface<'a> {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        let has_snapshot = if let Some(t) = self.layer.tiles.get(&(tx, ty)) {
            t.last_stroke_id == self.stroke_id
        } else {
            false
        };

        if !has_snapshot {
            let old_pixels = if let Some(t) = self.layer.tiles.get(&(tx, ty)) {
                let mut recycled = self.history.alloc_tile();
                *recycled = *t.pixels;
                Some(recycled)
            } else {
                None
            };

            self.snapshots.push(TileSnapshot {
                layer_id: self.layer.id,
                coords: (tx, ty),
                pixels: old_pixels,
            });
        }

        // Inline layer.tile_request_start logic and set last_stroke_id
        if self.layer.in_atomic {
            self.layer.dirty_tiles.insert((tx, ty));
        }

        let tile = self.layer.tiles.entry((tx, ty)).or_insert_with(crate::canvas::Tile::new);
        tile.is_dirty = true;
        tile.last_stroke_id = self.stroke_id;

        &mut *tile.pixels
    }

    fn tile_request_end(&mut self, tx: i32, ty: i32) {
        self.layer.tile_request_end(tx, ty);
    }

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.layer.tile_lookup(tx, ty)
    }

    fn begin_atomic(&mut self) {
        self.layer.begin_atomic();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.layer.end_atomic()
    }

    fn get_pixel_mask(&self, px: f32, py: f32, dab: &hokusai::Dab) -> f32 {
        let mut mask = 1.0;

        // 1. Canvas Bounds check
        if self.lock_canvas_bounds {
            if px < 0.0 || px >= self.canvas_width as f32 || py < 0.0 || py >= self.canvas_height as f32 {
                return 0.0;
            }
        }

        // 2. Selection Mask check
        if let Some(sel) = self.selection_mask {
            mask *= sel.get_value(px as i32, py as i32) as f32 / 255.0;
            if mask <= 0.0 {
                return 0.0;
            }
        }

        // 3. Brush Tip Texture check
        if let Some(tex) = self.brush_texture {
            let angle = dab.angle.to_radians();
            let cs = angle.cos();
            let sn = angle.sin();
            let yy = py + 0.5 - dab.y;
            let xx = px + 0.5 - dab.x;
            let yyr_unscaled = yy * cs - xx * sn;
            let xxr_unscaled = yy * sn + xx * cs;
            let scale = self.brush_texture_scale.max(0.01);
            let u = xxr_unscaled / (dab.radius * scale) * 0.5 + 0.5;
            let v = yyr_unscaled / (dab.radius * scale) * 0.5 + 0.5;
            
            mask *= sample_bilinear(tex, self.brush_texture_width, self.brush_texture_height, u, v);
        }

        mask
    }
}
