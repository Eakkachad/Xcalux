use crate::canvas::{Layer, Tile, BlendMode, SelectionMask};
use ahash::AHashMap;
use hokusai::tile::{empty_tile, TilePixels};

#[derive(Debug, Clone, PartialEq)]
pub enum LayerPropertyChange {
    Opacity { old: f32, new: f32 },
    Visible { old: bool, new: bool },
    LockAlpha { old: bool, new: bool },
    Locked { old: bool, new: bool },
    Clipping { old: bool, new: bool },
    BlendMode { old: BlendMode, new: BlendMode },
    Rename { old: String, new: String },
}

pub struct TileSnapshot {
    pub layer_id: u32,
    pub coords: (i32, i32),
    pub pixels: Option<Box<TilePixels>>, // None represents a tile that did not exist
    pub is_mask: bool,
}

pub enum HistoryCommand {
    TileEdit {
        snapshots: Vec<TileSnapshot>,
    },
    LayerCreate {
        layer: Box<Layer>,
        index: usize,
    },
    LayerDelete {
        layer: Box<Layer>,
        index: usize,
    },
    LayerReorder {
        old_order: Vec<u32>,
        new_order: Vec<u32>,
    },
    LayerProperty {
        layer_id: u32,
        property: LayerPropertyChange,
    },
    SelectionChange {
        old_mask: Box<SelectionMask>,
        new_mask: Box<SelectionMask>,
    },
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
    pub undo_stack: Vec<HistoryCommand>,
    pub redo_stack: Vec<HistoryCommand>,
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

    /// Recycle tiles from a command back into the pool.
    fn recycle_command(&mut self, command: HistoryCommand) {
        if let HistoryCommand::TileEdit { snapshots } = command {
            for snapshot in snapshots {
                if let Some(pixels) = snapshot.pixels {
                    self.pool.recycle(pixels);
                }
            }
        }
    }

    /// Push a new command to the undo stack, clearing the redo stack.
    pub fn push_command(&mut self, command: HistoryCommand) {
        self.undo_stack.push(command);

        // Evict oldest commands exceeding max depth and recycle their memory
        while self.undo_stack.len() > self.max_depth {
            let evicted = self.undo_stack.remove(0);
            self.recycle_command(evicted);
        }

        // Clear the redo stack and recycle its buffers
        let expired_redo = std::mem::take(&mut self.redo_stack);
        for cmd in expired_redo {
            self.recycle_command(cmd);
        }
    }

    /// Perform an undo action. Returns whether undo was successful.
    pub fn undo(
        &mut self,
        layers: &mut AHashMap<u32, Layer>,
        layer_order: &mut Vec<u32>,
        selection_mask: &mut SelectionMask,
        active_layer_id: &mut u32,
    ) -> bool {
        let Some(command) = self.undo_stack.pop() else {
            return false;
        };

        let reversed = self.apply_command_reversed(command, layers, layer_order, selection_mask, active_layer_id);
        self.redo_stack.push(reversed);
        true
    }

    /// Perform a redo action. Returns whether redo was successful.
    pub fn redo(
        &mut self,
        layers: &mut AHashMap<u32, Layer>,
        layer_order: &mut Vec<u32>,
        selection_mask: &mut SelectionMask,
        active_layer_id: &mut u32,
    ) -> bool {
        let Some(command) = self.redo_stack.pop() else {
            return false;
        };

        let forward = self.apply_command_forward(command, layers, layer_order, selection_mask, active_layer_id);
        self.undo_stack.push(forward);
        true
    }

