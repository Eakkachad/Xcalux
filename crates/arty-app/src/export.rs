//! PNG export. Compositing runs in parallel on the calling thread; encoding
//! and file I/O happen on a background thread.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use arty_core::{CompositeScratch, Document, TILE_SIZE, TileCoord, fix15, tile::new_tile_box};
use rayon::prelude::*;

/// Which part of the page Export PNG writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportCrop {
    Canvas,
    Bleed,
    Trim,
}

impl ExportCrop {
    pub const ALL: [ExportCrop; 3] = [ExportCrop::Canvas, ExportCrop::Bleed, ExportCrop::Trim];

    pub fn label(self) -> &'static str {
        match self {
            ExportCrop::Canvas => "Canvas",
            ExportCrop::Bleed => "Bleed",
            ExportCrop::Trim => "Trim (finished size)",
        }
    }

    /// Trim when the page has a page setup, Canvas otherwise.
    pub fn default_for(doc: &Document) -> ExportCrop {
        if doc.page_setup().is_some() { ExportCrop::Trim } else { ExportCrop::Canvas }
    }
}

/// The pixel rectangle `(x0, y0, w, h)` a crop exports: guide rects are
/// rounded (`x0 = x.round()`, `x1 = (x + w).round()`) and clamped to the
/// canvas. Without a page setup (or when nothing is left) the whole canvas.
pub fn crop_rect(doc: &Document, crop: ExportCrop) -> (u32, u32, u32, u32) {
    let (w, h) = (doc.width(), doc.height());
    let r = match (crop, doc.page_setup()) {
        (ExportCrop::Trim, Some(p)) => p.trim,
        (ExportCrop::Bleed, Some(p)) => p.bleed_rect(w, h),
        _ => return (0, 0, w, h),
    };
    let clamp = |v: f32, max: u32| (v.round().max(0.0) as u32).min(max);
    let (x0, x1) = (clamp(r.x, w), clamp(r.x + r.w, w));
    let (y0, y1) = (clamp(r.y, h), clamp(r.y + r.h, h));
    if x1 <= x0 || y1 <= y0 {
        return (0, 0, w, h);
    }
    (x0, y0, x1 - x0, y1 - y0)
}

/// Flatten the `w`×`h` rectangle at `(x0, y0)` (inside the page) to
/// straight-alpha RGBA8, compositing only the tiles it touches, one band
/// of tile rows per task.
pub fn flatten_rgba_rect(doc: &Document, x0: u32, y0: u32, w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    if w == 0 || h == 0 {
        return out;
    }
    let (x0, y0) = (x0 as usize, y0 as usize);
    let row_bytes = w * 4;
    // Split the output at tile-row boundaries.
    let mut bands = Vec::new();
    let mut rest = out.as_mut_slice();
    let mut y = y0;
    while y < y0 + h {
        let end = ((y / TILE_SIZE + 1) * TILE_SIZE).min(y0 + h);
        let (band, tail) = rest.split_at_mut((end - y) * row_bytes);
        bands.push((y, band));
        rest = tail;
        y = end;
    }
    let (tx0, tx1) = (x0 / TILE_SIZE, (x0 + w - 1) / TILE_SIZE);
    bands.into_par_iter().for_each_init(
        || (CompositeScratch::new(), new_tile_box()),
        |(scratch, tile), (y, rows)| {
            let ty = y / TILE_SIZE;
            let ry = y - ty * TILE_SIZE;
            for tx in tx0..=tx1 {
                doc.composite_tile(TileCoord::new(tx as i32, ty as i32), tile, scratch);
                let tile_x = tx * TILE_SIZE;
                let (sx0, sx1) = (x0.max(tile_x), (x0 + w).min(tile_x + TILE_SIZE));
                for (r, row) in rows.chunks_mut(row_bytes).enumerate() {
                    for sx in sx0..sx1 {
                        let p = tile[ry + r][sx - tile_x];
                        let i = (sx - x0) * 4;
                        row[i..i + 4].copy_from_slice(&straight_rgba8(p));
                    }
                }
            }
        },
    );
    out
}

