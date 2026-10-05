//! Stroke engine: stabilized samples → hokusai dabs → layer tiles.

use arty_core::{Document, Edit, LayerId, PixelRecorder, TilePixels, tile::new_tile_box};
use hokusai::mapping::SettingValue;
use hokusai::{BrushSetting, BrushState};

use crate::input::{InputSample, Stabilizer};
use crate::preset::BrushPreset;
use crate::shape::{
    self, DabStats, MAX_FULL_REPLAY_PX, MAX_LOGGED_SAMPLES, ShapeSample, TileClip, correct_path, correction_sigma_px,
    seg_len, taper,
};
use crate::surface::{LayerSurface, MaskCur};

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

/// What `end()` did to shape the finished stroke (exit taper / post correction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reshape {
    /// Nothing to reshape (shaping off, a tap, nothing painted, or no visible change).
    #[default]
    Skipped,
    /// Shaping was wanted but the stroke was too long or too costly; kept as drawn.
    TooLong,
    /// Only the tiles near the end were restored and repainted.
    Tail { tiles: u32 },
    /// Every stroke tile was restored and the whole stroke repainted.
    Full,
}

/// Which logged path a replay paints.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayPath {
    Log,
    Corrected,
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
    /// (taper_in, taper_out, post_correction) of the configured preset.
    shape: (f32, f32, u8),
    /// The brush smudges (reads the canvas), so only a Full replay is exact.
    blending: bool,
    /// Largest dab radius the configured brush can draw, for Tail clipping.
    max_radius: f32,
    /// Brush state at stroke start; a replay restarts from it (same RNG).
    state0: BrushState,
    /// Whether this stroke logs its samples (exit taper or post correction on).
    logging: bool,
    log: Vec<ShapeSample>,
    log_cap: usize,
    log_overflow: bool,
    /// Arc length painted so far (document px) and the previous sample.
    arc: f32,
    prev: Option<ShapeSample>,
    /// First painted sample and the farthest any later one got from it (px).
    origin: ShapeSample,
    extent: f32,
    corrected: Vec<ShapeSample>,
    scratch: Vec<[f32; 2]>,
    clip: TileClip,
    stats: DabStats,
    view_zoom: f32,
    reshape: Reshape,
}

impl Default for StrokeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StrokeEngine {
    pub fn new() -> Self {
        Self::with_log_capacity(MAX_LOGGED_SAMPLES)
    }

