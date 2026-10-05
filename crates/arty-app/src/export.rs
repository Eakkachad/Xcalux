//! PNG export. Compositing runs in parallel on the calling thread; encoding
//! and file I/O happen on a background thread.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use arty_core::{CompositeScratch, Document, TILE_SIZE, TileCoord, fix15, tile::new_tile_box};
use rayon::prelude::*;

/// Which part of the page Export PNG writes.
// FRAMES: the Export dialog offers Bleed and Trim once page setups exist.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportCrop {
    Canvas,
    Bleed,
    Trim,
}

/// Flatten the page to straight-alpha RGBA8.
pub fn flatten_rgba(doc: &Document) -> (u32, u32, Vec<u8>) {
    let (w, h) = (doc.width(), doc.height());
    let mut out = vec![0u8; (w * h * 4) as usize];
    let row_bytes = (w * 4) as usize;
    let band = row_bytes * TILE_SIZE;
    out.par_chunks_mut(band).enumerate().for_each_init(
        || (CompositeScratch::new(), new_tile_box()),
        |(scratch, tile), (ty, rows)| {
            for tx in 0..doc.tiles_wide() {
                let c = TileCoord::new(tx as i32, ty as i32);
                doc.composite_tile(c, tile, scratch);
                let x0 = tx as usize * TILE_SIZE;
                let cols = TILE_SIZE.min(w as usize - x0);
                for (y, row) in rows.chunks_mut(row_bytes).enumerate() {
                    for x in 0..cols {
                        let p = tile[y][x];
                        let a = p[3] as u32;
                        let i = (x0 + x) * 4;
                        if a == 0 {
                            row[i..i + 4].copy_from_slice(&[0; 4]);
                            continue;
                        }
                        // Un-premultiply for PNG.
                        for k in 0..3 {
                            row[i + k] = fix15::to_u8(((p[k] as u32 * fix15::ONE) / a).min(fix15::ONE) as u16);
                        }
                        row[i + 3] = fix15::to_u8(p[3]);
                    }
                }
            }
        },
    );
    (w, h, out)
}

fn write_png(path: &PathBuf, w: u32, h: u32, dpi: u32, data: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let ppm = (dpi as f64 / 0.0254).round() as u32;
    enc.set_pixel_dims(Some(png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter }));
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(data).map_err(|e| e.to_string())
}

/// Ask for a destination, then encode in the background. The receiver
/// yields a user-facing status message when done. (Every crop exports the
/// whole canvas for now.)
pub fn export_png(doc: &Document, _crop: ExportCrop) -> Option<Receiver<String>> {
    let path = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("page.png").save_file()?;
    let (w, h, data) = flatten_rgba(doc);
    let dpi = doc.dpi();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let msg = match write_png(&path, w, h, dpi, &data) {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
        let _ = tx.send(msg);
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_blank_page_is_white_and_sized() {
        let doc = Document::new(100, 70, 72);
        let (w, h, px) = flatten_rgba(&doc);
        assert_eq!((w, h), (100, 70));
        assert_eq!(px.len(), 100 * 70 * 4);
        assert!(px.iter().all(|&b| b == 255));
    }
}
