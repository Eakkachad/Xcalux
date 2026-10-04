//! Real-engine stroke previews for the sub tool list.

use arty_core::{CompositeScratch, Document, TILE_SIZE, TileCoord, fix15, tile::new_tile_box};

use crate::engine::StrokeEngine;
use crate::input::InputSample;
use crate::preset::BrushPreset;

/// Paint an S-curve with a 0 → 1 → 0 pressure ramp and return premultiplied
/// RGBA8 pixels (`width × height`, transparent background).
pub fn render_preview(preset: &BrushPreset, width: u32, height: u32, color: [f32; 3]) -> Vec<u8> {
    let mut doc = Document::new(width, height, 72);
    doc.set_paper(None);

    let mut p = preset.clone();
    p.size = p.size.min(height as f32 * 0.55);
    p.stabilizer = 0;
    // Erasers preview as paint so the stroke is visible.
    p.eraser = false;

    let mut engine = StrokeEngine::new();
    engine.configure(&p, color);
    let (w, h) = (width as f32, height as f32);
    let n = 90;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let s = InputSample {
            x: w * (0.08 + 0.84 * t),
            y: h * 0.5 + h * 0.22 * (t * std::f32::consts::TAU).sin(),
            pressure: (t * std::f32::consts::PI).sin().max(0.0),
            time: i as f64 * 0.008,
            ..Default::default()
        };
        if i == 0 {
            let _ = engine.begin(&mut doc, s);
        } else {
            engine.feed(&mut doc, s);
        }
    }
    engine.end(&mut doc);

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut tile = new_tile_box();
    let mut scratch = CompositeScratch::new();
    for ty in 0..doc.tiles_high() {
        for tx in 0..doc.tiles_wide() {
            let c = TileCoord::new(tx as i32, ty as i32);
            doc.composite_tile(c, &mut tile, &mut scratch);
            let (ox, oy) = c.origin();
            for y in 0..TILE_SIZE {
                let py = oy as u32 + y as u32;
                if py >= height {
                    break;
                }
                for x in 0..TILE_SIZE {
                    let px = ox as u32 + x as u32;
                    if px >= width {
                        break;
                    }
                    let i = ((py * width + px) * 4) as usize;
                    let v = tile[y][x];
                    rgba[i..i + 4].copy_from_slice(&v.map(fix15::to_u8));
                }
            }
        }
    }
    rgba
}
