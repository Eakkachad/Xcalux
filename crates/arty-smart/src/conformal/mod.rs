//! Ported from katgpt-rs `crates/katgpt-core/src/conformal/mod.rs` (MIT, see
//! `NOTICE`). Changes: dropped the KARC adapter, the floor-comparison
//! harness, the staleness guard and `sample_predictive_distribution` (it
//! needed `fastrand` and allocated); `fast_exp` (SIMD polynomial) replaced by
//! `f32::exp`; added `clear`; feature gates removed.
//!
//! # Conformal predictive intervals
//!
//! A modelless UQ overlay that wraps any point forecaster and turns its
//! residual history into calibrated predictive intervals (after the
//! Conformal Splice Predictive forecaster, arXiv:2605.03789).
//!
//! 1. Wraps a [`PointForecaster`] (ARTY's F2 predictor implements it).
//! 2. Keeps a per-channel residual pool with exponential recency weighting.
//! 3. Indexes the pool by horizon `h` via `L_h = m·⌈h/m⌉` ([`ResidualMode::HStep`])
//!    or a single lag-`m` pool ([`ResidualMode::Paper`]).
//! 4. Reads weighted empirical quantiles `q_{α/2}`, `q_{1−α/2}` to produce
//!    `[point + q_{α/2}, point + q_{1−α/2}]`.
//! 5. [`metrics`] scores intervals (CRPS, Winkler, coverage) for the F2 gate.
//!
//! In ARTY (plan F2 step 2) the half-width of this interval, in physical
//! pixels at the current zoom, decides whether predicted ink is shown at
//! all: the residual pool lives in session memory only and is never saved.
//!
//! ## "Report the floor"
//!
//! `ConformalIntervalCalibrator<SeasonalNaiveForecaster>` with `m = 1` is the
//! conformal-naive floor: a predictor claiming calibrated uncertainty must
//! beat it on CRPS / coverage / Winkler.
//!
//! ## Allocation
//!
//! `new` allocates the pool once. `update_residual`, `interval_into`,
//! `interval_from_point_into` and `coverage_violation` never touch the heap
//! (the sorted rings are pre-reserved; quantile weights live in a stack
//! buffer).

pub mod metrics;
mod ring;
mod seasonal;

pub use metrics::{
    crps, crps_interval, empirical_coverage, mean_crps_interval, mean_winkler, winkler_score,
};
pub use ring::{ResidualRingBuffer, RingBuffer, RingView};
pub use seasonal::{SeasonalNaiveForecaster, SeasonalPoolForecaster, seasonal_naive_floor};

/// Pools up to this size get the single-pass stack-buffer weights path;
/// larger pools fall back to the per-quantile recomputation. 1024 f32 = 4 KB
/// of stack. ARTY's F2 pool is far smaller.
const WEIGHTS_BUF_LEN: usize = 1024;

/// A point forecaster that produces a single deterministic forecast.
///
/// The `delay_state` slice is forecaster-specific (ignored by the seasonal
/// pool). Implementations must be deterministic in `(self, delay_state, h)`.
/// `&mut self` so a forecaster can reuse internal scratch.
pub trait PointForecaster {
    /// Forecast the value at horizon `h` (1-indexed) given the state. Writes
    /// into `out` (zero-alloc).
    fn forecast_into(&mut self, delay_state: &[f32], h: usize, out: &mut f32);
}

/// Residual pool indexing strategy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ResidualMode {
    /// Single residual pool (lag `m`) reused for all horizons; constant
    /// interval width across horizons.
    Paper,
    /// Horizon-indexed pool with `L_h = m·⌈h/m⌉`; the interval widens with
    /// horizon. Use for non-seasonal (`m=1`) series — pen input.
    #[default]
    HStep,
}

/// Unit for the residual pool's exponential recency decay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum DecayUnit {
    /// Decay by absolute observation age (time steps).
    #[default]
    Step,
    /// Decay by cycle age (`m`× weaker than `Step` for the same λ).
    Cycle,
}

