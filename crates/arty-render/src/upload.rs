//! Keeps the GPU page in sync with the document: recomposite dirty tiles in
//! parallel, build their mip chains, upload.
//!
//! Dirty tiles are grouped into rectangles inside one chunk
//! ([`plan_rects`]) and each rectangle is uploaded with one `write_texture`
//! per mip. wgpu creates a staging buffer per `write_texture`, so a full
//! page costs about `chunks × MIP_LEVELS` of them instead of
//! `tiles × MIP_LEVELS`.

use std::cell::RefCell;
use std::time::Instant;

use arty_core::{CompositeScratch, Document, TILE_SIZE, TileCoord, TilePixels, fix15, tile::new_tile_box};
use egui_wgpu::wgpu;
use rayon::prelude::*;

use crate::gpu::{CanvasGpu, MIP_LEVELS, UploadRect};

/// RGBA8 bytes of one tile at mip `k`.
pub const fn level_bytes(k: usize) -> usize {
    (TILE_SIZE >> k) * (TILE_SIZE >> k) * 4
}

/// RGBA8 bytes of one tile's whole mip chain.
pub const CHAIN_BYTES: usize = {
    let mut sum = 0;
    let mut k = 0;
    while k < MIP_LEVELS as usize {
        sum += level_bytes(k);
        k += 1;
    }
    sum
};

/// CPU staging per upload batch. Larger dirty sets go up in several
/// batches, each submitted before the next is built, so neither this buffer
/// nor wgpu's pending staging grows with the page. A full chunk (the
/// largest rect) is ~5.6 MiB.
pub const BATCH_BYTES: usize = 32 << 20;

#[derive(Debug, Clone, Copy, Default)]
pub struct SyncStats {
    pub tiles: usize,
    pub millis: f32,
}

#[derive(Default)]
pub struct CanvasSync {
    dirty: Vec<TileCoord>,
    /// Dirty tiles as `(layer, ty, tx)`, sorted for [`plan_rects`].
    slots: Vec<(u32, u32, u32)>,
    rects: Vec<UploadRect>,
    /// Staging for one batch: per rect, its mip images back to back.
    staging: Vec<u8>,
}

pub struct Worker {
    tile: Box<TilePixels>,
    scratch: CompositeScratch,
    /// RGBA8 mip chain, level k is (64 >> k)² pixels.
    levels: Vec<Vec<u8>>,
}

thread_local! {
    static WORKER: RefCell<Worker> = RefCell::new(Worker::new());
}

/// One tile row of an [`UploadRect`]: `bands[k]` is that row's slice of the
/// rect's mip `k` image (`64 >> k` image rows).
pub struct RowJob<'a> {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub bands: [&'a mut [u8]; MIP_LEVELS as usize],
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}

impl Worker {
    pub fn new() -> Self {
        Self {
            tile: new_tile_box(),
            scratch: CompositeScratch::new(),
            levels: (0..MIP_LEVELS as usize).map(|k| vec![0u8; level_bytes(k)]).collect(),
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

    /// Composite the tiles of `job` and write their mips into its bands.
    pub fn run(&mut self, doc: &Document, mut job: RowJob<'_>) {
        for i in 0..job.w as usize {
            let c = TileCoord::new((job.x as usize + i) as i32, job.y as i32);
            doc.composite_tile(c, &mut self.tile, &mut self.scratch);
            self.build_mips();
            for (k, band) in job.bands.iter_mut().enumerate() {
                scatter_tile(band, job.w as usize, i, TILE_SIZE >> k, &self.levels[k]);
            }
        }
    }
}

/// Access the current thread's persistent upload worker.
pub fn with_thread_worker<R>(f: impl FnOnce(&mut Worker) -> R) -> R {
    WORKER.with_borrow_mut(f)
}

/// Composite the tiles of `jobs` in parallel using persistent thread-local workers.
pub fn run_jobs_reused<'a>(doc: &Document, jobs: impl IntoParallelIterator<Item = RowJob<'a>>) {
    jobs.into_par_iter().for_each(|job| {
        WORKER.with_borrow_mut(|w| w.run(doc, job));
    });
}

/// Composite the tiles of `jobs` sequentially using the calling thread's worker.
pub fn run_jobs_seq<'a>(doc: &Document, jobs: impl IntoIterator<Item = RowJob<'a>>) {
    WORKER.with_borrow_mut(|w| {
        for job in jobs {
            w.run(doc, job);
        }
    });
}

/// Composite the tiles of `jobs` in parallel, initializing a fresh worker per split (pre-E5 baseline).
pub fn run_jobs_fresh<'a>(doc: &Document, jobs: impl IntoParallelIterator<Item = RowJob<'a>>) {
    jobs.into_par_iter().for_each_init(Worker::new, |w, job| w.run(doc, job));
}

/// Copy an `n × n` RGBA8 tile into column `i` of a band `w` tiles wide.
fn scatter_tile(band: &mut [u8], w: usize, i: usize, n: usize, tile: &[u8]) {
    let row = n * 4;
    for (y, src) in tile.chunks_exact(row).enumerate() {
        let at = (y * w + i) * row;
        band[at..at + row].copy_from_slice(src);
    }
}