    /// Apply a command forward, returning the reversed command.
    fn apply_command_forward(
        &mut self,
        command: HistoryCommand,
        layers: &mut AHashMap<u32, Layer>,
        layer_order: &mut Vec<u32>,
        selection_mask: &mut SelectionMask,
        active_layer_id: &mut u32,
    ) -> HistoryCommand {
        match command {
            HistoryCommand::TileEdit { snapshots } => {
                let mut redo_snapshots = Vec::with_capacity(snapshots.len());
                for mut snapshot in snapshots {
                    let Some(layer) = layers.get_mut(&snapshot.layer_id) else {
                        continue;
                    };
                    layer.thumbnail_dirty = true;

                    let tiles_map = if snapshot.is_mask {
                        &mut layer.temp_mask_tiles
                    } else {
                        &mut layer.tiles
                    };

                    let current_pixels = tiles_map.remove(&snapshot.coords).map(|t| t.pixels);
                    redo_snapshots.push(TileSnapshot {
                        layer_id: snapshot.layer_id,
                        coords: snapshot.coords,
                        pixels: current_pixels,
                        is_mask: snapshot.is_mask,
                    });

                    if let Some(pixels) = snapshot.pixels.take() {
                        tiles_map.insert(
                            snapshot.coords,
                            Tile { pixels, is_dirty: true, last_stroke_id: 0 },
                        );
                    }

                    if snapshot.is_mask {
                        layer.sync_mask_tile_from_temp(snapshot.coords);
                    }
                    layer.dirty_tiles.insert(snapshot.coords);
                }
                HistoryCommand::TileEdit { snapshots: redo_snapshots }
            }
            HistoryCommand::LayerCreate { layer, index } => {
                // Forward: INSERT the layer
                let layer_id = layer.id;
                let layer_clone = Layer::clone(&layer);
                layer_order.insert(index, layer_id);
                layers.insert(layer_id, layer_clone);
                *active_layer_id = layer_id;
                // Return reversed (undo): a LayerCreate that will remove it
                HistoryCommand::LayerCreate { layer, index }
            }
            HistoryCommand::LayerDelete { layer, index } => {
                // Forward: REMOVE the layer
                let layer_id = layer.id;
                let layer_clone = Layer::clone(&layer);
                layer_order.retain(|&id| id != layer_id);
                layers.remove(&layer_id);
                if let Some(id) = layer_order.first() {
                    *active_layer_id = *id;
                }
                // Return reversed (undo): a LayerDelete that will restore it
                HistoryCommand::LayerDelete { layer: Box::new(layer_clone), index }
            }
            HistoryCommand::LayerReorder { old_order, new_order } => {
                *layer_order = old_order.clone();
                HistoryCommand::LayerReorder {
                    old_order: new_order,
                    new_order: old_order,
                }
            }
            HistoryCommand::LayerProperty { layer_id, property } => {
                if let Some(layer) = layers.get_mut(&layer_id) {
                    layer.thumbnail_dirty = true;
                    let reverse = match property {
                        LayerPropertyChange::Opacity { old, new } => {
                            layer.opacity = old;
                            LayerPropertyChange::Opacity { old: new, new: old }
                        }
                        LayerPropertyChange::Visible { old, new } => {
                            layer.visible = old;
                            LayerPropertyChange::Visible { old: new, new: old }
                        }
                        LayerPropertyChange::LockAlpha { old, new } => {
                            layer.lock_alpha = old;
                            LayerPropertyChange::LockAlpha { old: new, new: old }
                        }
                        LayerPropertyChange::Clipping { old, new } => {
                            layer.is_clipping = old;
                            LayerPropertyChange::Clipping { old: new, new: old }
                        }
                        LayerPropertyChange::BlendMode { old, new } => {
                            layer.blend_mode = old;
                            LayerPropertyChange::BlendMode { old: new, new: old }
                        }
                        LayerPropertyChange::Locked { old, new } => {
                            layer.locked = old;
                            LayerPropertyChange::Locked { old: new, new: old }
                        }
                        LayerPropertyChange::Rename { old, new } => {
                            layer.name = old.clone();
                            LayerPropertyChange::Rename { old: new, new: old }
                        }
                    };
                    HistoryCommand::LayerProperty { layer_id, property: reverse }
                } else {
                    HistoryCommand::LayerProperty { layer_id, property }
                }
            }
            HistoryCommand::SelectionChange { old_mask, new_mask } => {
                *selection_mask = *old_mask.clone();
                HistoryCommand::SelectionChange {
                    old_mask: new_mask,
                    new_mask: old_mask,
                }
            }
        }
    }

