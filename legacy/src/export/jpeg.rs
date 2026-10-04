use crate::canvas::Layer;
use ahash::AHashMap;
use std::path::Path;
use std::fs::File;
use std::io::BufWriter;
use image::codecs::jpeg::JpegEncoder;
use image::{ColorType, ImageEncoder};

pub fn export_jpeg(
    path: &Path,
    layers: &AHashMap<u32, Layer>,
    layer_order: &[u32],
    canvas_width: u32,
    canvas_height: u32,
    quality: u8,
) -> Result<(), String> {
    let w = canvas_width;
    let h = canvas_height;

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
                    let wx = tx * 64 + lx as i32;
                    let wy = ty * 64 + ly as i32;

                    if wx < 0 || wy < 0 {
                        continue;
                    }
                    let px = wx as u32;
                    let py = wy as u32;
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
                        img[idx] = ((src_r as f32 * src_a_f
                            + img[idx] as f32 * dst_a_f * (1.0 - src_a_f))
                            / out_a) as u8;
                        img[idx + 1] = ((src_g as f32 * src_a_f
                            + img[idx + 1] as f32 * dst_a_f * (1.0 - src_a_f))
                            / out_a) as u8;
                        img[idx + 2] = ((src_b as f32 * src_a_f
                            + img[idx + 2] as f32 * dst_a_f * (1.0 - src_a_f))
                            / out_a) as u8;
                        img[idx + 3] = (out_a * 255.0) as u8;
                    }
                }
            }
        }
    }

    // Convert RGBA to RGB by blending onto a white background
    let mut rgb_img: Vec<u8> = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let src_idx = ((y * w + x) * 4) as usize;
            let dst_idx = ((y * w + x) * 3) as usize;

            let a = img[src_idx + 3] as f32 / 255.0;
            let inv_a = 1.0 - a;

            rgb_img[dst_idx] = (img[src_idx] as f32 * a + 255.0 * inv_a) as u8;
            rgb_img[dst_idx + 1] = (img[src_idx + 1] as f32 * a + 255.0 * inv_a) as u8;
            rgb_img[dst_idx + 2] = (img[src_idx + 2] as f32 * a + 255.0 * inv_a) as u8;
        }
    }

    let file = File::create(path).map_err(|e| e.to_string())?;
    let writer = BufWriter::new(file);

    let encoder = JpegEncoder::new_with_quality(writer, quality);
    encoder.write_image(&rgb_img, w, h, ColorType::Rgb8).map_err(|e| e.to_string())?;

    Ok(())
}
