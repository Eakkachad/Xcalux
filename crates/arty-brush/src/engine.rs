//! Stroke engine: stabilized samples → hokusai dabs → layer tiles.

use std::sync::OnceLock;
use std::time::Instant;

use arty_core::{Document, Edit, LayerId, PixelRecorder, TileGrid, TilePixels, tile::new_tile_box};
use hokusai::mapping::SettingValue;
use hokusai::{BrushSetting, BrushState};

use crate::input::{InputSample, Stabilizer};
use crate::preset::BrushPreset;
use crate::shape::{
    self, DabStats, MAX_LOGGED_SAMPLES, ShapeSample, TileClip, correct_path, correction_sigma_px, seg_len, taper,
};
use crate::speculative::{SpeculativeWorker, StrokeConfig};
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

/// Fine-grained timing breakdown of [`StrokeEngine::end`], in microseconds.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EndBreakdown {
    /// Time spent draining the stabilizer and finishing the live stroke, µs.
    pub drain_us: u64,
    /// Time spent checking and handling taps, µs.
    pub tap_us: u64,
    /// Time spent on post correction (`correct_path`), µs.
    pub correct_us: u64,
    /// Time spent building clip and bounding tail cost, µs.
    pub clip_cost_us: u64,
    /// Time spent restoring tiles (`PixelRecorder::restore`), µs.
    pub restore_us: u64,
    /// Time spent replaying dabs through the brush, µs.
    pub replay_us: u64,
    /// Time spent finishing the undo recording (`PixelRecorder::finish`), µs.
    pub finish_us: u64,
    /// Total `end()` duration, µs.
    pub total_us: u64,
}

/// Which logged path a replay paints.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayPath {
    Log,
    Corrected,
    TailCorrected,
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
    /// Pen-up time budget; `None` keeps the fixed MAX_FULL_REPLAY_PX ceiling,
    /// so library users and tests stay deterministic.
    budget_ms: Option<f64>,
    injected_rate_ns_per_px: Option<f64>,
    live_paint_ns: u64,
    calibrated_rate_ns_per_px: f64,
    tail_buf: Vec<ShapeSample>,
    tail_corrected: Vec<ShapeSample>,
    breakdown: EndBreakdown,
    speculative: bool,
    worker: SpeculativeWorker,
    pre_stroke_grid: TileGrid,
    worker_cpu_us: u64,
}

static STARTUP_RATE: OnceLock<f64> = OnceLock::new();

/// Calibrate the machine's real ns per live dab pixel (< 2 ms startup cost).
pub fn startup_rate() -> f64 {
    *STARTUP_RATE.get_or_init(calibrate_ns_per_px)
}

fn calibrate_ns_per_px() -> f64 {
    let mut doc = Document::new(128, 128, 350);
    let mut engine = StrokeEngine::new_empty(64);
    let p = BrushPreset { size: 8.0, density: 6.0, ..BrushPreset::default() };
    engine.configure(&p, [0.1, 0.1, 0.1]);
    let t0 = Instant::now();
    let _ = engine.begin(&mut doc, InputSample { x: 20.0, y: 64.0, pressure: 0.8, time: 0.0, ..Default::default() });
    for i in 1..=30 {
        engine.feed(&mut doc, InputSample { x: 20.0 + i as f32 * 2.0, y: 64.0, pressure: 0.8, time: i as f64 * 0.005, ..Default::default() });
    }
    let elapsed = t0.elapsed();
    let px = engine.dab_stats().px.max(1);
    let rate = elapsed.as_nanos() as f64 / px as f64;
    if rate.is_finite() && rate > 1.0 {
        rate.clamp(5.0, 100.0)
    } else {
        20.0
    }
}