/// The lag `L_h` used to bucket the residual at horizon `h`.
#[inline]
#[cfg_attr(not(test), allow(dead_code))] // part of the documented math surface
fn horizon_lag(h: usize, m: usize, mode: ResidualMode) -> usize {
    debug_assert!(m >= 1, "seasonal period m must be >= 1");
    debug_assert!(h >= 1, "horizon h is 1-indexed");
    match mode {
        ResidualMode::Paper => m,
        ResidualMode::HStep => m * h.div_ceil(m),
    }
}

/// The conformal UQ overlay. Generic over any [`PointForecaster`].
pub struct ConformalIntervalCalibrator<F: PointForecaster> {
    /// The wrapped point forecaster.
    pub forecaster: F,
    /// Per-channel × per-horizon-bucket sorted residual rings.
    pub residual_pool: ResidualRingBuffer,
    /// Seasonal period. `m=1` for non-seasonal data.
    pub m: usize,
    /// Exponential recency-decay rate. Weight `w = exp(−λ · age)`.
    pub exp_lambda: f32,
    /// Unit for `age` in the weight formula.
    pub decay_unit: DecayUnit,
    /// Residual-pool indexing strategy.
    pub residual_mode: ResidualMode,
    /// Quantile-orientation correction (average on exact CDF ties).
    pub orientation: bool,
    /// Monotonic tick counter (age = `global_tick − push_tick`).
    global_tick: u64,
}

/// A calibrated predictive interval `[lower, point, upper]` at level `1−α`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PredictiveInterval {
    /// Lower bound `point + q_{α/2}`.
    pub lower: f32,
    /// Point forecast `ŷ`.
    pub point: f32,
    /// Upper bound `point + q_{1−α/2}`.
    pub upper: f32,
    /// Two-tailed miscoverage level (e.g. `0.05` for a 95% interval).
    pub alpha: f32,
}

impl PredictiveInterval {
    /// Construct a new interval. Does NOT validate `lower ≤ point ≤ upper`.
    #[inline]
    pub const fn new(lower: f32, point: f32, upper: f32, alpha: f32) -> Self {
        Self {
            lower,
            point,
            upper,
            alpha,
        }
    }

    /// `true` iff `actual` is within `[lower, upper]` (inclusive).
    #[inline]
    pub fn contains(&self, actual: f32) -> bool {
        actual >= self.lower && actual <= self.upper
    }

    /// Interval half-width `(upper − lower) / 2`.
    #[inline]
    pub fn half_width(&self) -> f32 {
        0.5 * (self.upper - self.lower)
    }
}

impl<F: PointForecaster> ConformalIntervalCalibrator<F> {
    /// Construct a new calibrator (allocates the residual pool once).
    ///
    /// - `n_channels`: number of channels (ARTY F2: x, y or the tip distance).
    /// - `max_h`: maximum horizon that will be queried; larger horizons clamp
    ///   into the last bucket.
    /// - `m`: seasonal period (`m=1` for non-seasonal).
    /// - `capacity`: ring capacity per (channel, horizon-bucket). Memory is
    ///   `n_channels · ceil(max_h/m) · capacity · 12` bytes.
    /// - `exp_lambda`: recency decay rate. `0.0` disables recency weighting.
    pub fn new(
        forecaster: F,
        n_channels: usize,
        max_h: usize,
        m: usize,
        capacity: usize,
        exp_lambda: f32,
        decay_unit: DecayUnit,
        residual_mode: ResidualMode,
        orientation: bool,
    ) -> Self {
        assert!(m >= 1, "seasonal period m must be >= 1");
        assert!(n_channels >= 1, "n_channels must be >= 1");
        assert!(max_h >= 1, "max_h must be >= 1");
        assert!(capacity >= 1, "capacity must be >= 1");
        let n_buckets = Self::n_buckets_for(max_h, m, residual_mode);
        Self {
            forecaster,
            residual_pool: ResidualRingBuffer::new(n_channels, n_buckets, capacity),
            m,
            exp_lambda,
            decay_unit,
            residual_mode,
            orientation,
            global_tick: 0,
        }
    }