/// Group sorted, deduplicated `(layer, ty, tx)` slots into rects covering
/// exactly those tiles: horizontal runs within a tile row, each merged into
/// the rect above when that rect spans the same columns and ends on the
/// previous row. A full chunk becomes one rect.
pub fn plan_rects(slots: &[(u32, u32, u32)], out: &mut Vec<UploadRect>) {
    out.clear();
    // Rects ending on the previous / current row (indices into `out`).
    let mut above: Vec<usize> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut row: Option<(u32, u32)> = None;
    let mut i = 0;
    while i < slots.len() {
        let (layer, y, x) = slots[i];
        let mut w = 1;
        i += 1;
        while i < slots.len() && slots[i] == (layer, y, x + w) {
            w += 1;
            i += 1;
        }
        if row != Some((layer, y)) {
            let next_row = row.is_some_and(|(l, ry)| l == layer && ry + 1 == y);
            std::mem::swap(&mut above, &mut current);
            current.clear();
            if !next_row {
                above.clear();
            }
            row = Some((layer, y));
        }
        let hit = above.iter().copied().find(|&j| out[j].x == x && out[j].w == w);
        let j = match hit {
            Some(j) => {
                out[j].h += 1;
                j
            }
            None => {
                out.push(UploadRect { layer, x, y, w, h: 1 });
                out.len() - 1
            }
        };
        current.push(j);
    }
}

/// Split `buf` into the mip images of `r` (mip `k` is `r.w·r.h` tiles of
/// `level_bytes(k)`), then each image into per-tile-row bands.
pub fn row_jobs<'a>(r: &UploadRect, mut buf: &'a mut [u8], jobs: &mut Vec<RowJob<'a>>) {
    let tiles = (r.w * r.h) as usize;
    let mut bands = std::array::from_fn::<_, { MIP_LEVELS as usize }, _>(|k| {
        let (img, rest) = std::mem::take(&mut buf).split_at_mut(tiles * level_bytes(k));
        buf = rest;
        img.chunks_exact_mut(r.w as usize * level_bytes(k))
    });
    for dy in 0..r.h {
        let bands = std::array::from_fn(|k| bands[k].next().expect("one band per tile row"));
        jobs.push(RowJob { x: r.x, y: r.y + dy, w: r.w, bands });
    }
}

impl CanvasSync {
    /// Recomposites dirty tiles and builds their mips in parallel into `staging`.
    pub fn prepare_staging(staging: &mut Vec<u8>, doc: &Document, batch: &[UploadRect]) {
        let bytes: usize = batch.iter().map(|r| (r.w * r.h) as usize * CHAIN_BYTES).sum();
        staging.resize(bytes, 0);

        let total_rows: usize = batch.iter().map(|r| r.h as usize).sum();
        let mut jobs = Vec::with_capacity(total_rows);
        let mut buf = &mut staging[..];
        for r in batch {
            let (mine, tail) = buf.split_at_mut((r.w * r.h) as usize * CHAIN_BYTES);
            buf = tail;
            row_jobs(r, mine, &mut jobs);
        }
        run_jobs_reused(doc, jobs);
    }

    /// Recomposites dirty tiles and builds their mips in parallel into staging.
    pub fn prepare_batch(&mut self, doc: &Document, batch: &[UploadRect]) {
        Self::prepare_staging(&mut self.staging, doc, batch);
    }

    /// Read access to the staging buffer of the most recently prepared batch.
    pub fn staging(&self) -> &[u8] {
        &self.staging
    }

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
        self.slots.clear();
        self.slots.extend(self.dirty.iter().filter(|c| doc.contains_tile(**c)).filter_map(|c| {
            let (tx, ty) = (c.x as u32, c.y as u32);
            gpu.tile_layer(tx, ty).map(|layer| (layer, ty, tx))
        }));
        self.slots.sort_unstable();
        self.slots.dedup();
        plan_rects(&self.slots, &mut self.rects);