    /// Apply a command in reverse (undo), returning the forward command.
    fn apply_command_reversed(
        &mut self,
        command: HistoryCommand,
        layers: &mut AHashMap<u32, Layer>,
        layer_order: &mut Vec<u32>,
        selection_mask: &mut SelectionMask,
        active_layer_id: &mut u32,
    ) -> HistoryCommand {
        match command {
            HistoryCommand::TileEdit { snapshots } => {
                let mut undo_snapshots = Vec::with_capacity(snapshots.len());
                for mut snapshot in snapshots {
                    let Some(layer) = layers.get_mut(&snapshot.layer_id) else {
                        continue;
                    };
                    layer.thumbnail_dirty = true;

                    let tiles_map = if snapshot.is_mask {
                        &mut layer.temp_mask_tiles
                    } else {
                        &mut layer.tiles
                    };

                    let current_pixels = tiles_map.remove(&snapshot.coords).map(|t| t.pixels);
                    undo_snapshots.push(TileSnapshot {
                        layer_id: snapshot.layer_id,
                        coords: snapshot.coords,
                        pixels: current_pixels,
                        is_mask: snapshot.is_mask,
                    });

                    if let Some(pixels) = snapshot.pixels.take() {
                        tiles_map.insert(
                            snapshot.coords,
                            Tile { pixels, is_dirty: true, last_stroke_id: 0 },
                        );
                    }

                    if snapshot.is_mask {
                        layer.sync_mask_tile_from_temp(snapshot.coords);
                    }
                    layer.dirty_tiles.insert(snapshot.coords);
                }
                HistoryCommand::TileEdit { snapshots: undo_snapshots }
            }
            HistoryCommand::LayerCreate { layer, index } => {
                // Reversed: REMOVE the layer (undo a create)
                let layer_id = layer.id;
                let layer_clone = Layer::clone(&layer);
                layer_order.retain(|&id| id != layer_id);
                layers.remove(&layer_id);
                if let Some(id) = layer_order.first() {
                    *active_layer_id = *id;
                }
                // Return forward: a LayerCreate that will re-insert it
                HistoryCommand::LayerCreate { layer: Box::new(layer_clone), index }
            }
            HistoryCommand::LayerDelete { layer, index } => {
                // Reversed: INSERT the layer (undo a delete)
                let layer_id = layer.id;
                let layer_clone = Layer::clone(&layer);
                layer_order.insert(index, layer_id);
                layers.insert(layer_id, layer_clone);
                *active_layer_id = layer_id;
                // Return forward: a LayerDelete that will re-remove it
                HistoryCommand::LayerDelete { layer, index }
            }
            HistoryCommand::LayerReorder { old_order, new_order } => {
                *layer_order = old_order.clone();
                HistoryCommand::LayerReorder {
                    old_order: new_order,
                    new_order: old_order,
                }
            }
            HistoryCommand::LayerProperty { layer_id, property } => {
                if let Some(layer) = layers.get_mut(&layer_id) {
                    layer.thumbnail_dirty = true;
                    let reverse = match property {
                        LayerPropertyChange::Opacity { old, new } => {
                            layer.opacity = old;
                            LayerPropertyChange::Opacity { old: new, new: old }
                        }
                        LayerPropertyChange::Visible { old, new } => {
                            layer.visible = old;
                            LayerPropertyChange::Visible { old: new, new: old }
                        }
                        LayerPropertyChange::LockAlpha { old, new } => {
                            layer.lock_alpha = old;
                            LayerPropertyChange::LockAlpha { old: new, new: old }
                        }
                        LayerPropertyChange::Clipping { old, new } => {
                            layer.is_clipping = old;
                            LayerPropertyChange::Clipping { old: new, new: old }
                        }
                        LayerPropertyChange::BlendMode { old, new } => {
                            layer.blend_mode = old;
                            LayerPropertyChange::BlendMode { old: new, new: old }
                        }
                        LayerPropertyChange::Locked { old, new } => {
                            layer.locked = old;
                            LayerPropertyChange::Locked { old: new, new: old }
                        }
                        LayerPropertyChange::Rename { old, new } => {
                            layer.name = old.clone();
                            LayerPropertyChange::Rename { old: new, new: old }
                        }
                    };
                    HistoryCommand::LayerProperty { layer_id, property: reverse }
                } else {
                    HistoryCommand::LayerProperty { layer_id, property }
                }
            }
            HistoryCommand::SelectionChange { old_mask, new_mask } => {
                *selection_mask = *old_mask.clone();
                HistoryCommand::SelectionChange {
                    old_mask: new_mask,
                    new_mask: old_mask,
                }
            }
        }
    }
}

fn sample_bilinear(tex: &[u8], w: u32, h: u32, u: f32, v: f32) -> f32 {
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
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
    pub active_mask_editing: bool,
}

impl<'a> hokusai::TiledSurface for StrokeSurface<'a> {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        if self.active_mask_editing {
            // Ensure temp mask tile exists and is initialized
            if !self.layer.temp_mask_tiles.contains_key(&(tx, ty)) {
                let mut temp_tile = Tile::new();
                let mut has_copied = false;
                if let Some(ref mask) = self.layer.mask {
                    if let Some(mask_tile) = mask.tiles.get(&(tx, ty)) {
                        for y in 0..64 {
                            for x in 0..64 {
                                let v = mask_tile[y * 64 + x];
                                let pixel_val = (v as u32 * 32768 / 255) as u16;
                                temp_tile.pixels[y][x] = [pixel_val, pixel_val, pixel_val, pixel_val];
                            }
                        }
                        has_copied = true;
                    }
                }
                if !has_copied {
                    let pixel_val = 32768;
                    for y in 0..64 {
                        for x in 0..64 {
                            temp_tile.pixels[y][x] = [pixel_val, pixel_val, pixel_val, pixel_val];
                        }
                    }
                }
                self.layer.temp_mask_tiles.insert((tx, ty), temp_tile);
            }
        }