    /// Number of horizon buckets needed to cover `max_h`.
    #[inline]
    fn n_buckets_for(max_h: usize, m: usize, mode: ResidualMode) -> usize {
        match mode {
            ResidualMode::Paper => 1,
            ResidualMode::HStep => max_h.div_ceil(m),
        }
    }

    /// Map `h` to a flat horizon-bucket index.
    #[inline]
    fn bucket_index(&self, h: usize) -> usize {
        debug_assert!(h >= 1, "horizon h is 1-indexed");
        let n_buckets = self.residual_pool.n_buckets;
        if n_buckets == 0 {
            return 0;
        }
        let raw = match self.residual_mode {
            ResidualMode::Paper => 0,
            ResidualMode::HStep => h.div_ceil(self.m) - 1,
        };
        raw.min(n_buckets.saturating_sub(1))
    }

    /// Advance the monotonic tick counter by one step. Call once per
    /// observation step so that recency weights reflect elapsed time.
    #[inline]
    pub fn step(&mut self) {
        self.global_tick = self.global_tick.saturating_add(1);
    }

    /// Current monotonic tick.
    #[inline]
    pub fn tick(&self) -> u64 {
        self.global_tick
    }

    /// Drop every stored residual and restart the tick (keeps the pool's
    /// allocation). ARTY addition, for "reset" in a future Hand Profile.
    pub fn clear(&mut self) {
        self.residual_pool.clear();
        self.global_tick = 0;
    }

    /// Record an `(actual, forecast)` pair at horizon `h` for `channel`:
    /// pushes `r = actual − forecast` into the horizon bucket, tagged with the
    /// current tick. Recency weights are applied at read time.
    pub fn update_residual(&mut self, actual: f32, forecast: f32, channel: usize, h: usize) {
        let residual = actual - forecast;
        let bucket = self.bucket_index(h);
        self.residual_pool
            .push(residual, channel, bucket, self.global_tick);
    }

    /// Forecast via the wrapped forecaster, then record the realized
    /// `actual`.
    pub fn observe_and_update(
        &mut self,
        actual: f32,
        delay_state: &[f32],
        channel: usize,
        h: usize,
    ) {
        let mut forecast = 0.0_f32;
        self.forecaster.forecast_into(delay_state, h, &mut forecast);
        self.update_residual(actual, forecast, channel, h);
    }

    /// The calibrated interval at horizon `h`, level `1−α`, for `channel`,
    /// with the point taken from the wrapped forecaster (empty delay state).
    pub fn interval_into(
        &mut self,
        channel: usize,
        h: usize,
        alpha: f32,
        out: &mut PredictiveInterval,
    ) {
        let mut point = 0.0_f32;
        self.forecaster.forecast_into(&[], h, &mut point);
        self.interval_from_point_into(point, channel, h, alpha, out);
    }

    /// As [`interval_into`](Self::interval_into) but with a caller-supplied
    /// point forecast (the usual path: the predictor already produced ŷ).
    pub fn interval_from_point_into(
        &self,
        point: f32,
        channel: usize,
        h: usize,
        alpha: f32,
        out: &mut PredictiveInterval,
    ) {
        debug_assert!(
            (0.0..=0.5).contains(&alpha),
            "alpha must be in [0, 0.5] for a two-tailed interval"
        );
        let bucket = self.bucket_index(h);
        let (q_lo, q_hi) =
            self.weighted_quantile_pair(channel, bucket, 0.5 * alpha, 1.0 - 0.5 * alpha);
        out.lower = point + q_lo;
        out.point = point;
        out.upper = point + q_hi;
        out.alpha = alpha;
    }

    /// `true` iff `actual` is outside the `1−α` interval at horizon `h`.
    #[inline]
    pub fn coverage_violation(
        &mut self,
        actual: f32,
        channel: usize,
        h: usize,
        alpha: f32,
    ) -> bool {
        let mut interval = PredictiveInterval::new(0.0, 0.0, 0.0, alpha);
        self.interval_into(channel, h, alpha, &mut interval);
        !interval.contains(actual)
    }

