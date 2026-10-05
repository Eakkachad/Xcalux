//! Ported from katgpt-rs `crates/katgpt-types/src/temporal.rs` (MIT, see
//! `NOTICE`); tests from `crates/katgpt-core/src/temporal_deriv.rs`.
//! Changes: SIMD helpers replaced by plain scalar loops (N is 2–4 here);
//! LLM/NPC prose removed; added the ARTY time-correct path
//! ([`ema_alpha`], [`TemporalDerivativeKernel::observe_dt_into`]) and the
//! zero-copy [`TemporalDerivativeKernel::observe_into`].
//!
//! # Dual fast/slow surprise kernel
//!
//! Two EMAs of the same signal with different time constants; their
//! difference `(fast − slow)` is a signed band-pass derivative that spikes
//! when the signal changes and decays to zero when it is steady.
//!
//! Plan F3 (Intent-Aware Stabilizer) feeds it the pen direction / speed:
//! a large surprise means "the artist is turning on purpose", so the
//! stabilizer shortens its time constant τ_t and the corner stays sharp,
//! while long straight runs keep heavy smoothing.
//!
//! The upstream kernel uses a fixed α per sample, which is wrong for a
//! tablet that delivers samples at uneven intervals. [`observe_dt_into`]
//! derives α from the real gap: `α = 1 − exp(−Δt/τ)`, so the response is
//! the same whatever the report rate.
//!
//! [`observe_dt_into`]: TemporalDerivativeKernel::observe_dt_into

/// Time-correct EMA weight for a sample `dt` after the previous one with
/// time constant `tau` (same units): `α = 1 − exp(−Δt/τ)`.
///
/// `dt ≤ 0` → 0 (no update); `tau ≤ 0` or non-finite → 1 (follow the
/// signal).
#[inline]
#[must_use]
pub fn ema_alpha(dt: f32, tau: f32) -> f32 {
    if dt.is_nan() || dt <= 0.0 {
        return 0.0;
    }
    if !(tau.is_finite() && tau > 0.0) {
        return 1.0;
    }
    1.0 - (-dt / tau).exp()
}

/// Dual fast/slow EMA temporal-derivative kernel over `N` channels.
///
/// Invariants: `0 < alpha_slow < alpha_fast ≤ 1` (debug-asserted at
/// construction). Fixed-size arrays, zero heap.
#[derive(Clone, Debug, PartialEq)]
pub struct TemporalDerivativeKernel<const N: usize> {
    /// Fast EMA — short time constant.
    pub fast: [f32; N],
    /// Slow EMA — long time constant.
    pub slow: [f32; N],
    /// Fast EMA coefficient: `fast = (1 − α_f)·fast + α_f·signal`.
    pub alpha_fast: f32,
    /// Slow EMA coefficient: `slow = (1 − α_s)·slow + α_s·signal`.
    pub alpha_slow: f32,
}

impl<const N: usize> TemporalDerivativeKernel<N> {
    /// Zero-initialized kernel. Debug-asserts `0 < alpha_slow < alpha_fast
    /// ≤ 1` (the upstream default ratio is ~10×, e.g. 0.3 / 0.03).
    #[inline]
    pub fn new(alpha_fast: f32, alpha_slow: f32) -> Self {
        validate_alphas(alpha_fast, alpha_slow);
        Self {
            fast: [0.0; N],
            slow: [0.0; N],
            alpha_fast,
            alpha_slow,
        }
    }

    /// Kernel with initial EMA state (warm start; ARTY seeds both EMAs with
    /// the first sample at pen-down so the first surprise is zero).
    #[inline]
    pub fn with_initial(fast: [f32; N], slow: [f32; N], alpha_fast: f32, alpha_slow: f32) -> Self {
        validate_alphas(alpha_fast, alpha_slow);
        Self {
            fast,
            slow,
            alpha_fast,
            alpha_slow,
        }
    }

    /// Observe one sample with the stored per-sample alphas; returns the
    /// signed surprise `(fast − slow)`.
    #[inline]
    pub fn observe(&mut self, signal: &[f32; N]) -> [f32; N] {
        let mut out = [0.0f32; N];
        self.observe_into(signal, &mut out);
        out
    }