        let mut rects = &self.rects[..];
        while !rects.is_empty() {
            // Take rects while they fit the batch (always at least one).
            let mut n = 0;
            let mut bytes = 0;
            while n < rects.len() {
                let b = (rects[n].w * rects[n].h) as usize * CHAIN_BYTES;
                if n > 0 && bytes + b > BATCH_BYTES {
                    break;
                }
                bytes += b;
                n += 1;
            }
            let (batch, rest) = rects.split_at(n);
            rects = rest;

            Self::prepare_staging(&mut self.staging, doc, batch);

            let mut buf = &self.staging[..];
            for r in batch {
                let tiles = (r.w * r.h) as usize;
                let levels: [&[u8]; MIP_LEVELS as usize] = std::array::from_fn(|k| {
                    let (img, tail) = buf.split_at(tiles * level_bytes(k));
                    buf = tail;
                    img
                });
                gpu.upload_rect(queue, r, &levels);
            }
            if !rects.is_empty() {
                // Flush this batch's staging before building the next one.
                queue.submit(std::iter::empty());
                let _ = device.poll(wgpu::PollType::Poll);
            }
        }
        Some(SyncStats { tiles: self.slots.len(), millis: start.elapsed().as_secs_f32() * 1000.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{TILES_PER_CHUNK, chunk_slot};

    fn slots(tiles: &[(u32, u32)], chunks_x: u32) -> Vec<(u32, u32, u32)> {
        let mut s: Vec<_> = tiles
            .iter()
            .map(|&(x, y)| (chunk_slot(x, y, TILES_PER_CHUNK, chunks_x).0, y, x))
            .collect();
        s.sort_unstable();
        s.dedup();
        s
    }

    fn plan(tiles: &[(u32, u32)], chunks_x: u32) -> Vec<UploadRect> {
        let mut out = Vec::new();
        plan_rects(&slots(tiles, chunks_x), &mut out);
        out
    }

    /// Every rect stays inside one chunk, rects don't overlap, and together
    /// they cover exactly the input tiles.
    fn assert_exact_cover(tiles: &[(u32, u32)], chunks_x: u32, rects: &[UploadRect]) {
        let mut covered = Vec::new();
        for r in rects {
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    assert_eq!(chunk_slot(x, y, TILES_PER_CHUNK, chunks_x).0, r.layer, "{r:?} leaves its chunk");
                    covered.push((x, y));
                }
            }
        }
        let n = covered.len();
        covered.sort_unstable();
        covered.dedup();
        assert_eq!(covered.len(), n, "rects overlap");
        let mut want = tiles.to_vec();
        want.sort_unstable();
        want.dedup();
        assert_eq!(covered, want);
    }

    #[test]
    fn full_page_is_one_rect_per_chunk() {
        // 40×20 tiles = 2560×1280 px: chunks 16 wide, last column/row partial.
        let tiles: Vec<_> = (0..20).flat_map(|y| (0..40).map(move |x| (x, y))).collect();
        let rects = plan(&tiles, 3);
        assert_exact_cover(&tiles, 3, &rects);
        assert_eq!(rects.len(), 6);
        assert!(rects.contains(&UploadRect { layer: 0, x: 0, y: 0, w: 16, h: 16 }));
        assert!(rects.contains(&UploadRect { layer: 5, x: 32, y: 16, w: 8, h: 4 }));
    }

    #[test]
    fn scattered_tiles_are_covered_exactly() {
        // Runs that cross a chunk edge, an L shape, gaps, duplicates and
        // rows that skip a line must never pull in clean tiles.
        let tiles = [
            (14, 3), (15, 3), (16, 3), (17, 3),
            (14, 4), (15, 4), (16, 4), (17, 4),
            (2, 5), (3, 5), (4, 5), (2, 6), (3, 6),
            (8, 9), (10, 9), (8, 11), (8, 9),
            (15, 15), (15, 16), (16, 15), (16, 16),
        ];
        let rects = plan(&tiles, 2);
        assert_exact_cover(&tiles, 2, &rects);
        assert!(rects.contains(&UploadRect { layer: 0, x: 14, y: 3, w: 2, h: 2 }));
        assert!(rects.contains(&UploadRect { layer: 1, x: 16, y: 3, w: 2, h: 2 }));
        assert_eq!(rects.len(), 11);
    }

    #[test]
    fn row_jobs_tile_the_staging_buffer() {
        // Each job's bands are disjoint slices of the rect's mip images, so
        // scattering tile (i, dy) lands at its row-major place in each image.
        let r = UploadRect { layer: 0, x: 4, y: 7, w: 3, h: 2 };
        let tiles = (r.w * r.h) as usize;
        let mut buf = vec![0u8; tiles * CHAIN_BYTES];
        let mut jobs = Vec::new();
        row_jobs(&r, &mut buf, &mut jobs);
        assert_eq!(jobs.len(), 2);
        for (dy, mut job) in jobs.into_iter().enumerate() {
            assert_eq!((job.x, job.y, job.w), (4, 7 + dy as u32, 3));
            for (k, band) in job.bands.iter_mut().enumerate() {
                let n = TILE_SIZE >> k;
                for i in 0..3 {
                    let tag = (dy * 3 + i + 1) as u8;
                    scatter_tile(band, 3, i, n, &vec![tag; level_bytes(k)]);
                }
            }
        }
        let mut img = &buf[..];
        for k in 0..MIP_LEVELS as usize {
            let n = TILE_SIZE >> k;
            let (level, rest) = img.split_at(tiles * level_bytes(k));
            img = rest;
            let width = 3 * n;
            for py in 0..2 * n {
                for px in 0..width {
                    let tag = ((py / n) * 3 + px / n + 1) as u8;
                    assert_eq!(level[(py * width + px) * 4], tag, "mip {k} pixel ({px}, {py})");
                }
            }
        }
        assert!(img.is_empty());
    }
}