    /// Recency weight of an entry pushed at `pushed_tick`.
    #[inline]
    fn weight(&self, pushed_tick: u64) -> f32 {
        let unit_scale = match self.decay_unit {
            DecayUnit::Step => 1.0,
            DecayUnit::Cycle => self.m as f32,
        };
        let age = (self.global_tick.saturating_sub(pushed_tick)) as f32 / unit_scale;
        (-self.exp_lambda * age).exp()
    }

    /// Two weighted quantiles from the same `(channel, bucket)` in a single
    /// weights pass (weights computed once, reused for both lookups).
    fn weighted_quantile_pair(
        &self,
        channel: usize,
        bucket: usize,
        p_lo: f32,
        p_hi: f32,
    ) -> (f32, f32) {
        let view = self.residual_pool.channel_bucket(channel, bucket);
        let n = view.len();
        if n == 0 {
            return (0.0, 0.0);
        }
        if n <= WEIGHTS_BUF_LEN {
            let mut weights = [0.0_f32; WEIGHTS_BUF_LEN];
            let mut total_w = 0.0_f32;
            for (i, w_slot) in weights.iter_mut().enumerate().take(n) {
                let w = self.weight(view.get_sorted(i).1);
                *w_slot = w;
                total_w += w;
            }
            if total_w <= 0.0 {
                // Degenerate decay → median fallback.
                let med = view.get_sorted(n / 2).0;
                return (med, med);
            }
            let q_lo =
                Self::quantile_from_weights(&view, &weights[..n], total_w, p_lo, self.orientation);
            let q_hi =
                Self::quantile_from_weights(&view, &weights[..n], total_w, p_hi, self.orientation);
            return (q_lo, q_hi);
        }
        (
            self.weighted_quantile(channel, bucket, p_lo),
            self.weighted_quantile(channel, bucket, p_hi),
        )
    }

    /// Walk the sorted residuals using precomputed `weights`; returns the
    /// residual at weighted-CDF probability `p`.
    fn quantile_from_weights(
        view: &RingView<'_>,
        weights: &[f32],
        total_w: f32,
        p: f32,
        orientation: bool,
    ) -> f32 {
        let n = view.len();
        debug_assert_eq!(n, weights.len());
        let target = p * total_w;
        let mut acc = 0.0_f32;
        let mut prev_val = view.get_sorted(0).0;
        for (i, &w) in weights.iter().enumerate() {
            let (val, _) = view.get_sorted(i);
            prev_val = val;
            acc += w;
            if acc >= target {
                if orientation && i + 1 < n && acc == target {
                    let next_val = view.get_sorted(i + 1).0;
                    return 0.5 * (val + next_val);
                }
                return val;
            }
        }
        prev_val
    }