    /// An engine whose strokes log at most `n` samples; longer shaped
    /// strokes are kept as drawn ([`Reshape::TooLong`]).
    pub(crate) fn with_log_capacity(n: usize) -> Self {
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
            shape: (0.0, 0.0, 0),
            blending: false,
            max_radius: 0.0,
            state0: BrushState::default(),
            logging: false,
            log: Vec::new(),
            log_cap: n,
            log_overflow: false,
            arc: 0.0,
            prev: None,
            origin: ShapeSample::default(),
            extent: 0.0,
            corrected: Vec::new(),
            scratch: Vec::new(),
            clip: TileClip::default(),
            stats: DabStats::default(),
            view_zoom: 1.0,
            reshape: Reshape::Skipped,
        }
    }

    /// Load a preset + color. Allocates; call between strokes, not per sample.
    pub fn configure(&mut self, preset: &BrushPreset, color: [f32; 3]) {
        self.brush = preset.to_hokusai(color);
        self.eraser = preset.eraser;
        self.stabilizer.set_level(preset.stabilizer);
        let len = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
        self.shape = (len(preset.taper_in), len(preset.taper_out), preset.post_correction.min(shape::MAX_CORRECTION));
        self.blending = preset.blending > 0.0;
        self.max_radius = preset.max_dab_radius();
    }

    pub fn set_stabilizer(&mut self, level: u8) {
        self.stabilizer.set_level(level);
    }

    /// Screen pixels per document pixel at stroke start (post correction is screen-relative).
    pub fn set_view_zoom(&mut self, zoom: f32) {
        self.view_zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    }

    pub fn last_reshape(&self) -> Reshape {
        self.reshape
    }

    /// Dabs drawn so far by the current (or last) stroke, replay included.
    pub fn dab_stats(&self) -> DabStats {
        self.stats
    }

    #[cfg(test)]
    pub(crate) fn log_capacity(&self) -> usize {
        self.log.capacity()
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
        self.state0 = self.state.clone();
        let (_, taper_out, correction) = self.shape;
        self.logging = taper_out > 0.0 || correction > 0;
        self.log.clear();
        if self.logging && self.log.capacity() < self.log_cap {
            // Reserved once per engine: `feed` must never grow it.
            self.log.reserve_exact(self.log_cap - self.log.len());
        }
        self.log_overflow = false;
        self.arc = 0.0;
        self.prev = None;
        self.extent = 0.0;
        self.stats = DabStats::default();
        self.reshape = Reshape::Skipped;
        self.stabilizer.reset();
        self.last_time = s.time;
        self.painted = false;
        self.peak_pressure = s.pressure;
        let s = self.stabilizer.push(s);
        self.paint(doc, s, 0.0);
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
        self.paint(doc, smoothed, dt);
    }

    /// Finish the stroke; returns the undo entry if anything was painted.
    ///
    /// With exit taper or post correction on, the finished stroke is
    /// repainted from its log here ([`StrokeEngine::last_reshape`] says how).
    pub fn end(&mut self, doc: &mut Document) -> Option<Edit> {
        self.layer?;
        // Let a stabilized line catch up to the pen-up point.
        while let Some(s) = self.stabilizer.drain_step() {
            self.paint(doc, s, 0.004);
        }
        self.painted |= self.with_surface(doc, false, |brush, state, surface| brush.finish_stroke(state, surface))
            == Some(true);
        let p = self.peak_pressure;
        // With a taper, a tap whose pen skids a little paints only a faint
        // speck: the pressure ramps from 0 over the taper length, which the
        // skid never covers. A tapered stroke that never leaves its own dot is
        // a tap: put the stroke's tiles back and leave the dot below instead.
        // The path length must be short too, so a deliberate scribble in
        // place (building up coverage) is kept.
        let (tin, tout, _) = self.shape;
        let te = self.tap_extent();
        if self.painted && p > 0.0 && (tin > 0.0 || tout > 0.0) && self.extent < te && self.arc < 2.0 * te {
            if let Some(id) = self.layer
                && let Some((grid, dirty)) = doc.paint_target(id)
            {
                self.recorder.restore(grid, dirty, |_| true);
            }
            self.state = self.state0.clone();
            self.painted = false;
            // Seed the fresh brush state at the touch-down point (no dab).
            let o = self.origin;
            self.with_surface(doc, false, |brush, state, surface| {
                brush.stroke_to(state, surface, o.x, o.y, p, o.tilt_x, o.tilt_y, 0.004)
            });
        }
        // A tap never crosses a dab spacing, so hokusai has drawn nothing;
        // SAI and CSP leave a dot. Mark one whole dab as due: hokusai then
        // draws it in place (zero step) at the entry pressure. The touch
        // lift-off sample has pressure 0, so use the stroke's peak instead.
        let was_tap = !self.painted && p > 0.0;
        if was_tap {
            self.with_surface(doc, false, |brush, state, surface| {
                state.dist_past_dab = 1.0;
                state.last_pressure = p;
                let (x, y) = (state.last_event_x, state.last_event_y);
                brush.stroke_to(state, surface, x, y, p, 0.0, 0.0, 0.004);
            });
        }
        self.reshape = self.reshape_stroke(doc, was_tap);
        self.layer = None;
        self.recorder.finish()
    }

    /// Abort the stroke, restoring the layer as it was.
    pub fn cancel(&mut self, doc: &mut Document) {
        if self.layer.take().is_some()
            && let Some(edit) = self.recorder.finish() {
                let mut h = arty_core::History::new(1);
                h.push(edit, doc);
                h.undo(doc);
            }
    }

    /// How far (document px) a tapered stroke may get from its first point
    /// and still count as a tap: half the largest dab radius or one screen
    /// px, whichever is more, but never a whole taper length.
    fn tap_extent(&self) -> f32 {
        let (tin, tout, _) = self.shape;
        (0.5 * self.max_radius).max(1.0 / self.view_zoom).min(tin.max(tout))
    }

    /// Apply exit taper / post correction to the finished stroke.
    fn reshape_stroke(&mut self, doc: &mut Document, was_tap: bool) -> Reshape {
        let total = self.arc;
        let (_, tout, corr) = self.shape;
        if (tout == 0.0 && corr == 0) || was_tap || !self.painted || total < 1.0 || self.log.len() < 3 {
            return Reshape::Skipped;
        }
        if self.log_overflow {
            return Reshape::TooLong;
        }
        let affordable = self.stats.px <= MAX_FULL_REPLAY_PX;
        if corr > 0 {
            let sigma = correction_sigma_px(corr, self.view_zoom);
            let shift = correct_path(&self.log, sigma, &mut self.corrected, &mut self.scratch);
            if shift < 0.05 && tout == 0.0 {
                return Reshape::Skipped;
            }
            if affordable {
                self.replay(doc, ReplayPath::Corrected, false);
                return Reshape::Full;
            }
            if tout == 0.0 {
                return Reshape::TooLong;
            }
            // Too costly to repaint whole: still give the raw line its exit taper.
        }
        if self.blending {
            // Smudge reads the canvas, so a clipped replay would differ.
            if !affordable {
                return Reshape::TooLong;
            }
            self.replay(doc, ReplayPath::Log, false);
            return Reshape::Full;
        }
        // Tail: only samples within `tout` of the end change, so only the
        // tiles their dabs reach need repainting.
        let mut s = 0.0f32;
        let mut i0 = self.log.len() - 1;
        for i in 0..self.log.len() {
            if i > 0 {
                s += seg_len(&self.log[i - 1], &self.log[i]);
            }
            if total - s < tout {
                i0 = i;
                break;
            }
        }
        self.clip.build(&self.log[i0.saturating_sub(1)..], self.max_radius);
        if self.clip.is_empty() {
            return Reshape::TooLong;
        }
        // A clipped replay still renders every dab that reaches the clip in
        // full. That is bounded by the live cost; when even that is over
        // budget (huge brushes), count the clipped cost first without painting.
        if !affordable && self.tail_cost() > MAX_FULL_REPLAY_PX {
            return Reshape::TooLong;
        }
        let mut tiles = 0u32;
        if let Some(id) = self.layer
            && let Some((grid, dirty)) = doc.paint_target(id)
        {
            let clip = &self.clip;
            self.recorder.restore(grid, dirty, |c| {
                let hit = clip.contains(c);
                tiles += hit as u32;
                hit
            });
        }
        self.replay(doc, ReplayPath::Log, true);
        Reshape::Tail { tiles }
    }

    /// Repaint `path` from the stroke's initial brush state with the final
    /// taper. Unclipped, every stroke tile is first put back as it was before
    /// the stroke; clipped, the caller has restored the clip's tiles.
    fn replay(&mut self, doc: &mut Document, path: ReplayPath, clipped: bool) {
        self.state = self.state0.clone();
        if !clipped && let Some(id) = self.layer
            && let Some((grid, dirty)) = doc.paint_target(id)
        {
            self.recorder.restore(grid, dirty, |_| true);
        }
        // Moved out (not copied) so painting can borrow `self`.
        let buf = std::mem::take(match path {
            ReplayPath::Log => &mut self.log,
            ReplayPath::Corrected => &mut self.corrected,
        });
        let (tin, tout, _) = self.shape;
        for_each_tapered(&buf, tin, tout, |s, p| {
            self.stroke(doc, s, p, clipped);
        });
        match path {
            ReplayPath::Log => self.log = buf,
            ReplayPath::Corrected => self.corrected = buf,
        }
        self.with_surface(doc, clipped, |brush, state, surface| brush.finish_stroke(state, surface));
    }

    /// Dab pixels a clipped replay of the log would render: the brush runs
    /// over a surface that only counts the dabs reaching the clip.
    fn tail_cost(&mut self) -> u64 {
        let mut state = self.state0.clone();
        let mut surface = CountSurface { clip: &self.clip, discard: &mut self.discard, stats: DabStats::default() };
        let (tin, tout, _) = self.shape;
        let brush = &self.brush;
        for_each_tapered(&self.log, tin, tout, |s, p| {
            brush.stroke_to(&mut state, &mut surface, s.x, s.y, p, s.tilt_x, s.tilt_y, s.dt.clamp(0.0005, 1.0));
        });
        brush.finish_stroke(&mut state, &mut surface);
        surface.stats.px
    }

    /// Run `f` with a surface over the stroke's layer, clipped to `self.clip` if asked.
    fn with_surface<R>(
        &mut self,
        doc: &mut Document,
        clipped: bool,
        f: impl FnOnce(&hokusai::Brush, &mut BrushState, &mut LayerSurface) -> R,
    ) -> Option<R> {
        let id = self.layer?;
        let (tiles_wide, tiles_high) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
        let (grid, dirty, mask) = doc.paint_target_masked(id)?;
        let mut surface = LayerSurface {
            grid,
            dirty,
            recorder: &mut self.recorder,
            discard: &mut self.discard,
            tiles_wide,
            tiles_high,
            clip: clipped.then_some(&self.clip),
            stats: &mut self.stats,
            mask,
            mask_cur: MaskCur::default(),
        };
        Some(f(&self.brush, &mut self.state, &mut surface))
    }

    /// Paint one stabilized sample live: log it, advance the arc length and
    /// apply the entry taper.
    fn paint(&mut self, doc: &mut Document, s: InputSample, dt: f64) {
        let dt = dt.clamp(0.0005, 1.0);
        let ss = ShapeSample { x: s.x, y: s.y, pressure: s.pressure, tilt_x: s.tilt_x, tilt_y: s.tilt_y, dt };
        if self.logging {
            // Capacity was reserved in `begin`: never grow here.
            if self.log.len() < self.log.capacity().min(self.log_cap) {
                self.log.push(ss);
            } else {
                self.log_overflow = true;
            }
        }
        match self.prev {
            Some(p) => self.arc += seg_len(&p, &ss),
            None => self.origin = ss,
        }
        self.extent = self.extent.max(seg_len(&self.origin, &ss));
        self.prev = Some(ss);
        let k = taper(self.arc, None, self.shape.0, self.shape.1);
        self.painted |= self.stroke(doc, &ss, ss.pressure * k, false);
    }

    /// One hokusai event at `pressure`; true if a dab changed pixels.
    fn stroke(&mut self, doc: &mut Document, s: &ShapeSample, pressure: f32, clipped: bool) -> bool {
        let dt = s.dt.clamp(0.0005, 1.0);
        let painted = self.with_surface(doc, clipped, |brush, state, surface| {
            brush.stroke_to(state, surface, s.x, s.y, pressure, s.tilt_x, s.tilt_y, dt)
        });
        painted == Some(true)
    }
}

