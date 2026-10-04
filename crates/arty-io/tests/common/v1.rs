//! A v1 (ARTY 0.1) writer for tests, mirroring `perform_save` in
//! `legacy/src/save.rs` byte for byte: dummy header, then each tile
//! raw-deflated at the default level in task order, the JSON metadata
//! (`serde_json::to_string`, same field order), the 24-byte directory,
//! and finally the real offsets written over the header. Tiles are
//! deflated in parallel, which gives the same bytes as one after another.
//!
//! Deflate output depends on the flate2/miniz_oxide version; the layout
//! does not.

use std::io::{Cursor, Seek, SeekFrom, Write};

use arty_core::TileRef;
use flate2::Compression;
use flate2::write::DeflateEncoder;
use rayon::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct VectorControlPoint {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct VectorStroke {
    pub control_points: Vec<VectorControlPoint>,
    pub brush_preset_id: u64,
    pub color: [f32; 3],
    pub width: f32,
}

/// v1 `LayerMetadata`, fields in its order.
#[derive(Debug, Clone, Serialize)]
pub struct LayerMetadata {
    pub id: u32,
    pub name: String,
    pub opacity: f32,
    pub visible: bool,
    pub lock_alpha: bool,
    pub is_clipping: bool,
    pub blend_mode: String,
    pub kind: String,
    pub folder_child_ids: Vec<u32>,
    pub vector_strokes: Option<Vec<VectorStroke>>,
}

impl LayerMetadata {
    /// A visible, opaque layer of `kind` ("Raster", "Folder", "Vector").
    pub fn new(id: u32, kind: &str) -> Self {
        Self {
            id,
            name: format!("{kind} {id}"),
            opacity: 1.0,
            visible: true,
            lock_alpha: false,
            is_clipping: false,
            blend_mode: "Normal".into(),
            kind: kind.into(),
            folder_child_ids: Vec::new(),
            vector_strokes: None,
        }
    }
}

/// v1 `TileSaveData` (pixels shared instead of boxed; same bytes).
#[derive(Clone)]
pub struct TileSaveData {
    pub layer_id: u32,
    pub tx: i32,
    pub ty: i32,
    pub pixels: TileRef,
}

/// v1 `SaveTask` without the path.
#[derive(Clone, Default)]
pub struct SaveTask {
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// Top → bottom.
    pub layer_order: Vec<u32>,
    pub layers_meta: Vec<LayerMetadata>,
    pub tiles: Vec<TileSaveData>,
}

/// The bytes `perform_save` writes for `task`.
pub fn perform_save(task: &SaveTask) -> Vec<u8> {
    let mut file = Cursor::new(Vec::new());

    // 1. Dummy header
    file.write_all(b"ARTY").unwrap();
    file.write_all(&1u32.to_le_bytes()).unwrap();
    file.write_all(&0u64.to_le_bytes()).unwrap();
    file.write_all(&0u64.to_le_bytes()).unwrap();

    // 2. Compress and write tiles
    let compressed: Vec<Vec<u8>> = task
        .tiles
        .par_iter()
        .map(|t| {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(bytemuck::bytes_of(&*t.pixels)).unwrap();
            encoder.finish().unwrap()
        })
        .collect();
    let mut directory = Vec::with_capacity(task.tiles.len());
    for (t, compressed) in task.tiles.iter().zip(&compressed) {
        let offset = file.stream_position().unwrap();
        file.write_all(compressed).unwrap();
        directory.push((t.layer_id, t.tx, t.ty, offset, compressed.len() as u32));
    }

    // 3. Write JSON metadata block
    let json_offset = file.stream_position().unwrap();
    #[derive(Serialize)]
    struct DocumentMetadata<'a> {
        canvas_width: u32,
        canvas_height: u32,
        layer_order: &'a [u32],
        layers: &'a [LayerMetadata],
    }
    let doc_meta = DocumentMetadata {
        canvas_width: task.canvas_width,
        canvas_height: task.canvas_height,
        layer_order: &task.layer_order,
        layers: &task.layers_meta,
    };
    file.write_all(serde_json::to_string(&doc_meta).unwrap().as_bytes()).unwrap();

    // 4. Write Tile Offset Directory table
    let tile_dir_offset = file.stream_position().unwrap();
    for &(layer_id, tx, ty, offset, compressed_size) in &directory {
        file.write_all(&layer_id.to_le_bytes()).unwrap();
        file.write_all(&tx.to_le_bytes()).unwrap();
        file.write_all(&ty.to_le_bytes()).unwrap();
        file.write_all(&offset.to_le_bytes()).unwrap();
        file.write_all(&compressed_size.to_le_bytes()).unwrap();
    }

    // 5. Rewrite actual offsets in header
    file.seek(SeekFrom::Start(8)).unwrap();
    file.write_all(&json_offset.to_le_bytes()).unwrap();
    file.write_all(&tile_dir_offset.to_le_bytes()).unwrap();
    file.into_inner()
}

/// `(json_off, dir_off)` of a v1 file.
pub fn offsets(file: &[u8]) -> (usize, usize) {
    let at = |i: usize| u64::from_le_bytes(file[i..i + 8].try_into().unwrap()) as usize;
    (at(8), at(16))
}
