use std::fs::File;
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use flate2::write::DeflateEncoder;
use flate2::read::DeflateDecoder;
use flate2::Compression;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct LayerMetadata {
    pub id: u32,
    pub name: String,
    pub opacity: f32,
    pub visible: bool,
    pub lock_alpha: bool,
    pub is_clipping: bool,
    pub blend_mode: String,
    pub kind: String, // "Raster", "Folder", "Vector"
    pub folder_child_ids: Vec<u32>,
    pub vector_strokes: Option<Vec<crate::canvas::VectorStroke>>,
}

pub struct TileSaveData {
    pub layer_id: u32,
    pub tx: i32,
    pub ty: i32,
    pub pixels: Box<hokusai::tile::TilePixels>,
}

pub struct SaveTask {
    pub filepath: PathBuf,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub layer_order: Vec<u32>,
    pub layers_meta: Vec<LayerMetadata>,
    pub tiles: Vec<TileSaveData>,
}

pub fn blend_mode_to_str(m: crate::canvas::BlendMode) -> &'static str {
    match m {
        crate::canvas::BlendMode::Normal => "Normal",
        crate::canvas::BlendMode::Multiply => "Multiply",
        crate::canvas::BlendMode::Screen => "Screen",
        crate::canvas::BlendMode::Overlay => "Overlay",
        crate::canvas::BlendMode::Luminosity => "Luminosity",
        crate::canvas::BlendMode::Shade => "Shade",
    }
}

pub fn layer_type_to_str(kind: &crate::canvas::LayerType) -> &'static str {
    match kind {
        crate::canvas::LayerType::Raster => "Raster",
        crate::canvas::LayerType::Folder { .. } => "Folder",
        crate::canvas::LayerType::Vector => "Vector",
    }
}

pub fn save_worker_loop(rx: std::sync::mpsc::Receiver<SaveTask>) {
    while let Ok(task) = rx.recv() {
        if let Err(e) = perform_save(task) {
            log::error!("Background save failed: {:?}", e);
        } else {
            log::info!("Background save completed successfully.");
        }
    }
}

fn perform_save(task: SaveTask) -> std::io::Result<()> {
    let temp_path = task.filepath.with_extension("tmp");
    let mut file = File::create(&temp_path)?;

    // 1. Write dummy header
    file.write_all(b"ARTY")?; // Magic
    file.write_all(&1u32.to_le_bytes())?; // Version
    file.write_all(&0u64.to_le_bytes())?; // dummy json_offset
    file.write_all(&0u64.to_le_bytes())?; // dummy tile_dir_offset

    // 2. Compress and write tiles
    struct DirEntry {
        pub layer_id: u32,
        pub tx: i32,
        pub ty: i32,
        pub offset: u64,
        pub compressed_size: u32,
    }
    let mut directory = Vec::new();

    for t in task.tiles {
        let offset = file.stream_position()?;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        
        let pixels_slice: &[u16] = bytemuck::cast_slice(&*t.pixels);
        let pixels_bytes: &[u8] = bytemuck::cast_slice(pixels_slice);
        encoder.write_all(pixels_bytes)?;
        let compressed = encoder.finish()?;

        file.write_all(&compressed)?;
        let compressed_size = compressed.len() as u32;

        directory.push(DirEntry {
            layer_id: t.layer_id,
            tx: t.tx,
            ty: t.ty,
            offset,
            compressed_size,
        });
    }

    // 3. Write JSON metadata block
    let json_offset = file.stream_position()?;
    
    #[derive(Serialize)]
    struct DocumentMetadata {
        pub canvas_width: u32,
        pub canvas_height: u32,
        pub layer_order: Vec<u32>,
        pub layers: Vec<LayerMetadata>,
    }
    
    let doc_meta = DocumentMetadata {
        canvas_width: task.canvas_width,
        canvas_height: task.canvas_height,
        layer_order: task.layer_order,
        layers: task.layers_meta,
    };
    let json_str = serde_json::to_string(&doc_meta).unwrap();
    file.write_all(json_str.as_bytes())?;

    // 4. Write Tile Offset Directory table
    let tile_dir_offset = file.stream_position()?;
    for entry in &directory {
        file.write_all(&entry.layer_id.to_le_bytes())?;
        file.write_all(&entry.tx.to_le_bytes())?;
        file.write_all(&entry.ty.to_le_bytes())?;
        file.write_all(&entry.offset.to_le_bytes())?;
        file.write_all(&entry.compressed_size.to_le_bytes())?;
    }

    // 5. Rewrite actual offsets in header
    file.seek(SeekFrom::Start(8))?;
    file.write_all(&json_offset.to_le_bytes())?;
    file.write_all(&tile_dir_offset.to_le_bytes())?;

    file.sync_all()?;
    drop(file);

    // Atomic rename
    std::fs::rename(&temp_path, &task.filepath)?;

    Ok(())
}

pub struct LoadedTile {
    pub layer_id: u32,
    pub tx: i32,
    pub ty: i32,
    pub pixels: Box<hokusai::tile::TilePixels>,
}

pub struct LoadedLayer {
    pub id: u32,
    pub name: String,
    pub opacity: f32,
    pub visible: bool,
    pub lock_alpha: bool,
    pub is_clipping: bool,
    pub blend_mode: crate::canvas::BlendMode,
    pub kind: crate::canvas::LayerType,
    pub tiles: Vec<LoadedTile>,
    pub vector_data: Option<crate::canvas::VectorLayer>,
}