/// Call `f(sample, pressure × final taper)` along `path`, summing arc length
/// in the same order as the live stroke so the factor before the exit taper
/// is bit-identical to the live one.
fn for_each_tapered(path: &[ShapeSample], taper_in: f32, taper_out: f32, mut f: impl FnMut(&ShapeSample, f32)) {
    let total: f32 = path.windows(2).fold(0.0, |t, w| t + seg_len(&w[0], &w[1]));
    let mut arc = 0.0f32;
    for (i, s) in path.iter().enumerate() {
        if i > 0 {
            arc += seg_len(&path[i - 1], s);
        }
        f(s, s.pressure * taper(arc, Some(total), taper_in, taper_out));
    }
}

/// Surface for [`StrokeEngine::tail_cost`]: paints nothing, counts the dabs
/// a clipped replay would render.
struct CountSurface<'a> {
    clip: &'a TileClip,
    discard: &'a mut TilePixels,
    stats: DabStats,
}

impl hokusai::TiledSurface for CountSurface<'_> {
    fn tile_request_start(&mut self, _tx: i32, _ty: i32) -> &mut hokusai::TilePixels {
        self.discard
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn draw_dab(&mut self, dab: &hokusai::Dab) -> bool {
        if self.clip.touches(dab.x, dab.y, dab.radius + 1.0) {
            self.stats.dabs += 1;
            let d = (2.0 * dab.radius + 3.0) as u64;
            self.stats.px += d * d;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use arty_core::{TileCoord, TileRef};

    use super::*;
    use crate::preset::default_presets;

    fn tiles(doc: &Document) -> Vec<(TileCoord, TileRef)> {
        let mut v: Vec<_> = doc.active_layer().raster().unwrap().iter().map(|(c, t)| (c, t.clone())).collect();
        v.sort_by_key(|(c, _)| *c);
        v
    }

    fn line(engine: &mut StrokeEngine, doc: &mut Document, n: usize) -> Option<Edit> {
        let at = |i: usize| InputSample { x: 20.0 + i as f32 * 2.0, y: 60.0, pressure: 0.8, time: i as f64 * 0.005, ..Default::default() };
        engine.begin(doc, at(0)).unwrap();
        for i in 1..n {
            engine.feed(doc, at(i));
        }
        engine.end(doc)
    }

    #[test]
    fn log_overflow_leaves_stroke_as_drawn() {
        let mut p = default_presets().into_iter().find(|p| p.name == "Inking Pen").unwrap();
        p.stabilizer = 0;
        let mut plain = p.clone();
        (plain.taper_out, plain.post_correction) = (0.0, 0);

        // 150 samples into a 64-sample log: shaping gives up, keeping the live line.
        let mut doc = Document::new(512, 128, 72);
        let mut engine = StrokeEngine::with_log_capacity(64);
        engine.configure(&p, [0.0; 3]);
        assert!(line(&mut engine, &mut doc, 150).is_some());
        assert_eq!(engine.last_reshape(), Reshape::TooLong);
        assert!(engine.log.capacity() < 150, "the log never grows past its reservation");

        // Identical to the same stroke drawn with the entry taper only.
        let mut reference = Document::new(512, 128, 72);
        let mut engine = StrokeEngine::new();
        engine.configure(&plain, [0.0; 3]);
        line(&mut engine, &mut reference, 150);
        assert_eq!(engine.last_reshape(), Reshape::Skipped);
        let (a, b) = (tiles(&doc), tiles(&reference));
        assert_eq!(a.len(), b.len());
        for ((ca, ta), (cb, tb)) in a.iter().zip(&b) {
            assert_eq!(ca, cb);
            assert!(**ta == **tb, "tile {ca:?} differs");
        }

        // The same stroke within capacity is reshaped.
        let mut doc = Document::new(512, 128, 72);
        let mut engine = StrokeEngine::with_log_capacity(256);
        engine.configure(&p, [0.0; 3]);
        line(&mut engine, &mut doc, 150);
        assert_eq!(engine.last_reshape(), Reshape::Full);
    }

    #[test]
    fn view_zoom_is_sanitized() {
        let mut e = StrokeEngine::new();
        e.set_view_zoom(2.0);
        assert_eq!(e.view_zoom, 2.0);
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            e.set_view_zoom(bad);
            assert_eq!(e.view_zoom, 1.0);
        }
    }
}