    /// [`observe`](Self::observe) writing the surprise into `out`.
    #[inline]
    pub fn observe_into(&mut self, signal: &[f32; N], out: &mut [f32; N]) {
        let (af, asl) = (self.alpha_fast, self.alpha_slow);
        self.step(signal, af, asl, out);
    }

    /// Time-correct observe: alphas come from the real sample gap `dt` and
    /// the time constants `tau_fast < tau_slow` (same units as `dt`) via
    /// [`ema_alpha`]. The stored alphas are not used or changed.
    #[inline]
    pub fn observe_dt_into(
        &mut self,
        signal: &[f32; N],
        dt: f32,
        tau_fast: f32,
        tau_slow: f32,
        out: &mut [f32; N],
    ) {
        debug_assert!(tau_fast < tau_slow, "tau_fast must be < tau_slow");
        self.step(
            signal,
            ema_alpha(dt, tau_fast),
            ema_alpha(dt, tau_slow),
            out,
        );
    }

    #[inline]
    fn step(&mut self, signal: &[f32; N], af: f32, asl: f32, out: &mut [f32; N]) {
        for i in 0..N {
            self.fast[i] = (1.0 - af) * self.fast[i] + af * signal[i];
            self.slow[i] = (1.0 - asl) * self.slow[i] + asl * signal[i];
            out[i] = self.fast[i] - self.slow[i];
        }
    }

    /// L2 norm of the current `(fast − slow)` derivative.
    #[inline]
    pub fn surprise_norm(&self) -> f32 {
        let mut sq = 0.0f32;
        for i in 0..N {
            let d = self.fast[i] - self.slow[i];
            sq += d * d;
        }
        sq.max(0.0).sqrt()
    }

    /// Write `(fast − slow)` into `out`.
    #[inline]
    pub fn derivative_slice(&self, out: &mut [f32; N]) {
        for i in 0..N {
            out[i] = self.fast[i] - self.slow[i];
        }
    }

    /// Zero both EMAs.
    #[inline]
    pub fn reset(&mut self) {
        self.fast = [0.0; N];
        self.slow = [0.0; N];
    }

    /// Set both EMAs to `signal` (surprise becomes zero) — pen-down seed.
    #[inline]
    pub fn reset_to(&mut self, signal: &[f32; N]) {
        self.fast = *signal;
        self.slow = *signal;
    }
}

impl<const N: usize> Default for TemporalDerivativeKernel<N> {
    /// Upstream default ~10× ratio: `alpha_fast = 0.3`, `alpha_slow = 0.03`.
    #[inline]
    fn default() -> Self {
        Self::new(0.3, 0.03)
    }
}

/// Project a derivative vector onto one bounded scalar:
/// `sigmoid(β · ‖derivative‖₂)` ∈ [0.5, 1) for β ≥ 0. Sigmoid, never softmax.
#[inline]
pub fn sigmoid_surprise_gate(derivative: &[f32], beta: f32) -> f32 {
    debug_assert!(
        beta.is_finite(),
        "sigmoid_surprise_gate: beta must be finite"
    );
    let sq: f32 = derivative.iter().map(|d| d * d).sum();
    crate::sigmoid(beta * sq.max(0.0).sqrt())
}