impl Default for StrokeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StrokeEngine {
    fn new_empty(n: usize) -> Self {
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
            budget_ms: None,
            injected_rate_ns_per_px: None,
            live_paint_ns: 0,
            calibrated_rate_ns_per_px: 20.0,
            tail_buf: Vec::new(),
            tail_corrected: Vec::new(),
            breakdown: EndBreakdown::default(),
            speculative: true,
            worker: SpeculativeWorker::new(n),
            pre_stroke_grid: TileGrid::new(),
            worker_cpu_us: 0,
        }
    }

    pub fn new() -> Self {
        Self::with_log_capacity(MAX_LOGGED_SAMPLES)
    }

    /// An engine whose strokes log at most `n` samples; longer shaped
    /// strokes are kept as drawn ([`Reshape::TooLong`]).
    pub(crate) fn with_log_capacity(n: usize) -> Self {
        Self::new_empty(n)
    }

    /// Override the replay rate (ns/px) for deterministic tests.
    pub fn set_replay_rate(&mut self, rate_ns_per_px: f64) {
        self.injected_rate_ns_per_px = Some(rate_ns_per_px.max(0.1));
    }

    /// Bound pen-up replay by time instead of the fixed pixel ceiling: strokes
    /// that would take longer than `budget_ms` on this machine get a tail
    /// replay. The rate comes from a startup calibration, then from live
    /// strokes, so the choice depends on the machine (B022).
    pub fn set_replay_budget(&mut self, budget_ms: f64) {
        self.budget_ms = Some(budget_ms.max(0.1));
        self.calibrated_rate_ns_per_px = startup_rate();
    }

    /// Effective replay rate in ns per live dab pixel.
    pub fn replay_rate(&self) -> f64 {
        self.effective_rate_ns_per_px()
    }

    /// Pen-up replay budget in milliseconds, if one is set.
    pub fn replay_budget(&self) -> Option<f64> {
        self.budget_ms
    }

    /// The derived full-replay ceiling in live dab pixels.
    pub fn full_replay_ceiling(&self) -> u64 {
        self.full_replay_ceiling_px()
    }

    /// Fine-grained timing breakdown of the last `end()` call.
    pub fn last_breakdown(&self) -> EndBreakdown {
        self.breakdown
    }

    /// Enable or disable speculative stroke replay (Zero-Wait Pen-Up).
    pub fn set_speculative_replay(&mut self, enabled: bool) {
        self.speculative = enabled;
    }

    /// Whether speculative stroke replay is enabled.
    pub fn is_speculative_replay(&self) -> bool {
        self.speculative
    }

    /// Worker CPU time consumed by the last stroke replay (in microseconds).
    pub fn worker_cpu_time_us(&self) -> u64 {
        self.worker_cpu_us
    }

    fn effective_rate_ns_per_px(&self) -> f64 {
        if let Some(r) = self.injected_rate_ns_per_px {
            return r;
        }
        if self.stats.px >= 200_000 && self.live_paint_ns > 0 {
            let live_rate = self.live_paint_ns as f64 / self.stats.px as f64;
            if live_rate.is_finite() && live_rate > 1.0 {
                return live_rate.clamp(5.0, 100.0);
            }
        }
        self.calibrated_rate_ns_per_px
    }

    fn full_replay_ceiling_px(&self) -> u64 {
        let Some(budget_ms) = self.budget_ms else { return shape::MAX_FULL_REPLAY_PX };
        let rate = self.effective_rate_ns_per_px();
        if rate <= 0.0 || !rate.is_finite() {
            return shape::MAX_FULL_REPLAY_PX;
        }
        let ns = budget_ms * 1_000_000.0;
        (ns / rate) as u64
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
        self.live_paint_ns = 0;
        self.worker_cpu_us = 0;
        let (tiles_wide, tiles_high) = (doc.tiles_wide() as i32, doc.tiles_high() as i32);
        if let Some((grid, _, mask)) = doc.paint_target_masked(id) {
            self.pre_stroke_grid = grid.clone();
            if self.logging && self.speculative {
                self.worker.start_stroke(StrokeConfig {
                    brush: self.brush.clone(),
                    state0: self.state0.clone(),
                    shape: self.shape,
                    view_zoom: self.view_zoom,
                    pre_stroke_grid: self.pre_stroke_grid.clone(),
                    tiles_wide,
                    tiles_high,
                    mask: mask.cloned(),
                });
            }
        }
        let s = self.stabilizer.push(s);
        let t0 = Instant::now();
        self.paint(doc, s, 0.0);
        self.live_paint_ns += t0.elapsed().as_nanos() as u64;
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
        let t0 = Instant::now();
        self.paint(doc, smoothed, dt);
        self.live_paint_ns += t0.elapsed().as_nanos() as u64;
    }

    /// Finish the stroke; returns the undo entry if anything was painted.
    ///
    /// With exit taper or post correction on, the finished stroke is
    /// repainted from its log here ([`StrokeEngine::last_reshape`] says how).
    pub fn end(&mut self, doc: &mut Document) -> Option<Edit> {
        self.layer?;
        let t_end_start = Instant::now();
        self.breakdown = EndBreakdown::default();

        // Let a stabilized line catch up to the pen-up point.
        let t_drain = Instant::now();
        while let Some(s) = self.stabilizer.drain_step() {
            self.paint(doc, s, 0.004);
        }
        self.painted |= self.with_surface(doc, false, |brush, state, surface| brush.finish_stroke(state, surface))
            == Some(true);
        self.breakdown.drain_us = t_drain.elapsed().as_micros() as u64;

        let t_tap = Instant::now();
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
                let t_r0 = Instant::now();
                self.recorder.restore(grid, dirty, |_| true);
                self.breakdown.restore_us += t_r0.elapsed().as_micros() as u64;
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
            self.worker.cancel();
            self.with_surface(doc, false, |brush, state, surface| {
                state.dist_past_dab = 1.0;
                state.last_pressure = p;
                let (x, y) = (state.last_event_x, state.last_event_y);
                brush.stroke_to(state, surface, x, y, p, 0.0, 0.0, 0.004);
            });
        }
        self.breakdown.tap_us = t_tap.elapsed().as_micros() as u64;

        self.reshape = self.reshape_stroke(doc, was_tap);
        self.layer = None;

        let t_fin = Instant::now();
        let edit = self.recorder.finish();
        self.breakdown.finish_us = t_fin.elapsed().as_micros() as u64;
        self.breakdown.total_us = t_end_start.elapsed().as_micros() as u64;
        edit
    }

    /// Abort the stroke, restoring the layer as it was.
    pub fn cancel(&mut self, doc: &mut Document) {
        self.worker.cancel();
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
            self.worker.cancel();
            return Reshape::Skipped;
        }
        if self.log_overflow {
            self.worker.cancel();
            return Reshape::TooLong;
        }

        let sigma = if corr > 0 { correction_sigma_px(corr, self.view_zoom) } else { 0.0 };
        let mut shift = 0.0f32;
        if corr > 0 {
            let t_c0 = Instant::now();
            shift = correct_path(&self.log, sigma, &mut self.corrected, &mut self.scratch);
            self.breakdown.correct_us += t_c0.elapsed().as_micros() as u64;
            if shift < 0.05 && tout == 0.0 {
                self.worker.cancel();
                return Reshape::Skipped;
            }
        }

        let ceiling = self.full_replay_ceiling_px();
        let affordable = self.stats.px <= ceiling;

        // The worker has replayed the stable prefix while the pen was down, so
        // only the remainder is left and the pixel ceiling does not apply (B023),
        // except for huge brushes, whose remaining dabs alone could stall pen-up.
        if self.speculative && (affordable || self.max_radius <= 64.0) {
            let t_rep0 = Instant::now();
            if let Some(res) = self.worker.finish_stroke(self.log.clone(), total, 100)
                && let Some(id) = self.layer
                && let Some((grid, dirty)) = doc.paint_target(id)
            {
                let t_r0 = Instant::now();
                self.recorder.restore(grid, dirty, |c| res.shadow_grid.get_ref(c).is_none());
                for (c, new_tile) in res.shadow_grid.iter() {
                    self.recorder.before_write(grid, c);
                    grid.replace(c, Some(new_tile.clone()));
                    dirty.mark(c);
                }
                self.breakdown.restore_us += t_r0.elapsed().as_micros() as u64;
                self.breakdown.replay_us += t_rep0.elapsed().as_micros() as u64;
                self.stats = res.stats;
                self.worker_cpu_us = res.cpu_time_us;
                return Reshape::Full;
            }
            self.worker.cancel();
        }

        if corr > 0 {
            if affordable {
                self.replay(doc, ReplayPath::Corrected, false);
                return Reshape::Full;
            }
            if tout == 0.0 {
                return Reshape::TooLong;
            }
            // Too costly to repaint whole: still give the raw line its exit taper and end correction.
        }
        if self.blending {
            // Smudge reads the canvas, so a clipped replay would differ.
            if !affordable {
                return Reshape::TooLong;
            }
            self.replay(doc, ReplayPath::Log, false);
            return Reshape::Full;
        }
        // Tail: only samples within the tail reach change.
        // For taper only: samples within `tout`.
        // If correction is active (`corr > 0` and `shift >= 0.05`),
        // the tail reach must also cover the smoothing window (`3.0 * sigma`).
        let tail_reach = if corr > 0 && shift >= 0.05 {
            tout.max(3.0 * sigma)
        } else {
            tout
        };
        let mut s = 0.0f32;
        let mut i0 = self.log.len() - 1;
        for i in 0..self.log.len() {
            if i > 0 {
                s += seg_len(&self.log[i - 1], &self.log[i]);
            }
            if total - s < tail_reach {
                i0 = i;
                break;
            }
        }
        let use_tail_correction = corr > 0 && shift >= 0.05 && self.log.len() - i0 >= 3;
        if use_tail_correction {
            let t_c0 = Instant::now();
            correct_path(&self.log[i0..], sigma, &mut self.tail_corrected, &mut self.scratch);
            self.breakdown.correct_us += t_c0.elapsed().as_micros() as u64;

            self.tail_buf.clear();
            self.tail_buf.extend_from_slice(&self.log[..i0]);
            self.tail_buf.extend_from_slice(&self.tail_corrected);

            let t_cl0 = Instant::now();
            self.clip.build(&self.tail_buf[i0.saturating_sub(1)..], self.max_radius);
            self.breakdown.clip_cost_us += t_cl0.elapsed().as_micros() as u64;
        } else {
            let t_cl0 = Instant::now();
            self.clip.build(&self.log[i0.saturating_sub(1)..], self.max_radius);
            self.breakdown.clip_cost_us += t_cl0.elapsed().as_micros() as u64;
        }
        if self.clip.is_empty() {
            return Reshape::TooLong;
        }
        // A clipped replay still renders every dab that reaches the clip in
        // full. That is bounded by the live cost; when even that is over
        // budget (huge brushes), count the clipped cost first without painting.
        if !affordable && self.max_radius > 64.0 {
            let t_cc0 = Instant::now();
            let tc = self.tail_cost();
            self.breakdown.clip_cost_us += t_cc0.elapsed().as_micros() as u64;
            if tc > ceiling {
                return Reshape::TooLong;
            }
        }
        let mut tiles = 0u32;
        if let Some(id) = self.layer
            && let Some((grid, dirty)) = doc.paint_target(id)
        {
            let t_r0 = Instant::now();
            let clip = &self.clip;
            self.recorder.restore(grid, dirty, |c| {
                let hit = clip.contains(c);
                tiles += hit as u32;
                hit
            });
            self.breakdown.restore_us += t_r0.elapsed().as_micros() as u64;
        }
        if use_tail_correction {
            self.replay(doc, ReplayPath::TailCorrected, true);
        } else {
            self.replay(doc, ReplayPath::Log, true);
        }
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
            let t_r0 = Instant::now();
            self.recorder.restore(grid, dirty, |_| true);
            self.breakdown.restore_us += t_r0.elapsed().as_micros() as u64;
        }
        // Moved out (not copied) so painting can borrow `self`.
        let buf = std::mem::take(match path {
            ReplayPath::Log => &mut self.log,
            ReplayPath::Corrected => &mut self.corrected,
            ReplayPath::TailCorrected => &mut self.tail_buf,
        });
        let t_rep0 = Instant::now();
        let (tin, tout, _) = self.shape;
        for_each_tapered(&buf, tin, tout, |s, p| {
            self.stroke(doc, s, p, clipped);
        });
        self.with_surface(doc, clipped, |brush, state, surface| brush.finish_stroke(state, surface));
        self.breakdown.replay_us += t_rep0.elapsed().as_micros() as u64;
        match path {
            ReplayPath::Log => self.log = buf,
            ReplayPath::Corrected => self.corrected = buf,
            ReplayPath::TailCorrected => self.tail_buf = buf,
        }
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
                if self.speculative && !self.log_overflow {
                    self.worker.push_sample(ss);
                }
            } else {
                self.log_overflow = true;
                if self.speculative {
                    self.worker.cancel();
                }
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

    /// Fed in small chunks, each processed by the worker before the next one
    /// arrives (as at a real pen rate), the stroke is rendered while the pen is
    /// down and still equals the synchronous replay bit for bit.
    #[test]
    fn zero_wait_incremental_matches_sync_replay() {
        let preset = |name: &str| default_presets().into_iter().find(|p| p.name == name).unwrap();
        let g_pen = BrushPreset { stabilizer: 0, ..preset("G-Pen") };
        let cases = [
            (BrushPreset { taper_out: 80.0, post_correction: 3, ..g_pen.clone() }, 1),
            (BrushPreset { taper_out: 60.0, post_correction: 0, ..g_pen.clone() }, 2),
            (BrushPreset { taper_out: 0.0, post_correction: 6, ..g_pen.clone() }, 5),
            (BrushPreset { size: 24.0, taper_in: 0.0, taper_out: 30.0, post_correction: 10, ..g_pen }, 3),
            (preset("Inking Pen"), 4),
        ];
        // Wobble, a zigzag (its smoothed path is far shorter than the raw one) and a pause.
        let mut pts = Vec::new();
        for i in 0..420 {
            let t = i as f32;
            let (x, y) = match i {
                0..150 => (40.0 + 2.0 * t, 120.0 + 12.0 * (t / 9.0).sin()),
                150..170 => (340.0, 120.0),
                170..330 => (340.0 - 0.5 * (t - 170.0), 200.0 + if i % 2 == 0 { 4.0 } else { -4.0 }),
                _ => (260.0 - 2.5 * (t - 330.0), 200.0 + 1.5 * (t - 330.0)),
            };
            let pressure = 0.3 + 0.6 * (t / 23.0).sin().abs();
            pts.push(InputSample { x, y, pressure, time: i as f64 / 240.0, ..Default::default() });
        }
        for (p, chunk) in cases {
            let mut doc_sync = Document::new(512, 512, 350);
            let mut doc_spec = Document::new(512, 512, 350);
            let mut e_sync = StrokeEngine::new();
            e_sync.set_speculative_replay(false);
            e_sync.configure(&p, [0.2, 0.5, 0.8]);
            let mut e_spec = StrokeEngine::new();
            e_spec.configure(&p, [0.2, 0.5, 0.8]);
            e_sync.begin(&mut doc_sync, pts[0]).unwrap();
            e_spec.begin(&mut doc_spec, pts[0]).unwrap();
            let mut rendered = 0;
            for c in pts[1..].chunks(chunk) {
                for &s in c {
                    e_sync.feed(&mut doc_sync, s);
                    e_spec.feed(&mut doc_spec, s);
                }
                rendered = e_spec.worker.wait_processed(e_spec.log.len());
            }
            // Most of the stroke was rendered before pen-up (steps: stations or samples).
            let (_, tout, corr) = e_spec.shape;
            let steps = if corr > 0 { e_spec.arc / shape::station_step(correction_sigma_px(corr, 1.0)) } else { 400.0 };
            assert!(rendered as f32 > 0.6 * steps, "{}: only {rendered} of ~{steps} steps rendered live", p.name);
            e_sync.end(&mut doc_sync);
            e_spec.end(&mut doc_spec);
            // Taper only replays the tail synchronously, which is exact too.
            assert!(matches!(e_sync.last_reshape(), Reshape::Full | Reshape::Tail { .. }));
            assert_eq!(e_spec.last_reshape(), Reshape::Full);
            assert!(e_spec.worker_cpu_time_us() > 0, "{}: the worker result was not used", p.name);
            let (a, b) = (tiles(&doc_spec), tiles(&doc_sync));
            assert_eq!(a.len(), b.len(), "{} (taper {tout}, correction {corr})", p.name);
            for ((ca, ta), (cb, tb)) in a.iter().zip(&b) {
                assert_eq!(ca, cb);
                assert!(**ta == **tb, "{} (taper {tout}, correction {corr}, chunk {chunk}): tile {ca:?} differs", p.name);
            }
        }
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