/// fix15 premultiplied → straight RGBA8 (as `flatten_rgba`).
fn straight_rgba8(p: [u16; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0; 4];
    }
    let un = |v: u16| fix15::to_u8(((v as u32 * fix15::ONE) / a).min(fix15::ONE) as u16);
    [un(p[0]), un(p[1]), un(p[2]), fix15::to_u8(p[3])]
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
/// yields a user-facing status message when done.
pub fn export_png(doc: &Document, crop: ExportCrop) -> Option<Receiver<String>> {
    let path = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("page.png").save_file()?;
    let (x0, y0, w, h) = crop_rect(doc, crop);
    let data = if (x0, y0, w, h) == (0, 0, doc.width(), doc.height()) {
        flatten_rgba(doc).2
    } else {
        flatten_rgba_rect(doc, x0, y0, w, h)
    };
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

    #[test]
    fn fr13_crops_and_rect_flattening() {
        use arty_core::{PageSetup, RectF};
        let mut doc = Document::new(300, 200, 350);
        assert_eq!(ExportCrop::default_for(&doc), ExportCrop::Canvas);
        for crop in ExportCrop::ALL {
            assert_eq!(crop_rect(&doc, crop), (0, 0, 300, 200), "{crop:?} without a page setup");
        }
        // Something different in every pixel, half transparent in places.
        let id = doc.active();
        let (grid, _) = doc.paint_target(id).unwrap();
        for ty in 0..4 {
            for tx in 0..5 {
                let t = grid.get_mut_or_create(TileCoord::new(tx, ty));
                for (i, p) in t.as_flattened_mut().iter_mut().enumerate() {
                    let a = if i % 3 == 0 { fix15::ONE_U16 } else { (i as u16 * 7) % fix15::ONE_U16 };
                    *p = [(i as u16 * 13 + tx as u16) % (a + 1), (i as u16) % (a + 1), (ty as u16 * 1000) % (a + 1), a];
                }
            }
        }
        doc.set_paper(None);
        let trim = RectF { x: 20.4, y: 13.6, w: 250.2, h: 170.0 };
        doc.set_page_unrecorded(Some(PageSetup { trim, bleed: 10.0, safe: 5.0, inner: RectF::default(), unit: 0 }));
        assert_eq!(ExportCrop::default_for(&doc), ExportCrop::Trim);
        assert_eq!(crop_rect(&doc, ExportCrop::Trim), (20, 14, 251, 170), "x1 = (20.4 + 250.2).round()");
        assert_eq!(crop_rect(&doc, ExportCrop::Bleed), (10, 4, 271, 190));
        assert_eq!(crop_rect(&doc, ExportCrop::Canvas), (0, 0, 300, 200));
        // Clamped to the canvas.
        doc.set_page_unrecorded(Some(PageSetup {
            trim: RectF { x: 2.0, y: 3.0, w: 296.0, h: 190.0 },
            bleed: 30.0,
            safe: 0.0,
            inner: RectF::default(),
            unit: 0,
        }));
        assert_eq!(crop_rect(&doc, ExportCrop::Bleed), (0, 0, 300, 200));

        let (w, h, full) = flatten_rgba(&doc);
        let rects = [(20, 14, 251, 170), (0, 0, 300, 200), (63, 64, 2, 1), (64, 0, 64, 64), (299, 199, 1, 1), (5, 70, 130, 129)];
        for (x0, y0, cw, ch) in rects {
            let part = flatten_rgba_rect(&doc, x0, y0, cw, ch);
            assert_eq!(part.len(), (cw * ch * 4) as usize);
            for y in 0..ch {
                let (src, dst, n) = (((y0 + y) * w + x0) as usize * 4, (y * cw) as usize * 4, cw as usize * 4);
                assert_eq!(&part[dst..dst + n], &full[src..src + n], "rect ({x0}, {y0}) {cw}×{ch} row {y}");
            }
        }
        assert_eq!(h, 200);
        assert!(flatten_rgba_rect(&doc, 0, 0, 0, 5).is_empty());
    }
}