#[inline]
fn validate_alphas(alpha_fast: f32, alpha_slow: f32) {
    debug_assert!(
        alpha_slow > 0.0 && alpha_fast > alpha_slow && alpha_fast <= 1.0,
        "TemporalDerivativeKernel: require 0 < alpha_slow < alpha_fast <= 1, got fast={alpha_fast}, slow={alpha_slow}"
    );
    let _ = (alpha_fast, alpha_slow);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_signal_yields_zero_derivative() {
        let mut k: TemporalDerivativeKernel<4> = TemporalDerivativeKernel::new(0.5, 0.05);
        for _ in 0..10 {
            let d = k.observe(&[0.0; 4]);
            assert!(d.iter().all(|x| *x == 0.0));
        }
        assert_eq!(k.surprise_norm(), 0.0);
    }

    #[test]
    fn constant_signal_converges_to_zero_derivative() {
        let mut k: TemporalDerivativeKernel<2> = TemporalDerivativeKernel::new(0.5, 0.05);
        let signal = [0.5f32, 0.5];
        let _ = k.observe(&signal);
        let early_norm = k.surprise_norm();
        assert!(early_norm > 0.0);
        for _ in 0..2000 {
            k.observe(&signal);
        }
        assert!(k.surprise_norm() < early_norm * 0.01);
    }

    /// The derivative is not monotone from t=1 (fast rises while slow barely
    /// moves), so check: spike positive, then decays below 10% of its peak.
    #[test]
    fn step_up_signal_produces_positive_spike() {
        let mut k: TemporalDerivativeKernel<1> = TemporalDerivativeKernel::new(0.5, 0.05);
        for _ in 0..100 {
            k.observe(&[0.0]);
        }
        assert!(k.surprise_norm() < 1e-6);
        let d = k.observe(&[1.0]);
        assert!(d[0] > 0.0);
        let mut peak = k.surprise_norm();
        for _ in 0..20 {
            k.observe(&[1.0]);
            peak = peak.max(k.surprise_norm());
        }
        for _ in 0..2000 {
            k.observe(&[1.0]);
        }
        assert!(k.surprise_norm() < peak * 0.1);
    }

    #[test]
    fn step_down_signal_produces_negative_spike() {
        let mut k: TemporalDerivativeKernel<1> = TemporalDerivativeKernel::new(0.5, 0.05);
        for _ in 0..1000 {
            k.observe(&[1.0]);
        }
        assert!(k.surprise_norm() < 1e-3);
        let d = k.observe(&[0.0]);
        assert!(d[0] < 0.0);
    }

    #[test]
    #[should_panic(expected = "require 0 < alpha_slow < alpha_fast <= 1")]
    #[cfg(debug_assertions)]
    fn swapped_alphas_panics_in_debug() {
        let _ = TemporalDerivativeKernel::<4>::new(0.05, 0.5);
    }

    #[test]
    fn reset_zeroes_state() {
        let mut k: TemporalDerivativeKernel<2> = TemporalDerivativeKernel::new(0.5, 0.05);
        k.observe(&[1.0, 1.0]);
        k.observe(&[1.0, 1.0]);
        assert!(k.surprise_norm() > 0.0);
        k.reset();
        assert_eq!(k.fast, [0.0, 0.0]);
        assert_eq!(k.slow, [0.0, 0.0]);
        assert_eq!(k.surprise_norm(), 0.0);
        k.reset_to(&[3.0, -1.0]);
        assert_eq!(k.surprise_norm(), 0.0);
        assert_eq!(k.fast, [3.0, -1.0]);
    }

    #[test]
    fn surprise_norm_matches_manual_l2() {
        let k: TemporalDerivativeKernel<4> = TemporalDerivativeKernel::with_initial(
            [0.3, -0.7, 0.1, 0.5],
            [0.1, -0.2, 0.05, 0.1],
            0.5,
            0.05,
        );
        let mut diff = [0.0f32; 4];
        k.derivative_slice(&mut diff);
        let manual = diff.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((k.surprise_norm() - manual).abs() < 1e-5);
    }

    #[test]
    fn derivative_slice_matches_observe_output() {
        let mut k: TemporalDerivativeKernel<3> = TemporalDerivativeKernel::new(0.4, 0.04);
        let d = k.observe(&[0.7, -0.3, 0.5]);
        let mut buf = [0.0f32; 3];
        k.derivative_slice(&mut buf);
        assert_eq!(d, buf);
    }

    #[test]
    fn observe_into_matches_observe() {
        let mut a: TemporalDerivativeKernel<8> = TemporalDerivativeKernel::new(0.4, 0.04);
        let mut b = a.clone();
        let signal = [0.5f32; 8];
        let mut out = [0.0f32; 8];
        for _ in 0..50 {
            let da = a.observe(&signal);
            b.observe_into(&signal, &mut out);
            assert_eq!(da, out);
        }
    }

    #[test]
    fn sigmoid_surprise_gate_is_bounded_and_monotone() {
        let g_zero = sigmoid_surprise_gate(&[0.0f32; 4], 4.0);
        let g_small = sigmoid_surprise_gate(&[0.1f32; 4], 4.0);
        let g_big = sigmoid_surprise_gate(&[1.0f32; 4], 4.0);
        assert!(g_zero > 0.0 && g_zero < 1.0);
        assert!(g_small > g_zero);
        assert!(g_big > g_small);
        assert!(g_big < 1.0);
    }

    #[test]
    fn default_is_ten_to_one_ratio() {
        let k: TemporalDerivativeKernel<4> = TemporalDerivativeKernel::default();
        assert!((k.alpha_fast - 0.3).abs() < 1e-6);
        assert!((k.alpha_slow - 0.03).abs() < 1e-6);
    }

    #[test]
    fn with_initial_preserves_state() {
        let k: TemporalDerivativeKernel<2> =
            TemporalDerivativeKernel::with_initial([0.5, 0.5], [0.1, 0.1], 0.4, 0.04);
        assert_eq!(k.fast, [0.5, 0.5]);
        assert_eq!(k.slow, [0.1, 0.1]);
    }

    // ===== ARTY additions: time-correct EMA =====

    #[test]
    fn ema_alpha_edges_and_composition() {
        assert_eq!(ema_alpha(0.0, 1.0), 0.0);
        assert_eq!(ema_alpha(-1.0, 1.0), 0.0);
        assert_eq!(ema_alpha(f32::NAN, 1.0), 0.0);
        assert_eq!(ema_alpha(1.0, 0.0), 1.0);
        assert!((ema_alpha(1.0, 1.0) - (1.0 - (-1.0f32).exp())).abs() < 1e-7);
        // Two half-steps decay exactly like one full step.
        let (a_half, a_full) = (ema_alpha(0.5, 2.0), ema_alpha(1.0, 2.0));
        let two_half = 1.0 - (1.0 - a_half) * (1.0 - a_half);
        assert!((two_half - a_full).abs() < 1e-6);
    }

    /// The point of the time-correct path: the same physical motion sampled
    /// at 133 Hz or 240 Hz (or unevenly) ends in the same EMA state.
    #[test]
    fn observe_dt_is_report_rate_independent() {
        // A sample's value holds back to the previous sample, so the step is
        // placed where both grids see its onset at t = 0.05.
        let signal = |t: f32| if t < 0.051 { 0.0 } else { 1.0 };
        let run = |times: &[f32]| {
            let mut k = TemporalDerivativeKernel::<1>::new(0.5, 0.05);
            let mut out = [0.0f32];
            let mut prev = 0.0f32;
            for &t in times {
                k.observe_dt_into(&[signal(t)], t - prev, 0.004, 0.04, &mut out);
                prev = t;
            }
            (k.fast[0], k.slow[0])
        };
        let a: Vec<f32> = (1..=40).map(|i| i as f32 * 0.0025).collect(); // 400 Hz
        let b: Vec<f32> = (1..=20).map(|i| i as f32 * 0.005).collect(); // 200 Hz
        let (fa, sa) = run(&a);
        let (fb, sb) = run(&b);
        assert!((fa - fb).abs() < 1e-3, "fast {fa} vs {fb}");
        assert!((sa - sb).abs() < 1e-3, "slow {sa} vs {sb}");
        // A fixed per-sample alpha does NOT have this property.
        let mut ka = TemporalDerivativeKernel::<1>::new(0.5, 0.05);
        let mut kb = ka.clone();
        a.iter().for_each(|&t| {
            ka.observe(&[signal(t)]);
        });
        b.iter().for_each(|&t| {
            kb.observe(&[signal(t)]);
        });
        assert!((ka.slow[0] - kb.slow[0]).abs() > 0.05);
    }
}