        let tiles_map = if self.active_mask_editing {
            &mut self.layer.temp_mask_tiles
        } else {
            &mut self.layer.tiles
        };

        let has_snapshot = if let Some(t) = tiles_map.get(&(tx, ty)) {
            t.last_stroke_id == self.stroke_id
        } else {
            false
        };

        if !has_snapshot {
            let old_pixels = if let Some(t) = tiles_map.get(&(tx, ty)) {
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
                is_mask: self.active_mask_editing,
            });
        }

        if self.layer.in_atomic {
            self.layer.dirty_tiles.insert((tx, ty));
        }

        let tile = tiles_map.entry((tx, ty)).or_insert_with(crate::canvas::Tile::new);
        tile.is_dirty = true;
        tile.last_stroke_id = self.stroke_id;

        &mut tile.pixels
    }

    fn tile_request_end(&mut self, tx: i32, ty: i32) {
        self.layer.tile_request_end(tx, ty);
    }

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        if self.active_mask_editing {
            self.layer.temp_mask_tiles.get(&(tx, ty)).map(|t| &*t.pixels)
        } else {
            self.layer.tiles.get(&(tx, ty)).map(|t| &*t.pixels)
        }
    }

    fn begin_atomic(&mut self) {
        self.layer.begin_atomic();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.layer.end_atomic()
    }

    fn get_pixel_mask(&self, px: f32, py: f32, dab: &hokusai::Dab) -> f32 {
        let mut mask = 1.0;

        if self.lock_canvas_bounds
            && (px < 0.0 || px >= self.canvas_width as f32 || py < 0.0 || py >= self.canvas_height as f32) {
                return 0.0;
            }

        if let Some(sel) = self.selection_mask {
            mask *= sel.get_value(px as i32, py as i32) as f32 / 255.0;
            if mask <= 0.0 {
                return 0.0;
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_layer(id: u32, name: &str) -> Layer {
        Layer::new(id, name.to_string())
    }

    #[test]
    fn test_tile_edit_undo_redo() {
        let mut layers = AHashMap::new();
        let mut layer_order = vec![1];
        let mut sel = SelectionMask::new();
        let mut active = 1u32;
        let mut hm = HistoryManager::new(50);

        let mut layer = make_layer(1, "L1");
        // Set pixel at (0,0) to some value
        let mut tile = Tile::new();
        tile.pixels[0][0] = [16384, 0, 0, 16384];
        layer.tiles.insert((0, 0), tile);
        layers.insert(1, layer);

        // Create a TileEdit command: old pixels were [0,0,0,0], new pixels are [16384,...]
        let mut old_pixels = hm.alloc_tile();
        old_pixels[0][0] = [0, 0, 0, 0];
        hm.push_command(HistoryCommand::TileEdit {
            snapshots: vec![TileSnapshot {
                layer_id: 1,
                coords: (0, 0),
                pixels: Some(old_pixels),
                is_mask: false,
            }],
        });

        // The command is on the undo stack. Now undo it.
        assert!(hm.undo(&mut layers, &mut layer_order, &mut sel, &mut active));
        let layer = layers.get(&1).unwrap();
        assert_eq!(layer.tiles.get(&(0, 0)).unwrap().pixels[0][0], [0, 0, 0, 0]);

        // Redo
        assert!(hm.redo(&mut layers, &mut layer_order, &mut sel, &mut active));
        let layer = layers.get(&1).unwrap();
        assert_eq!(layer.tiles.get(&(0, 0)).unwrap().pixels[0][0], [16384, 0, 0, 16384]);
    }

    #[test]
    fn test_layer_create_undo_redo() {
        let mut layers = AHashMap::new();
        let mut layer_order = vec![1];
        let mut sel = SelectionMask::new();
        let mut active;
        let mut hm = HistoryManager::new(50);

        layers.insert(1, make_layer(1, "L1"));

        // Simulate creating layer 2 at index 0
        let l2 = make_layer(2, "L2");
        layers.insert(2, l2.clone());
        layer_order.insert(0, 2);
        active = 2;
        hm.push_command(HistoryCommand::LayerCreate { layer: Box::new(l2), index: 0 });

        // Undo: layer 2 should be removed
        assert!(hm.undo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert!(!layers.contains_key(&2));
        assert_eq!(layer_order, vec![1]);

        // Redo: layer 2 should be restored
        assert!(hm.redo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert!(layers.contains_key(&2));
        assert_eq!(layer_order, vec![2, 1]);
        assert_eq!(active, 2);
    }

    #[test]
    fn test_layer_delete_undo_redo() {
        let mut layers = AHashMap::new();
        let mut layer_order = vec![2, 1];
        let mut sel = SelectionMask::new();
        let mut active;
        let mut hm = HistoryManager::new(50);

        layers.insert(1, make_layer(1, "L1"));
        let mut l2 = make_layer(2, "L2");
        // Give L2 some content
        let mut tile = Tile::new();
        tile.pixels[0][0] = [32768, 0, 0, 32768];
        l2.tiles.insert((0, 0), tile);
        layers.insert(2, l2);

        // Delete layer 2
        let removed = layers.remove(&2).unwrap();
        layer_order.retain(|&id| id != 2);
        active = 1;
        hm.push_command(HistoryCommand::LayerDelete {
            layer: Box::new(removed),
            index: 0,
        });

        assert!(!layers.contains_key(&2));
        assert_eq!(layer_order, vec![1]);

        // Undo: layer 2 should be restored
        assert!(hm.undo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert!(layers.contains_key(&2));
        assert_eq!(layer_order, vec![2, 1]);
        assert_eq!(active, 2);
        assert_eq!(layers.get(&2).unwrap().tiles.get(&(0, 0)).unwrap().pixels[0][0], [32768, 0, 0, 32768]);

        // Redo: layer 2 should be deleted again
        assert!(hm.redo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert!(!layers.contains_key(&2));
        assert_eq!(layer_order, vec![1]);
    }

    #[test]
    fn test_layer_reorder_undo_redo() {
        let mut layers = AHashMap::new();
        let mut layer_order = vec![1, 2, 3];
        let mut sel = SelectionMask::new();
        let mut active = 1u32;
        let mut hm = HistoryManager::new(50);

        for i in 1..=3 {
            layers.insert(i, make_layer(i, &format!("L{}", i)));
        }

        let old_order = layer_order.clone();
        layer_order = vec![3, 1, 2];
        hm.push_command(HistoryCommand::LayerReorder {
            old_order: old_order.clone(),
            new_order: layer_order.clone(),
        });

        assert_eq!(layer_order, vec![3, 1, 2]);

        // Undo
        assert!(hm.undo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert_eq!(layer_order, vec![1, 2, 3]);

        // Redo
        assert!(hm.redo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert_eq!(layer_order, vec![3, 1, 2]);
    }

    #[test]
    fn test_layer_property_undo_redo() {
        let mut layers = AHashMap::new();
        let mut layer_order = vec![1];
        let mut sel = SelectionMask::new();
        let mut active = 1u32;
        let mut hm = HistoryManager::new(50);

        let mut layer = make_layer(1, "L1");
        layer.opacity = 1.0;
        layers.insert(1, layer);

        // Change opacity to 0.5
        hm.push_command(HistoryCommand::LayerProperty {
            layer_id: 1,
            property: LayerPropertyChange::Opacity { old: 1.0, new: 0.5 },
        });
        layers.get_mut(&1).unwrap().opacity = 0.5;

        assert_eq!(layers.get(&1).unwrap().opacity, 0.5);

        // Undo
        assert!(hm.undo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert_eq!(layers.get(&1).unwrap().opacity, 1.0);

        // Redo
        assert!(hm.redo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert_eq!(layers.get(&1).unwrap().opacity, 0.5);
    }

    #[test]
    fn test_selection_change_undo_redo() {
        let mut layers = AHashMap::new();
        let mut layer_order = vec![];
        let mut sel;
        let mut active = 1u32;
        let mut hm = HistoryManager::new(50);

        let mut old_sel = SelectionMask::new();
        old_sel.is_active = false;

        let mut new_sel = SelectionMask::new();
        new_sel.is_active = true;
        new_sel.tiles.insert((0, 0), Box::new([255u8; 4096]));

        // Apply new selection
        sel = new_sel.clone();

        hm.push_command(HistoryCommand::SelectionChange {
            old_mask: Box::new(old_sel.clone()),
            new_mask: Box::new(new_sel.clone()),
        });

        assert!(sel.is_active);

        // Undo
        assert!(hm.undo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert!(!sel.is_active);

        // Redo
        assert!(hm.redo(&mut layers, &mut layer_order, &mut sel, &mut active));
        assert!(sel.is_active);
    }
}
