//! Stroke engine: stabilized samples → hokusai dabs → layer tiles.

use arty_core::{Document, Edit, LayerId, PixelRecorder, TilePixels, tile::new_tile_box};
use hokusai::mapping::SettingValue;
use hokusai::{BrushSetting, BrushState};

use crate::input::{InputSample, Stabilizer};
use crate::preset::BrushPreset;
use crate::surface::LayerSurface;

/// Why a stroke could not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeRefused {
    NotRaster,
    Locked,
    Hidden,
    /// An eraser on a layer whose transparency is locked: it could only
    /// change alpha, which the lock forbids.
    AlphaLocked,
}

pub struct StrokeEngine {
    brush: hokusai::Brush,
    state: BrushState,
    stabilizer: Stabilizer,
    recorder: PixelRecorder,
    discard: Box<TilePixels>,
    layer: Option<LayerId>,
    last_time: f64,
    eraser: bool,
    /// Whether the stroke has laid down any dab yet.
    painted: bool,
    /// Highest pressure fed this stroke, for the dot a tap leaves.
    peak_pressure: f32,
}

impl Default for StrokeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StrokeEngine {
    pub fn new() -> Self {
        Self {
            brush: BrushPreset::default().to_hokusai([0.0; 3]),
            state: BrushState::default(),
            stabilizer: Stabilizer::default(),
            recorder: PixelRecorder::default(),
            discard: new_tile_box(),
            layer: None,
            last_time: 0.0,
            eraser: false,
            painted: false,
            peak_pressure: 0.0,
        }
    }

    /// Load a preset + color. Allocates; call between strokes, not per sample.
    pub fn configure(&mut self, preset: &BrushPreset, color: [f32; 3]) {
        self.brush = preset.to_hokusai(color);
        self.eraser = preset.eraser;
        self.stabilizer.set_level(preset.stabilizer);
    }

    pub fn set_stabilizer(&mut self, level: u8) {
        self.stabilizer.set_level(level);
    }

    pub fn is_stroking(&self) -> bool {
        self.layer.is_some()
    }

    /// Start a stroke on the active layer.
    pub fn begin(&mut self, doc: &mut Document, s: InputSample) -> Result<(), StrokeRefused> {
        self.layer = None;
        let layer = doc.active_layer();
        if layer.is_folder() {
            return Err(StrokeRefused::NotRaster);
        }
        if layer.props.locked {
            return Err(StrokeRefused::Locked);
        }
        if !layer.props.visible {
            return Err(StrokeRefused::Hidden);
        }
        if layer.props.lock_alpha && self.eraser {
            return Err(StrokeRefused::AlphaLocked);
        }
        let lock = if layer.props.lock_alpha { 1.0 } else { 0.0 };
        self.brush.set(BrushSetting::LockAlpha, SettingValue::constant(lock));

        let id = layer.id;
        self.layer = Some(id);
        self.recorder.begin(id);
        self.state.reset();
        self.stabilizer.reset();
        self.last_time = s.time;
        self.painted = false;
        self.peak_pressure = s.pressure;
        let s = self.stabilizer.push(s);
        self.dab(doc, s, 0.0);
        Ok(())
    }

    /// Feed one raw sample. Allocation-free once the stroke's tiles exist.
    pub fn feed(&mut self, doc: &mut Document, s: InputSample) {
        if self.layer.is_none() {
            return;
        }
        let dt = s.time - self.last_time;
        self.last_time = s.time;
        self.peak_pressure = self.peak_pressure.max(s.pressure);
        let smoothed = self.stabilizer.push(s);
        self.dab(doc, smoothed, dt);
    }

    /// Finish the stroke; returns the undo entry if anything was painted.
    pub fn end(&mut self, doc: &mut Document) -> Option<Edit> {
        self.layer?;
        // Let a stabilized line catch up to the pen-up point.
        while let Some(s) = self.stabilizer.drain_step() {
            self.dab(doc, s, 0.004);
        }
        self.painted |= self.with_surface(doc, |brush, state, surface| brush.finish_stroke(state, surface)) == Some(true);
        // A tap never crosses a dab spacing, so hokusai has drawn nothing;
        // SAI and CSP leave a dot. Mark one whole dab as due: hokusai then
        // draws it in place (zero step) at the entry pressure. The touch
        // lift-off sample has pressure 0, so use the stroke's peak instead.
        let p = self.peak_pressure;
        if !self.painted && p > 0.0 {
            self.with_surface(doc, |brush, state, surface| {
                state.dist_past_dab = 1.0;
                state.last_pressure = p;
                let (x, y) = (state.last_event_x, state.last_event_y);
                brush.stroke_to(state, surface, x, y, p, 0.0, 0.0, 0.004);
            });
        }
        self.layer = None;
        self.recorder.finish()
    }

    /// Abort the stroke, restoring the layer as it was.
    pub fn cancel(&mut self, doc: &mut Document) {
        if self.layer.take().is_some()
            && let Some(edit) = self.recorder.finish() {
                let mut h = arty_core::History::new(1);
                h.push(edit);
                h.undo(doc);
            }
    }

    /// Run `f` with a surface over the stroke's layer.
    fn with_surface<R>(
        &mut self,
        doc: &mut Document,
        f: impl FnOnce(&hokusai::Brush, &mut BrushState, &mut LayerSurface) -> R,
    ) -> Option<R> {
        let id = self.layer?;
        let (tiles_wide, tiles_high) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
        let (grid, dirty) = doc.paint_target(id)?;
        let mut surface = LayerSurface {
            grid,
            dirty,
            recorder: &mut self.recorder,
            discard: &mut self.discard,
            tiles_wide,
            tiles_high,
        };
        Some(f(&self.brush, &mut self.state, &mut surface))
    }

    fn dab(&mut self, doc: &mut Document, s: InputSample, dt: f64) {
        let dt = dt.clamp(0.0005, 1.0);
        let painted = self.with_surface(doc, |brush, state, surface| {
            brush.stroke_to(state, surface, s.x, s.y, s.pressure, s.tilt_x, s.tilt_y, dt)
        });
        self.painted |= painted == Some(true);
    }
}
