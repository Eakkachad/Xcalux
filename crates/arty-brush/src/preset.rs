//! Brush presets ("sub tools") as plain data, compiled to hokusai brushes.
//!
//! Parameters are the ones an illustrator thinks in — size in pixels,
//! minimum size / opacity under light pressure, hardness, blending — in the
//! spirit of SAI and Clip Studio. [`BrushPreset::to_hokusai`] maps them onto
//! libmypaint settings.

use hokusai::mapping::{InputMapping, SettingValue};
use hokusai::{BrushInput, BrushSetting};
use serde::{Deserialize, Serialize};

/// Tool group a preset belongs to (CSP "tool" → "sub tool").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BrushGroup {
    Pen,
    Pencil,
    Brush,
    Airbrush,
    Blend,
    Eraser,
}

impl BrushGroup {
    pub const ALL: [BrushGroup; 6] = [
        BrushGroup::Pen,
        BrushGroup::Pencil,
        BrushGroup::Brush,
        BrushGroup::Airbrush,
        BrushGroup::Blend,
        BrushGroup::Eraser,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BrushGroup::Pen => "Pen",
            BrushGroup::Pencil => "Pencil",
            BrushGroup::Brush => "Brush",
            BrushGroup::Airbrush => "Airbrush",
            BrushGroup::Blend => "Blend",
            BrushGroup::Eraser => "Eraser",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushPreset {
    pub name: String,
    pub group: BrushGroup,
    /// Diameter in document pixels at full pressure.
    pub size: f32,
    /// Size at zero pressure as a fraction of `size` (1.0 = no size pressure).
    pub min_size: f32,
    /// 0..=1
    pub opacity: f32,
    /// Opacity at zero pressure as a fraction of `opacity` (1.0 = none).
    pub min_opacity: f32,
    /// Edge hardness, 0 (airbrush) ..= 1 (crisp).
    pub hardness: f32,
    /// Dabs per radius; higher gives smoother edges on fast strokes.
    pub density: f32,
    /// Picks up color already on the canvas (SAI "Blending"), 0..=1.
    pub blending: f32,
    /// How long picked-up color lingers (SAI "Persistence"), 0..=1.
    pub persistence: f32,
    /// Random size jitter, 0..=1.
    pub jitter: f32,
    pub eraser: bool,
    /// Stabilizer strength 0..=15 (SAI "S-levels").
    pub stabilizer: u8,
    /// Entry taper length in document px (CSP "Starting"); 0 = off. Applied live.
    pub taper_in: f32,
    /// Exit taper length in document px (CSP "Ending"); 0 = off. Applied at pen-up by replay.
    pub taper_out: f32,
    /// Post correction strength 0..=shape::MAX_CORRECTION; 0 = off.
    pub post_correction: u8,
}

impl Default for BrushPreset {
    fn default() -> Self {
        Self {
            name: "Brush".into(),
            group: BrushGroup::Brush,
            size: 10.0,
            min_size: 0.3,
            opacity: 1.0,
            min_opacity: 1.0,
            hardness: 0.8,
            density: 4.0,
            blending: 0.0,
            persistence: 0.5,
            jitter: 0.0,
            eraser: false,
            stabilizer: 3,
            taper_in: 0.0,
            taper_out: 0.0,
            post_correction: 0,
        }
    }
}

pub const MIN_BRUSH_SIZE: f32 = 0.5;
pub const MAX_BRUSH_SIZE: f32 = 2000.0;

/// Smallest optical dab radius (px) we let reach hokusai. libmypaint drops
/// dabs at or below half the 1 px `AntiAliasing`; just above it the dab is
/// a faint spike, so keep a margin (softened hardness 0.2 here).
const MIN_OPTICAL_RADIUS: f32 = 0.75;
/// Persistence 100% maps here; `SmudgeLength` must stay < 1 for the smudge
/// bucket to pick up canvas color at all.
const MAX_SMUDGE_LENGTH: f32 = 0.99;

/// libmypaint's documented defaults (`brushsettings.json`). hokusai zeroes
/// every setting, which would e.g. give dabs a 0 aspect ratio.
const LIBMYPAINT_DEFAULTS: &[(BrushSetting, f32)] = &[
    (BrushSetting::Opaque, 1.0),
    (BrushSetting::OpaqueLinearize, 0.9),
    (BrushSetting::Radius, 2.0),
    (BrushSetting::Hardness, 0.8),
    (BrushSetting::AntiAliasing, 1.0),
    (BrushSetting::DabsPerActualRadius, 2.0),
    (BrushSetting::Speed1Slowness, 0.04),
    (BrushSetting::Speed2Slowness, 0.8),
    (BrushSetting::Speed1Gamma, 4.0),
    (BrushSetting::Speed2Gamma, 4.0),
    (BrushSetting::OffsetBySpeedSlowness, 1.0),
    (BrushSetting::SmudgeLength, 0.5),
    (BrushSetting::StrokeDurationLogarithmic, 4.0),
    (BrushSetting::EllipticalDabRatio, 1.0),
    (BrushSetting::EllipticalDabAngle, 90.0),
    (BrushSetting::DirectionFilter, 2.0),
    (BrushSetting::PosterizeNum, 0.05),
    (BrushSetting::GridmapScaleX, 1.0),
    (BrushSetting::GridmapScaleY, 1.0),
];

fn constant(b: &mut hokusai::Brush, s: BrushSetting, v: f32) {
    b.set(s, SettingValue::constant(v));
}

/// `base + f(pressure)`, sampled at a few knots (libmypaint mappings are
/// piecewise linear).
fn pressure_curve(b: &mut hokusai::Brush, s: BrushSetting, base: f32, f: impl Fn(f32) -> f32) {
    let mut m = InputMapping::new(BrushInput::Pressure);
    m.points = [0.0f32, 0.015, 0.1, 0.25, 0.45, 0.7, 1.0].iter().map(|&p| (p, f(p))).collect();
    let mut v = SettingValue::constant(base);
    v.inputs.push(m);
    b.set(s, v);
}

impl BrushPreset {
    /// Upper bound of any dab's radius (px): the full-pressure radius
    /// `to_hokusai` sets (after its optical floor), grown by the largest size
    /// jitter hokusai's Gaussian (sum of 4 uniforms, ≤ √12 σ) can draw, plus
    /// a margin for the anti-aliasing bake (≤ 0.5 px).
    pub(crate) fn max_dab_radius(&self) -> f32 {
        let hardness = self.hardness.clamp(0.02, 1.0);
        let r_base = (self.size.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE) * 0.5)
            .max(0.2)
            .max(MIN_OPTICAL_RADIUS / (0.5 + 0.5 * hardness));
        r_base * (3.4642 * self.jitter.clamp(0.0, 1.0) * 0.4).exp() + 1.5
    }

    /// Compile to a hokusai brush painting `color` (sRGB, 0..=1).
    pub fn to_hokusai(&self, color: [f32; 3]) -> hokusai::Brush {
        let mut b = hokusai::Brush::new();
        for &(s, v) in LIBMYPAINT_DEFAULTS {
            constant(&mut b, s, v);
        }
        // Classic (non-spectral) mixing: our pixels are display-space.
        constant(&mut b, BrushSetting::Paint, 0.0);

        let radius = (self.size.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE) * 0.5).max(0.2);
        let min_size = self.min_size.clamp(0.01, 1.0);
        let size_pressure = min_size < 0.999;
        // libmypaint drops dabs with hardness <= 0, so 0% means "softest".
        let hardness = self.hardness.clamp(0.02, 1.0);
        // Anti-aliasing (1 px) turns a dab whose optical radius r·(½ + ½h) is
        // ≤ 0.5 px into negative hardness, and libmypaint drops it: small
        // sizes and the light end of a pen taper would paint nothing. Keep the
        // radius at the floor instead and pay the lost width back in opacity,
        // the way CSP draws sub-pixel lines.
        let r_floor = MIN_OPTICAL_RADIUS / (0.5 + 0.5 * hardness);
        let wanted = |p: f32| if size_pressure { radius * (min_size + (1.0 - min_size) * p) } else { radius };
        let thin = |p: f32| (wanted(p) / r_floor).min(1.0);
        let base = radius.max(r_floor);
        if size_pressure {
            // ln(max(r · lerp(min, 1, p), floor)) relative to the base.
            pressure_curve(&mut b, BrushSetting::Radius, base.ln(), |p| wanted(p).max(r_floor).ln() - base.ln());
        } else {
            constant(&mut b, BrushSetting::Radius, base.ln());
        }

        constant(&mut b, BrushSetting::Opaque, self.opacity.clamp(0.0, 1.0));
        let min_op = self.min_opacity.clamp(0.0, 1.0);
        if min_op < 0.999 || wanted(0.0) < r_floor {
            // Zero pressure (hover / lift-off) must not paint.
            pressure_curve(&mut b, BrushSetting::OpaqueMultiply, 0.0, |p| {
                let op = if p <= 0.0 {
                    0.0
                } else if min_op < 0.999 {
                    min_op + (1.0 - min_op) * p
                } else {
                    (p / 0.015).min(1.0) // as the touch-only curve below
                };
                op * thin(p)
            });
        } else {
            // Opaque only while the pen touches (pressure > 0).
            let mut m = InputMapping::new(BrushInput::Pressure);
            m.points = vec![(0.0, 0.0), (0.015, 1.0), (1.0, 1.0)];
            let mut v = SettingValue::constant(0.0);
            v.inputs.push(m);
            b.set(BrushSetting::OpaqueMultiply, v);
        }
        // Thin, dense dabs need less linearization to avoid faint lines.
        constant(&mut b, BrushSetting::OpaqueLinearize, if self.density > 3.0 { 0.35 } else { 0.9 });

        constant(&mut b, BrushSetting::Hardness, hardness);
        constant(&mut b, BrushSetting::DabsPerActualRadius, self.density.clamp(0.5, 12.0));
        constant(&mut b, BrushSetting::RadiusByRandom, self.jitter.clamp(0.0, 1.0) * 0.4);

        if self.blending > 0.0 {
            constant(&mut b, BrushSetting::Smudge, self.blending.clamp(0.0, 1.0));
            // persistence 1 → color lingers (smudge_length → 1). At exactly
            // 1.0 libmypaint never samples the canvas, so the bucket stays
            // transparent and every dab erases: top out just below it.
            constant(&mut b, BrushSetting::SmudgeLength, self.persistence.clamp(0.0, 1.0) * MAX_SMUDGE_LENGTH);
        }
        if self.eraser {
            constant(&mut b, BrushSetting::Eraser, 1.0);
        }

        let hsv = hokusai::color::rgb_to_hsv(color[0], color[1], color[2]);
        constant(&mut b, BrushSetting::ColorH, hsv.h);
        constant(&mut b, BrushSetting::ColorS, hsv.s);
        constant(&mut b, BrushSetting::ColorV, hsv.v);
        b
    }
}

