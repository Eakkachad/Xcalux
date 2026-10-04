//! Keeps the GPU page in sync with the document: recomposite dirty tiles in
//! parallel, build their mip chains, upload.

use std::time::Instant;

use arty_core::{CompositeScratch, Document, TILE_SIZE, TileCoord, TilePixels, fix15, tile::new_tile_box};
use egui_wgpu::wgpu;
use rayon::prelude::*;

use crate::gpu::{CanvasGpu, MIP_LEVELS};

#[derive(Debug, Clone, Copy, Default)]
pub struct SyncStats {
    pub tiles: usize,
    pub millis: f32,
}

#[derive(Default)]
pub struct CanvasSync {
    dirty: Vec<TileCoord>,
}

struct Worker {
    tile: Box<TilePixels>,
    scratch: CompositeScratch,
    /// RGBA8 mip chain, level k is (64 >> k)² pixels.
    levels: Vec<Vec<u8>>,
}

impl Worker {
    fn new() -> Self {
        Self {
            tile: new_tile_box(),
            scratch: CompositeScratch::new(),
            levels: (0..MIP_LEVELS).map(|k| vec![0u8; (TILE_SIZE >> k).pow(2) * 4]).collect(),
        }
    }

    fn build_mips(&mut self) {
        let base = &mut self.levels[0];
        for (dst, src) in base.chunks_exact_mut(4).zip(self.tile.as_flattened()) {
            for c in 0..4 {
                dst[c] = fix15::to_u8(src[c]);
            }
        }
        for k in 1..self.levels.len() {
            let (lo, hi) = self.levels.split_at_mut(k);
            let src = &lo[k - 1];
            let dst = &mut hi[0];
            let n = TILE_SIZE >> k; // dst side
            let sn = n * 2;
            for y in 0..n {
                for x in 0..n {
                    for c in 0..4 {
                        let p = |xx: usize, yy: usize| src[(yy * sn + xx) * 4 + c] as u32;
                        let sum = p(2 * x, 2 * y) + p(2 * x + 1, 2 * y) + p(2 * x, 2 * y + 1) + p(2 * x + 1, 2 * y + 1);
                        dst[(y * n + x) * 4 + c] = ((sum + 2) / 4) as u8;
                    }
                }
            }
        }
    }
}

impl CanvasSync {
    /// Push every dirty tile to the GPU. Cheap when nothing changed.
    pub fn sync(
        &mut self,
        doc: &mut Document,
        gpu: &mut CanvasGpu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<SyncStats> {
        if gpu.ensure_page(device, doc.width(), doc.height()) {
            doc.dirty_mut().mark_all();
        }
        if doc.dirty_mut().is_clean() {
            return None;
        }
        let start = Instant::now();
        if doc.dirty_mut().drain_into(&mut self.dirty) {
            for y in 0..doc.tiles_high() as i32 {
                for x in 0..doc.tiles_wide() as i32 {
                    self.dirty.push(TileCoord::new(x, y));
                }
            }
        }
        let doc: &Document = doc;
        let gpu: &CanvasGpu = gpu;
        self.dirty.retain(|c| doc.contains_tile(*c));
        self.dirty.par_iter().for_each_init(Worker::new, |w, &c| {
            doc.composite_tile(c, &mut w.tile, &mut w.scratch);
            w.build_mips();
            let levels: [&[u8]; MIP_LEVELS as usize] = std::array::from_fn(|k| w.levels[k].as_slice());
            gpu.upload_tile(queue, c, &levels);
        });
        Some(SyncStats { tiles: self.dirty.len(), millis: start.elapsed().as_secs_f32() * 1000.0 })
    }
}