pub struct LoadedDocument {
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub layer_order: Vec<u32>,
    pub layers: Vec<LoadedLayer>,
}

pub fn load_document(path: &Path) -> std::io::Result<LoadedDocument> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"ARTY" {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid file format"));
    }

    let mut version = [0u8; 4];
    file.read_exact(&mut version)?;

    let mut offsets = [0u8; 16];
    file.read_exact(&mut offsets)?;
    let json_offset = u64::from_le_bytes(offsets[0..8].try_into().unwrap());
    let tile_dir_offset = u64::from_le_bytes(offsets[8..16].try_into().unwrap());

    // 1. Read JSON metadata
    file.seek(SeekFrom::Start(json_offset))?;
    let mut json_bytes = Vec::new();
    let json_len = (tile_dir_offset - json_offset) as usize;
    json_bytes.resize(json_len, 0);
    file.read_exact(&mut json_bytes)?;
    
    #[derive(Deserialize)]
    struct DocumentMetadata {
        pub canvas_width: u32,
        pub canvas_height: u32,
        pub layer_order: Vec<u32>,
        pub layers: Vec<LayerMetadata>,
    }
    let doc_meta: DocumentMetadata = serde_json::from_slice(&json_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    // 2. Read Tile Offset Directory
    file.seek(SeekFrom::Start(tile_dir_offset))?;
    let metadata_len = file.metadata()?.len();
    let dir_len = (metadata_len - tile_dir_offset) as usize;
    let entry_count = dir_len / 24;

    struct DirEntry {
        pub layer_id: u32,
        pub tx: i32,
        pub ty: i32,
        pub offset: u64,
        pub compressed_size: u32,
    }
    let mut directory = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let mut entry_buf = [0u8; 24];
        file.read_exact(&mut entry_buf)?;
        let layer_id = u32::from_le_bytes(entry_buf[0..4].try_into().unwrap());
        let tx = i32::from_le_bytes(entry_buf[4..8].try_into().unwrap());
        let ty = i32::from_le_bytes(entry_buf[8..12].try_into().unwrap());
        let offset = u64::from_le_bytes(entry_buf[12..20].try_into().unwrap());
        let compressed_size = u32::from_le_bytes(entry_buf[20..24].try_into().unwrap());
        directory.push(DirEntry { layer_id, tx, ty, offset, compressed_size });
    }

    // 3. Load and Decompress Tiles
    let mut loaded_tiles = Vec::with_capacity(entry_count);
    for entry in directory {
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut comp_bytes = vec![0u8; entry.compressed_size as usize];
        file.read_exact(&mut comp_bytes)?;

        let mut decoder = DeflateDecoder::new(&comp_bytes[..]);
        let mut uncomp_bytes = vec![0u8; 64 * 64 * 8];
        decoder.read_exact(&mut uncomp_bytes)?;

        let mut pixels = hokusai::tile::empty_tile();
        let pixels_u16: &[u16] = bytemuck::cast_slice(&uncomp_bytes);
        for y in 0..64 {
            for x in 0..64 {
                let idx = (y * 64 + x) * 4;
                pixels[y][x] = [
                    pixels_u16[idx],
                    pixels_u16[idx + 1],
                    pixels_u16[idx + 2],
                    pixels_u16[idx + 3],
                ];
            }
        }

        loaded_tiles.push(LoadedTile {
            layer_id: entry.layer_id,
            tx: entry.tx,
            ty: entry.ty,
            pixels,
        });
    }

    // 4. Reconstruct Loaded Layers
    let mut layers = Vec::with_capacity(doc_meta.layers.len());
    for lm in doc_meta.layers {
        let blend_mode = match lm.blend_mode.as_str() {
            "Multiply" => crate::canvas::BlendMode::Multiply,
            "Screen" => crate::canvas::BlendMode::Screen,
            "Overlay" => crate::canvas::BlendMode::Overlay,
            "Luminosity" => crate::canvas::BlendMode::Luminosity,
            "Shade" => crate::canvas::BlendMode::Shade,
            _ => crate::canvas::BlendMode::Normal,
        };

        let kind = match lm.kind.as_str() {
            "Folder" => crate::canvas::LayerType::Folder { child_ids: lm.folder_child_ids },
            "Vector" => crate::canvas::LayerType::Vector,
            _ => crate::canvas::LayerType::Raster,
        };

        let vector_data = if let crate::canvas::LayerType::Vector = kind {
            lm.vector_strokes.map(|strokes| crate::canvas::VectorLayer { strokes })
        } else {
            None
        };

        let mut layer_tiles = Vec::new();
        for lt in &loaded_tiles {
            if lt.layer_id == lm.id {
                layer_tiles.push(LoadedTile {
                    layer_id: lt.layer_id,
                    tx: lt.tx,
                    ty: lt.ty,
                    pixels: lt.pixels.clone(),
                });
            }
        }

        layers.push(LoadedLayer {
            id: lm.id,
            name: lm.name,
            opacity: lm.opacity,
            visible: lm.visible,
            lock_alpha: lm.lock_alpha,
            is_clipping: lm.is_clipping,
            blend_mode,
            kind,
            tiles: layer_tiles,
            vector_data,
        });
    }

    Ok(LoadedDocument {
        canvas_width: doc_meta.canvas_width,
        canvas_height: doc_meta.canvas_height,
        layer_order: doc_meta.layer_order,
        layers,
    })
}