/// The built-in sub tools, tuned for manga inking and coloring.
pub fn default_presets() -> Vec<BrushPreset> {
    let p = |name: &str, group, f: &dyn Fn(&mut BrushPreset)| {
        let mut b = BrushPreset { name: name.into(), group, ..Default::default() };
        f(&mut b);
        b
    };
    vec![
        p("G-Pen", BrushGroup::Pen, &|b| {
            b.size = 8.0;
            b.min_size = 0.12;
            b.hardness = 0.92;
            b.density = 6.0;
            b.stabilizer = 4;
        }),
        p("Inking Pen", BrushGroup::Pen, &|b| {
            b.size = 8.0;
            b.min_size = 0.12;
            b.hardness = 0.92;
            b.density = 6.0;
            b.stabilizer = 4;
            b.taper_in = 40.0;
            b.taper_out = 80.0;
            b.post_correction = 3;
        }),
        p("Mapping Pen", BrushGroup::Pen, &|b| {
            b.size = 3.5;
            b.min_size = 0.05;
            b.hardness = 0.95;
            b.density = 7.0;
            b.stabilizer = 6;
        }),
        p("Marker", BrushGroup::Pen, &|b| {
            b.size = 6.0;
            b.min_size = 1.0;
            b.hardness = 0.9;
            b.density = 6.0;
            b.stabilizer = 3;
        }),
        p("Pencil", BrushGroup::Pencil, &|b| {
            b.size = 5.0;
            b.min_size = 0.6;
            b.opacity = 0.9;
            b.min_opacity = 0.15;
            b.hardness = 0.55;
            b.density = 4.0;
            b.jitter = 0.15;
            b.stabilizer = 2;
        }),
        p("Sketch Pencil", BrushGroup::Pencil, &|b| {
            b.size = 12.0;
            b.min_size = 0.5;
            b.opacity = 0.55;
            b.min_opacity = 0.1;
            b.hardness = 0.35;
            b.density = 3.0;
            b.jitter = 0.3;
            b.stabilizer = 1;
        }),
        p("Brush", BrushGroup::Brush, &|b| {
            b.size = 24.0;
            b.min_size = 0.35;
            b.opacity = 0.95;
            b.hardness = 0.65;
            b.density = 4.0;
            b.blending = 0.45;
            b.persistence = 0.6;
            b.stabilizer = 2;
        }),
        p("Watercolor", BrushGroup::Brush, &|b| {
            b.size = 40.0;
            b.min_size = 0.5;
            b.opacity = 0.8;
            b.min_opacity = 0.3;
            b.hardness = 0.3;
            b.density = 3.0;
            b.blending = 0.55;
            b.persistence = 0.8;
            b.stabilizer = 1;
        }),
        p("Flat Color", BrushGroup::Brush, &|b| {
            b.size = 30.0;
            b.min_size = 0.8;
            b.hardness = 0.95;
            b.density = 4.0;
            b.stabilizer = 1;
        }),
        p("Airbrush", BrushGroup::Airbrush, &|b| {
            b.size = 120.0;
            b.min_size = 1.0;
            b.opacity = 0.45;
            b.min_opacity = 0.0;
            b.hardness = 0.0;
            b.density = 2.5;
            b.stabilizer = 0;
        }),
        p("Blender", BrushGroup::Blend, &|b| {
            b.size = 40.0;
            b.min_size = 0.6;
            b.opacity = 0.7;
            b.hardness = 0.4;
            b.density = 3.0;
            b.blending = 1.0;
            b.persistence = 0.7;
            b.stabilizer = 1;
        }),
        p("Hard Eraser", BrushGroup::Eraser, &|b| {
            b.size = 20.0;
            b.min_size = 0.6;
            b.hardness = 0.95;
            b.density = 4.0;
            b.eraser = true;
            b.stabilizer = 1;
        }),
        p("Soft Eraser", BrushGroup::Eraser, &|b| {
            b.size = 80.0;
            b.min_size = 1.0;
            b.opacity = 0.6;
            b.min_opacity = 0.1;
            b.hardness = 0.1;
            b.density = 2.5;
            b.eraser = true;
            b.stabilizer = 0;
        }),
    ]
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde::de::value::{Error, MapDeserializer};

    use super::*;

    #[test]
    fn presets_saved_before_shaping_load_with_shaping_off() {
        // A preset as stored before taper / post correction existed.
        let old = MapDeserializer::<_, Error>::new([("size", 12.0f32), ("opacity", 0.5)].into_iter());
        let p = BrushPreset::deserialize(old).unwrap();
        assert_eq!((p.size, p.opacity), (12.0, 0.5));
        assert_eq!((p.taper_in, p.taper_out, p.post_correction), (0.0, 0.0, 0));
    }

    #[test]
    fn only_inking_pen_shapes_by_default() {
        let names: Vec<_> = default_presets().into_iter().map(|p| p.name).collect();
        let shaped: Vec<_> = default_presets()
            .into_iter()
            .filter(|p| p.taper_in > 0.0 || p.taper_out > 0.0 || p.post_correction > 0)
            .map(|p| p.name)
            .collect();
        assert_eq!(shaped, ["Inking Pen"]);
        let g = names.iter().position(|n| n == "G-Pen").unwrap();
        assert_eq!(names[g + 1], "Inking Pen");
    }

    /// Largest radius among the dabs a brush draws (paints nothing).
    struct MaxDab {
        discard: Box<arty_core::TilePixels>,
        max: f32,
    }

    impl hokusai::TiledSurface for MaxDab {
        fn tile_request_start(&mut self, _tx: i32, _ty: i32) -> &mut hokusai::TilePixels {
            &mut self.discard
        }

        fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

        fn draw_dab(&mut self, dab: &hokusai::Dab) -> bool {
            self.max = self.max.max(dab.radius);
            false
        }
    }

    /// `max_dab_radius` bounds every dab, including thin brushes the optical
    /// floor widens (the Tail replay's clip relies on it).
    #[test]
    fn max_dab_radius_bounds_every_dab() {
        for size in [0.5, 1.0, 3.0, 40.0] {
            for hardness in [0.0, 0.5, 0.92, 1.0] {
                for jitter in [0.0, 0.5, 1.0] {
                    for min_size in [0.05, 1.0] {
                        let p = BrushPreset { size, hardness, jitter, min_size, ..Default::default() };
                        let brush = p.to_hokusai([0.0; 3]);
                        let mut state = hokusai::BrushState::default();
                        let mut surface = MaxDab { discard: arty_core::tile::new_tile_box(), max: 0.0 };
                        for i in 0..400 {
                            let t = i as f32;
                            let pressure = 0.5 + 0.5 * (t * 0.05).sin();
                            brush.stroke_to(&mut state, &mut surface, 20.0 + t, 60.0 + 10.0 * (t * 0.03).sin(), pressure, 0.0, 0.0, 0.005);
                        }
                        assert!(surface.max > 0.0, "{p:?} drew nothing");
                        assert!(surface.max <= p.max_dab_radius(), "{size} {hardness} {jitter} {min_size}: dab {} > bound {}", surface.max, p.max_dab_radius());
                    }
                }
            }
        }
    }
}