    /// Weighted empirical quantile at `p ∈ [0,1]` for `(channel, bucket)`,
    /// recomputing weights per entry (no buffer; any pool size). Returns
    /// `0.0` for an empty pool (the interval collapses to the point).
    fn weighted_quantile(&self, channel: usize, bucket: usize, p: f32) -> f32 {
        let view = self.residual_pool.channel_bucket(channel, bucket);
        let n = view.len();
        if n == 0 {
            return 0.0;
        }
        let mut total_w = 0.0_f32;
        for i in 0..n {
            total_w += self.weight(view.get_sorted(i).1);
        }
        if total_w <= 0.0 {
            return view.get_sorted(n / 2).0;
        }
        let target = p * total_w;
        let mut acc = 0.0_f32;
        let mut prev_val = view.get_sorted(0).0;
        for i in 0..n {
            let (val, pushed_tick) = view.get_sorted(i);
            prev_val = val;
            acc += self.weight(pushed_tick);
            if acc >= target {
                if self.orientation && i + 1 < n && acc == target {
                    let next_val = view.get_sorted(i + 1).0;
                    return 0.5 * (val + next_val);
                }
                return val;
            }
        }
        prev_val
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trivial forecaster that always predicts a constant.
    struct ConstForecaster {
        value: f32,
    }
    impl PointForecaster for ConstForecaster {
        fn forecast_into(&mut self, _delay_state: &[f32], _h: usize, out: &mut f32) {
            *out = self.value;
        }
    }

    fn cal(
        value: f32,
        max_h: usize,
        m: usize,
        cap: usize,
        lambda: f32,
        orientation: bool,
    ) -> ConformalIntervalCalibrator<ConstForecaster> {
        ConformalIntervalCalibrator::new(
            ConstForecaster { value },
            1,
            max_h,
            m,
            cap,
            lambda,
            DecayUnit::Step,
            ResidualMode::HStep,
            orientation,
        )
    }

    #[test]
    fn horizon_lag_paper_vs_hstep() {
        assert_eq!(horizon_lag(1, 12, ResidualMode::Paper), 12);
        assert_eq!(horizon_lag(24, 12, ResidualMode::Paper), 12);
        assert_eq!(horizon_lag(1, 12, ResidualMode::HStep), 12);
        assert_eq!(horizon_lag(12, 12, ResidualMode::HStep), 12);
        assert_eq!(horizon_lag(13, 12, ResidualMode::HStep), 24);
        assert_eq!(horizon_lag(24, 12, ResidualMode::HStep), 24);
        assert_eq!(horizon_lag(25, 12, ResidualMode::HStep), 36);
        assert_eq!(horizon_lag(1, 1, ResidualMode::HStep), 1);
        assert_eq!(horizon_lag(7, 1, ResidualMode::HStep), 7);
    }

    #[test]
    fn interval_empty_pool_collapses_to_point() {
        let mut cal = cal(42.0, 8, 1, 16, 0.0, false);
        let mut iv = PredictiveInterval::new(0.0, 0.0, 0.0, 0.05);
        cal.interval_into(0, 1, 0.05, &mut iv);
        assert_eq!((iv.lower, iv.point, iv.upper), (42.0, 42.0, 42.0));
    }

    #[test]
    fn update_then_interval_contains_point_plus_residual_quantile() {
        let mut cal = cal(10.0, 1, 1, 64, 0.0, false);
        for &r in &[-2.0_f32, -1.0, 1.0, 2.0] {
            cal.update_residual(10.0 + r, 10.0, 0, 1);
        }
        let mut iv = PredictiveInterval::new(0.0, 0.0, 0.0, 0.5);
        cal.interval_into(0, 1, 0.5, &mut iv);
        // Sorted residuals [−2, −1, 1, 2], equal weights: q_{0.25} = −2,
        // q_{0.75} = 1.
        assert!((iv.point - 10.0).abs() < 1e-6, "point {}", iv.point);
        assert!((iv.lower - 8.0).abs() < 1e-6, "lower {}", iv.lower);
        assert!((iv.upper - 11.0).abs() < 1e-6, "upper {}", iv.upper);
        assert!((iv.half_width() - 1.5).abs() < 1e-6);
    }

    #[test]
    fn coverage_violation_flag() {
        let mut cal = cal(0.0, 1, 1, 64, 0.0, false);
        for &r in &[-1.0_f32, 1.0] {
            cal.update_residual(r, 0.0, 0, 1);
        }
        assert!(
            !cal.coverage_violation(0.0, 0, 1, 0.5),
            "0 should be inside"
        );
        assert!(
            cal.coverage_violation(5.0, 0, 1, 0.5),
            "5 should be outside"
        );
    }

    #[test]
    fn orientation_correction_averages_on_tie() {
        let mut cal = cal(0.0, 1, 1, 64, 0.0, true);
        for &r in &[0.0_f32, 10.0] {
            cal.update_residual(r, 0.0, 0, 1);
        }
        let q = cal.weighted_quantile(0, 0, 0.5);
        assert!((q - 5.0).abs() < 1e-6, "orientation tie-break q={q}");
        // The buffered pair path agrees with the per-quantile path.
        assert_eq!(cal.weighted_quantile_pair(0, 0, 0.5, 0.5), (q, q));
    }

    #[test]
    fn recency_decay_favours_recent_residuals() {
        // Old residuals are large, recent ones small: with strong decay the
        // interval shrinks toward the recent spread.
        let mut flat = cal(0.0, 1, 1, 64, 0.0, false);
        let mut decayed = cal(0.0, 1, 1, 64, 0.5, false);
        for i in 0..40 {
            let r = if i < 20 { 10.0 } else { 0.5 } * if i % 2 == 0 { 1.0 } else { -1.0 };
            for c in [&mut flat, &mut decayed] {
                c.update_residual(r, 0.0, 0, 1);
                c.step();
            }
        }
        let mut a = PredictiveInterval::new(0.0, 0.0, 0.0, 0.1);
        let mut b = a;
        flat.interval_from_point_into(0.0, 0, 1, 0.1, &mut a);
        decayed.interval_from_point_into(0.0, 0, 1, 0.1, &mut b);
        assert!(b.half_width() < a.half_width(), "{b:?} vs {a:?}");
    }

    #[test]
    fn bit_reproducibility_identical_configs() {
        let mk = || {
            let mut cal = cal(1.0, 1, 1, 32, 0.01, false);
            for i in 0..16 {
                let r = (i as f32) * 0.5 - 4.0;
                cal.update_residual(r, 1.0, 0, 1);
                cal.step();
            }
            cal
        };
        let mut a = mk();
        let mut b = mk();
        let mut iva = PredictiveInterval::new(0.0, 0.0, 0.0, 0.05);
        let mut ivb = PredictiveInterval::new(0.0, 0.0, 0.0, 0.05);
        for &alpha in &[0.01_f32, 0.05, 0.1, 0.2] {
            a.interval_into(0, 1, alpha, &mut iva);
            b.interval_into(0, 1, alpha, &mut ivb);
            assert_eq!(iva.lower.to_bits(), ivb.lower.to_bits(), "alpha={alpha}");
            assert_eq!(iva.upper.to_bits(), ivb.upper.to_bits(), "alpha={alpha}");
        }
    }

    #[test]
    fn bucket_index_clamps_beyond_max_h() {
        // max_h=4, m=2 → 2 buckets; h beyond max_h clamps to the last.
        let cal = cal(0.0, 4, 2, 8, 0.0, false);
        assert_eq!(cal.bucket_index(1), 0);
        assert_eq!(cal.bucket_index(2), 0);
        assert_eq!(cal.bucket_index(3), 1);
        assert_eq!(cal.bucket_index(4), 1);
        assert_eq!(cal.bucket_index(5), 1);
    }

    #[test]
    fn clear_empties_the_pool() {
        let mut cal = cal(3.0, 1, 1, 8, 0.0, false);
        cal.update_residual(5.0, 3.0, 0, 1);
        cal.step();
        cal.clear();
        assert_eq!(cal.tick(), 0);
        assert_eq!(cal.residual_pool.channel_bucket(0, 0).len(), 0);
    }

    #[test]
    fn calibrated_coverage_on_stationary_noise() {
        // Uniform noise in [−1, 1): a 90% interval should cover ~90% of
        // fresh draws once the pool is warm.
        let mut cal = cal(0.0, 1, 1, 256, 0.0, false);
        let mut s = 0x1234_5678_u32;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        for _ in 0..256 {
            let r = next();
            cal.update_residual(r, 0.0, 0, 1);
        }
        let mut hits = 0;
        let mut iv = PredictiveInterval::new(0.0, 0.0, 0.0, 0.1);
        for _ in 0..2000 {
            let r = next();
            cal.interval_from_point_into(0.0, 0, 1, 0.1, &mut iv);
            if iv.contains(r) {
                hits += 1;
            }
        }
        let cov = hits as f32 / 2000.0;
        assert!((0.85..=0.95).contains(&cov), "coverage {cov}");
    }
}
