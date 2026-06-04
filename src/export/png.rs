use crate::canvas::Layer;
use ahash::AHashMap;
use std::path::Path;

#[derive(Clone)]
#[allow(dead_code)]
pub struct ExportPngOptions {
    #[allow(dead_code)]
    pub area: ExportArea,
    #[allow(dead_code)]
    pub background: ExportBackground,
    #[allow(dead_code)]
    pub scale: f32,
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum ExportArea {
    CanvasBounds,
    ArtworkBounds,
    Selection,
}

#[derive(Clone)]
pub enum ExportBackground {
    Transparent,
    White,
}

impl Default for ExportPngOptions {
    fn default() -> Self {
        Self {
            area: ExportArea::CanvasBounds,
            background: ExportBackground::White,
            scale: 1.0,
        }
    }
}

pub fn export_png(
    path: &Path,
    layers: &AHashMap<u32, Layer>,
    layer_order: &[u32],
    canvas_width: u32,
    canvas_height: u32,
    options: &ExportPngOptions,
) -> Result<(), String> {
    let scale = options.scale;
    let w = (canvas_width as f32 * scale) as u32;
    let h = (canvas_height as f32 * scale) as u32;

    let mut img: Vec<u8> = vec![0u8; (w * h * 4) as usize];

    // Composite layers from bottom to top
    for &layer_id in layer_order.iter().rev() {
        let layer = match layers.get(&layer_id) {
            Some(l) => l,
            None => continue,
        };
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }

        for (&(tx, ty), tile) in &layer.tiles {
            for ly in 0usize..64 {
                for lx in 0usize..64 {
                    let wx = (tx * 64 + lx as i32) as f32;
                    let wy = (ty * 64 + ly as i32) as f32;

                    let px = (wx * scale) as u32;
                    let py = (wy * scale) as u32;
                    if px >= w || py >= h {
                        continue;
                    }

                    let idx = ((py * w + px) * 4) as usize;
                    let pixel = tile.pixels[ly][lx];

                    // Fix15 to 8-bit
                    let src_r = ((pixel[0] as u32 * 255 + 16384) >> 15) as u8;
                    let src_g = ((pixel[1] as u32 * 255 + 16384) >> 15) as u8;
                    let src_b = ((pixel[2] as u32 * 255 + 16384) >> 15) as u8;
                    let src_a = ((pixel[3] as u32 * 255 + 16384) >> 15) as u8;

                    let dst_a = img[idx + 3] as u32;

                    let src_a_f = src_a as f32 / 255.0;
                    let dst_a_f = dst_a as f32 / 255.0;
                    let out_a = src_a_f + dst_a_f * (1.0 - src_a_f);

                    if out_a > 0.0 {
                        img[idx] = ((src_r as f32 * src_a_f + img[idx] as f32 * dst_a_f * (1.0 - src_a_f)) / out_a) as u8;
                        img[idx + 1] = ((src_g as f32 * src_a_f + img[idx + 1] as f32 * dst_a_f * (1.0 - src_a_f)) / out_a) as u8;
                        img[idx + 2] = ((src_b as f32 * src_a_f + img[idx + 2] as f32 * dst_a_f * (1.0 - src_a_f)) / out_a) as u8;
                        img[idx + 3] = (out_a * 255.0) as u8;
                    }
                }
            }
        }
    }

    // Apply background
    match options.background {
        ExportBackground::White => {
            for y in 0..h {
                for x in 0..w {
                    let idx = ((y * w + x) * 4) as usize;
                    if img[idx + 3] < 255 {
                        let a = img[idx + 3] as f32 / 255.0;
                        let inv_a = 1.0 - a;
                        img[idx] = (img[idx] as f32 * a + 255.0 * inv_a) as u8;
                        img[idx + 1] = (img[idx + 1] as f32 * a + 255.0 * inv_a) as u8;
                        img[idx + 2] = (img[idx + 2] as f32 * a + 255.0 * inv_a) as u8;
                        img[idx + 3] = 255;
                    }
                }
            }
        }
        ExportBackground::Transparent => {}
    }

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;

    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);

    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&img).map_err(|e| e.to_string())?;

    Ok(())
}
