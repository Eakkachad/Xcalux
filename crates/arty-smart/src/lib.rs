//! `arty-smart` — the deterministic math behind ARTY's assists.
//!
//! Every assist in ARTY is geometry or a written rule: no neural nets, no
//! training, nothing that learns behind the artist's back (see
//! `plans/differentiation_plan.md` §1). This crate holds the small, tested
//! building blocks those assists need, so the app crates never depend on an
//! external research repo:
//!
//! | Module | What it gives | Used by (plan) |
//! |---|---|---|
//! | [`kinematics`] | finite-difference pen state on a fixed tick ([`kinematics::KinState`]) and on real timestamps ([`kinematics::DividedDiff`]), closed-form extrapolation, error bound + admission horizon, residual CUSUM monitor, regime classifier | F2 Predicted Ink, F3 Intent-Aware Stabilizer, F6 soft ruler |
//! | [`conformal`] | conformal predictive intervals over any point forecaster, interval metrics, the naive floor forecaster | F2 confidence gate |
//! | [`stats`] | Welford mean/variance, nearest-rank percentiles with tail support | F2 residual stats, HUD, benches |
//! | [`temporal`] | dual fast/slow EMA surprise kernel, time-correct EMA weights | F3 |
//! | [`cumprodsum`] | the `h = a·h + x` scan | F3 |
//! | [`partition`] | min-max contiguous DP partition with reusable scratch | F4 Stroke Grammar, F6 |
//! | [`geom`] | PCA tangent, chord / circle deviation, Kåsa circle fit, turning angle | F1 closure baseline, F4 |
//!
//! Most of it is ported from katgpt-rs (MIT); each ported file names its
//! source and the crate's `NOTICE` carries the licence. Nothing here is
//! wired into the app yet.
//!
//! # Rules this crate keeps
//!
//! - `#![forbid(unsafe_code)]`, zero runtime dependencies.
//! - Builds on the workspace MSRV (1.88).
//! - Everything callable per pen sample is allocation-free: state is plain
//!   `Copy` data or a scratch struct sized once up front, and outputs go
//!   through `&mut` (`*_into`). `tests/alloc_gate.rs` enforces this with
//!   `arty-testkit`'s counting allocator.
//! - Deterministic: same inputs give the same bits on the same build.

#![forbid(unsafe_code)]

pub mod conformal;
pub mod cumprodsum;
pub mod geom;
pub mod kinematics;
pub mod partition;
pub mod stats;
pub mod temporal;

/// Logistic sigmoid `1 / (1 + e^{-x})` in the numerically stable two-branch
/// form over `f32::exp`.
///
/// katgpt-rs gates use a SIMD polynomial approximation; ARTY uses the exact
/// libm form instead (one call per sample is nowhere near a cost problem, and
/// it keeps the gates easy to reason about and bit-stable across CPUs that
/// share a libm).
#[inline]
#[must_use]
pub fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

#[cfg(test)]
mod tests {
    use super::sigmoid;

    #[test]
    fn sigmoid_is_bounded_symmetric_and_monotone() {
        assert_eq!(sigmoid(0.0), 0.5);
        for x in [0.1f32, 1.0, 3.0, 10.0, 50.0] {
            let (p, n) = (sigmoid(x), sigmoid(-x));
            assert!((p + n - 1.0).abs() < 1e-6, "symmetry at {x}");
            assert!(p > 0.5 && p <= 1.0);
        }
        let mut prev = 0.0;
        for i in -100..=100 {
            let s = sigmoid(i as f32 * 0.1);
            assert!(s >= prev);
            prev = s;
        }
        assert!(sigmoid(-200.0).is_finite() && sigmoid(200.0).is_finite());
    }
}
