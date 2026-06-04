use crate::app::{BrushPreset, PresetIcon};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use serde::{Serialize, Deserialize};

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct ArtyBrushMetadata {
    pub name: String,
    pub icon: String,
    pub radius_log: f32,
    pub opacity: f32,
    pub hardness: f32,
    pub min_size_fraction: f32,
    pub color_blending: f32,
    pub dilution: f32,
    pub is_eraser: bool,
    pub texture_scale: f32,
}

#[allow(dead_code)]
fn icon_to_str(icon: PresetIcon) -> &'static str {
    match icon {
        PresetIcon::Pencil => "Pencil",
        PresetIcon::InkPen => "InkPen",
        PresetIcon::PaintBrush => "PaintBrush",
        PresetIcon::Smudge => "Smudge",
        PresetIcon::Eraser => "Eraser",
        PresetIcon::AirBrush => "AirBrush",
        PresetIcon::Water => "Water",
        PresetIcon::Marker => "Marker",
        PresetIcon::BinaryPen => "BinaryPen",
    }
}

fn str_to_icon(s: &str) -> PresetIcon {
    match s {
        "Pencil" => PresetIcon::Pencil,
        "InkPen" => PresetIcon::InkPen,
        "PaintBrush" => PresetIcon::PaintBrush,
        "Smudge" => PresetIcon::Smudge,
        "AirBrush" => PresetIcon::AirBrush,
        "Water" => PresetIcon::Water,
        "Marker" => PresetIcon::Marker,
        "BinaryPen" => PresetIcon::BinaryPen,
        _ => PresetIcon::Eraser,
    }
}

pub fn decode_png_to_grayscale(png_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoder = png::Decoder::new(png_bytes);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;

    let bytes = &buf[..info.buffer_size()];
    let w = info.width;
    let h = info.height;

    let mut grayscale = Vec::with_capacity((w * h) as usize);

    match info.color_type {
        png::ColorType::Grayscale => {
            grayscale.extend_from_slice(bytes);
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in bytes.chunks_exact(2) {
                grayscale.push(chunk[0]);
            }
        }
        png::ColorType::Rgb => {
            for chunk in bytes.chunks_exact(3) {
                let g = (0.299 * chunk[0] as f32 + 0.587 * chunk[1] as f32 + 0.114 * chunk[2] as f32) as u8;
                grayscale.push(g);
            }
        }
        png::ColorType::Rgba => {
            for chunk in bytes.chunks_exact(4) {
                let g = (0.299 * chunk[0] as f32 + 0.587 * chunk[1] as f32 + 0.114 * chunk[2] as f32) as u8;
                let a = chunk[3] as f32 / 255.0;
                grayscale.push((g as f32 * a) as u8);
            }
        }
        _ => return Err("Unsupported color type".to_string()),
    }

    Ok((grayscale, w, h))
}

pub fn load_artybrush(path: &Path, textures_registry: &mut Vec<crate::app::BrushTexture>) -> std::io::Result<BrushPreset> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"ARTB" {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid magic bytes"));
    }

    let mut sizes = [0u8; 12];
    file.read_exact(&mut sizes)?;
    let meta_size = u32::from_le_bytes(sizes[0..4].try_into().unwrap()) as usize;
    let tex_size = u32::from_le_bytes(sizes[4..8].try_into().unwrap()) as usize;
    let bristle_size = u32::from_le_bytes(sizes[8..12].try_into().unwrap()) as usize;

    let mut meta_bytes = vec![0u8; meta_size];
    file.read_exact(&mut meta_bytes)?;
    let meta: ArtyBrushMetadata = serde_json::from_slice(&meta_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut tex_bytes = vec![0u8; tex_size];
    if tex_size > 0 {
        file.read_exact(&mut tex_bytes)?;
    }

    let mut bristle_bytes = vec![0u8; bristle_size];
    if bristle_size > 0 {
        file.read_exact(&mut bristle_bytes)?;
    }

    let texture_id = if tex_size > 0 {
        match decode_png_to_grayscale(&tex_bytes) {
            Ok((gray_bytes, w, h)) => {
                let mut final_bytes = vec![255u8; 256 * 256];
                for y in 0..h.min(256) {
                    for x in 0..w.min(256) {
                        final_bytes[(y * 256 + x) as usize] = gray_bytes[(y * w + x) as usize];
                    }
                }
                let name = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Imported")
                    .to_string();
                textures_registry.push(crate::app::BrushTexture {
                    name: format!("[imported] {}", name),
                    width: 256,
                    height: 256,
                    pixels: final_bytes,
                });
                (textures_registry.len() - 1) as u32
            }
            Err(_) => 0,
        }
    } else {
        0
    };

    let bristle_id = if bristle_size > 0 {
        1
    } else {
        0
    };

    Ok(BrushPreset {
        id: 0,
        name: meta.name,
        icon: str_to_icon(&meta.icon),
        radius_log: meta.radius_log,
        opacity: meta.opacity,
        hardness: meta.hardness,
        min_size_fraction: meta.min_size_fraction,
        color_blending: meta.color_blending,
        dilution: meta.dilution,
        is_eraser: meta.is_eraser,
        texture_id,
        texture_scale: meta.texture_scale,
        bristle_id,
        stabilizer_level: crate::input::StabilizerLevel::default(),
        stabilizer_mode: crate::input::StabilizerMode::SpringMassDamper,
        spacing: 2.0,
        density: 1.0,
    })
}

pub fn extract_sut_texture(sut_path: &Path) -> std::io::Result<(Vec<u8>, u32, u32)> {
    let mut file = File::open(sut_path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let png_header = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let png_footer = &[0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];

    if let Some(start_pos) = buf.windows(8).position(|w| w == png_header) {
        if let Some(end_pos) = buf[start_pos..].windows(8).position(|w| w == png_footer) {
            let png_bytes = &buf[start_pos..start_pos + end_pos + 8];
            match decode_png_to_grayscale(png_bytes) {
                Ok((gray_bytes, w, h)) => Ok((gray_bytes, w, h)),
                Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            }
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "No PNG footer found in SUT file"))
        }
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "No PNG header found in SUT file"))
    }
}
