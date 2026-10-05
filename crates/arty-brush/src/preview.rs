//! Real-engine stroke previews for the sub tool list.

use arty_core::{CompositeScratch, Document, TILE_SIZE, TileCoord, fix15, tile::new_tile_box};

use crate::engine::StrokeEngine;
use crate::input::InputSample;
use crate::preset::BrushPreset;

/// Paint an S-curve with a 0 → 1 → 0 pressure ramp and return premultiplied
/// RGBA8 pixels (`width × height`, transparent background; blending brushes
/// smear a backdrop of bars instead).
pub fn render_preview(preset: &BrushPreset, width: u32, height: u32, color: [f32; 3]) -> Vec<u8> {
    let mut doc = Document::new(width, height, 72);
    doc.set_paper(None);

    // The preview stroke has PREVIEW_SAMPLES samples: a small log suffices.
    let mut engine = StrokeEngine::with_log_capacity(256);
    paint_preview(&mut engine, &mut doc, preset, color);

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

/// Samples in the preview stroke.
const PREVIEW_SAMPLES: u32 = 91;

/// The preview stroke itself, through the real engine (taper and post
/// correction included, so the list shows them).
fn paint_preview(engine: &mut StrokeEngine, doc: &mut Document, preset: &BrushPreset, color: [f32; 3]) {
    let (w, h) = (doc.width() as f32, doc.height() as f32);
    let mut p = preset.clone();
    p.size = p.size.min(h * 0.55);
    p.stabilizer = 0;
    // Erasers preview as paint so the stroke is visible.
    p.eraser = false;
    // Taper lengths are document px; keep a long one from fading the whole
    // little stroke (its path is roughly 1.2 × the width).
    p.taper_in = p.taper_in.min(w * 0.35);
    p.taper_out = p.taper_out.min(w * 0.35);

    // A mostly-blending brush only moves color around and the preview page
    // starts empty: give it bars to smear.
    if p.blending > 0.5 {
        paint_backdrop(engine, doc, color, w, h);
    }
    engine.configure(&p, color);
    let n = PREVIEW_SAMPLES - 1;
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
            let _ = engine.begin(doc, s);
        } else {
            engine.feed(doc, s);
        }
    }
    engine.end(doc);
}

/// Upright bars across the preview's middle, alternating `color` and grey.
fn paint_backdrop(engine: &mut StrokeEngine, doc: &mut Document, color: [f32; 3], w: f32, h: f32) {
    let pen = BrushPreset { size: (h * 0.14).max(2.0), min_size: 1.0, hardness: 0.9, stabilizer: 0, ..Default::default() };
    for i in 0..6 {
        let x = w * (0.2 + 0.12 * i as f32);
        engine.configure(&pen, if i % 2 == 0 { color } else { [0.6; 3] });
        let at = |j: u32| InputSample {
            x,
            y: h * (0.15 + 0.7 * j as f32 / 8.0),
            pressure: 1.0,
            time: j as f64 * 0.008,
            ..Default::default()
        };
        if engine.begin(doc, at(0)).is_ok() {
            for j in 1..=8 {
                engine.feed(doc, at(j));
            }
            engine.end(doc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Reshape;
    use crate::preset::default_presets;

    #[test]
    fn preview_with_shaping_is_cheap() {
        let inking = default_presets().into_iter().find(|p| p.name == "Inking Pen").unwrap();
        let mut doc = Document::new(192, 48, 72);
        doc.set_paper(None);
        let mut engine = StrokeEngine::with_log_capacity(256);
        paint_preview(&mut engine, &mut doc, &inking, [0.0; 3]);
        // The whole stroke fit the small log and was reshaped from it.
        assert_eq!(engine.last_reshape(), Reshape::Full);
        assert!(engine.log_capacity() <= 256 && PREVIEW_SAMPLES as usize <= 256, "log grew to {}", engine.log_capacity());
    }
}
