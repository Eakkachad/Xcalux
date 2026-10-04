//! `ARTY_DEMO=1`: paint sample strokes at startup — a smoke test for the
//! brush → composite → GPU path that needs no pointer input.

use arty_brush::InputSample;

use crate::studio::Studio;

pub fn paint_sample_strokes(studio: &mut Studio) {
    let (w, h) = (studio.doc.width() as f32, studio.doc.height() as f32);
    let colors = [[0.05, 0.05, 0.08], [0.85, 0.25, 0.3], [0.2, 0.45, 0.85], [0.95, 0.7, 0.2]];
    for i in 0..studio.presets.len() {
        if studio.presets[i].eraser {
            continue;
        }
        studio.select_preset(i);
        studio.set_main_color(colors[i % colors.len()]);
        let y0 = h * (0.08 + 0.075 * i as f32);
        let n = 160;
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let s = InputSample {
                x: w * (0.1 + 0.8 * t),
                y: y0 + h * 0.025 * (t * std::f32::consts::TAU * 1.5).sin(),
                pressure: (t * std::f32::consts::PI).sin(),
                time: k as f64 * 0.006,
                ..Default::default()
            };
            if k == 0 {
                studio.begin_stroke(s);
            } else {
                studio.feed_stroke(s);
            }
        }
        studio.end_stroke();
    }
    studio.select_preset(0);
    studio.set_main_color([0.0; 3]);
}
