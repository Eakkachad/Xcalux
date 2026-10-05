//! ARTY-original (not ported): finite differences on real timestamps.
//!
//! Plan F2 step 1: "a divided-difference predictor of order ≤ 3 on Windows
//! Ink's real timestamps (x, y, p), with no resampling and therefore no extra
//! lag; the ported [`KinState`](super::KinState) is the oracle in tests".
//!
//! [`DividedDiff`] keeps the last four samples and their timestamps and fits
//! the Newton interpolating polynomial through them, anchored at the newest
//! sample:
//!
//! ```text
//! p(t) = f[t₀] + f[t₀,t₁]·(t−t₀) + f[t₀,t₁,t₂]·(t−t₀)(t−t₁)
//!      + f[t₀,t₁,t₂,t₃]·(t−t₀)(t−t₁)(t−t₂)
//! ```
//!
//! On a uniform tick this is exactly the Newton backward form that
//! [`kinematic_extrapolate_into`](super::kinematic_extrapolate_into) uses
//! (`f[t₀,t₁] = vel`, `f[t₀,t₁,t₂] = acc/2`, `f[t₀..t₃] = jerk/6`), so the
//! two must agree; the tests pin that. Unlike `KinState` it accepts uneven
//! gaps, which is what a tablet actually delivers.
//!
//! Arithmetic is f64 internally (timestamps in seconds lose precision in f32
//! after a few hours of uptime; the cost is a handful of flops per sample).
//! Positions in and out are f32. Zero heap, `Copy`.

use super::KinError;

/// Window length: four samples saturate the order-3 ladder.
const WIN: usize = 4;

/// Newton divided-difference state over `D` channels on real timestamps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DividedDiff<const D: usize> {
    /// Timestamps, newest first (`t[0]` is the anchor).
    t: [f64; WIN],
    /// Samples, newest first.
    x: [[f64; D]; WIN],
    /// Newton coefficients anchored at `t[0]`: `c[0] = f[t₀]`,
    /// `c[1] = f[t₀,t₁]`, `c[2] = f[t₀,t₁,t₂]`, `c[3] = f[t₀..t₃]`.
    c: [[f64; D]; WIN],
    /// Samples absorbed (saturates at 4).
    n: u8,
}

impl<const D: usize> Default for DividedDiff<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const D: usize> DividedDiff<D> {
    /// Empty state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            t: [0.0; WIN],
            x: [[0.0; D]; WIN],
            c: [[0.0; D]; WIN],
            n: 0,
        }
    }

    /// Forget all samples. Call at pen-down.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Samples absorbed so far (saturates at 4).
    #[must_use]
    pub fn n_obs(&self) -> u8 {
        self.n
    }

    /// Highest polynomial order the window supports (`n_obs − 1`, max 3).
    #[must_use]
    pub fn ladder_order(&self) -> u8 {
        self.n.saturating_sub(1)
    }

    /// Timestamp of the newest sample (the anchor).
    #[must_use]
    pub fn anchor_time(&self) -> f64 {
        self.t[0]
    }

    /// Gap between the two newest samples, or `None` with fewer than two.
    #[must_use]
    pub fn last_dt(&self) -> Option<f64> {
        (self.n >= 2).then(|| self.t[0] - self.t[1])
    }

    /// Absorb one sample taken at time `t` (seconds, any epoch).
    ///
    /// Screens: non-finite samples or times are refused; `t` must be strictly
    /// greater than the previous sample's time (merge same-timestamp
    /// duplicates before calling). On error the state is unchanged.
    pub fn observe_into(&mut self, pos: &[f32; D], t: f64) -> Result<(), KinError> {
        if pos.iter().any(|x| !x.is_finite()) || !t.is_finite() {
            return Err(KinError::NonFinite);
        }
        if self.n > 0 && t <= self.t[0] {
            return Err(KinError::NonMonotonicTick);
        }
        for i in (1..WIN).rev() {
            self.t[i] = self.t[i - 1];
            self.x[i] = self.x[i - 1];
        }
        self.t[0] = t;
        for ch in 0..D {
            self.x[0][ch] = f64::from(pos[ch]);
        }
        self.n = (self.n + 1).min(WIN as u8);
        self.recompute();
        Ok(())
    }

    /// Rebuild the Newton coefficients from the window (≤ 4 points, so the
    /// whole table is a dozen flops per channel).
    fn recompute(&mut self) {
        let n = self.n as usize;
        let t = self.t;
        for ch in 0..D {
            // Divided-difference table, column by column, newest first:
            // after pass k, d[i] = f[t_i .. t_{i+k}].
            let mut d = [0.0f64; WIN];
            for i in 0..n {
                d[i] = self.x[i][ch];
            }
            self.c[0][ch] = d[0];
            for k in 1..WIN {
                if k < n {
                    for i in 0..(n - k) {
                        d[i] = (d[i] - d[i + 1]) / (t[i] - t[i + k]);
                    }
                    self.c[k][ch] = d[0];
                } else {
                    self.c[k][ch] = 0.0;
                }
            }
        }
    }

    /// Evaluate the fitted polynomial at time `t_target`, using at most
    /// `max_order` (clamped to the ladder order). `t_target` past the anchor
    /// extrapolates; inside the window it interpolates.
    pub fn extrapolate_into(
        &self,
        t_target: f64,
        max_order: u8,
        out: &mut [f32; D],
    ) -> Result<(), KinError> {
        if self.n == 0 {
            return Err(KinError::NotEnoughObs);
        }
        if !t_target.is_finite() {
            return Err(KinError::NonFinite);
        }
        let order = max_order.min(self.ladder_order()) as usize;
        let h0 = t_target - self.t[0];
        let h1 = t_target - self.t[1];
        let h2 = t_target - self.t[2];
        // Newton basis products ω₁ = h0, ω₂ = h0·h1, ω₃ = h0·h1·h2.
        let w = [1.0, h0, h0 * h1, h0 * h1 * h2];
        for ch in 0..D {
            let mut p = self.c[0][ch];
            for k in 1..=order {
                p += self.c[k][ch] * w[k];
            }
            out[ch] = p as f32;
        }
        Ok(())
    }

    /// Instantaneous derivatives of the fitted polynomial at the anchor,
    /// using at most `max_order` terms: velocity `p'(t₀)`, acceleration
    /// `p''(t₀)` and jerk `p'''(t₀)` per channel (units per second, s², s³).
    ///
    /// Unlike `KinState::vel` (the mean velocity over the last step) this is
    /// the derivative *at* the newest sample — what a stabilizer or a corner
    /// detector wants.
    pub fn derivatives_into(
        &self,
        max_order: u8,
        vel: &mut [f32; D],
        acc: &mut [f32; D],
        jerk: &mut [f32; D],
    ) -> Result<(), KinError> {
        if self.n == 0 {
            return Err(KinError::NotEnoughObs);
        }
        let order = max_order.min(self.ladder_order());
        let a = self.t[0] - self.t[1]; // (t₀ − t₁)
        let b = self.t[0] - self.t[2]; // (t₀ − t₂)
        for ch in 0..D {
            let (c1, c2, c3) = (self.c[1][ch], self.c[2][ch], self.c[3][ch]);
            let (c2, c3) = match order {
                0 | 1 => (0.0, 0.0),
                2 => (c2, 0.0),
                _ => (c2, c3),
            };
            let c1 = if order == 0 { 0.0 } else { c1 };
            // p(t) = c0 + c1·(t−t₀) + c2·(t−t₀)(t−t₁) + c3·(t−t₀)(t−t₁)(t−t₂)
            vel[ch] = (c1 + c2 * a + c3 * a * b) as f32;
            acc[ch] = (2.0 * c2 + 2.0 * c3 * (a + b)) as f32;
            jerk[ch] = (6.0 * c3) as f32;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinematics::{KinState, Sched, kinematic_extrapolate_into};

    #[test]
    fn screens_bad_input_and_leaves_state_unchanged() {
        let mut dd = DividedDiff::<1>::new();
        assert_eq!(
            dd.extrapolate_into(1.0, 3, &mut [0.0]).unwrap_err(),
            KinError::NotEnoughObs
        );
        dd.observe_into(&[1.0], 0.5).unwrap();
        let before = dd;
        assert_eq!(
            dd.observe_into(&[f32::NAN], 1.0).unwrap_err(),
            KinError::NonFinite
        );
        assert_eq!(
            dd.observe_into(&[1.0], f64::INFINITY).unwrap_err(),
            KinError::NonFinite
        );
        assert_eq!(
            dd.observe_into(&[2.0], 0.5).unwrap_err(),
            KinError::NonMonotonicTick
        );
        assert_eq!(
            dd.observe_into(&[2.0], 0.4).unwrap_err(),
            KinError::NonMonotonicTick
        );
        assert_eq!(dd, before);
    }

    #[test]
    fn ladder_grows_to_three() {
        let mut dd = DividedDiff::<1>::new();
        for (i, t) in [0.0, 0.004, 0.009, 0.0131, 0.017].iter().enumerate() {
            dd.observe_into(&[i as f32], *t).unwrap();
            assert_eq!(dd.ladder_order(), (i as u8).min(3));
        }
        assert!((dd.last_dt().unwrap() - (0.017 - 0.0131)).abs() < 1e-12);
    }

    /// Oracle: on a uniform tick the divided-difference predictor and the
    /// ported KinState rollout give the same answer (bit-equal on dyadic
    /// data, where every operation in both paths is exact).
    #[test]
    fn matches_kinstate_oracle_on_uniform_ticks() {
        let dt = 0.25f32; // dyadic
        let traj = |t: f32| 0.5 * t * t * t - 1.25 * t * t + 3.0 * t + 2.0;
        let mut ks = KinState::<1>::new(dt).unwrap();
        let mut dd = DividedDiff::<1>::new();
        for i in 0..6u32 {
            let t = i as f32 * dt;
            ks.observe_into(&[traj(t)], i).unwrap();
            dd.observe_into(&[traj(t)], f64::from(t)).unwrap();
        }
        let t0 = 5.0 * dt;
        for k in [1u32, 2, 3, 8] {
            let mut a = [0.0f32];
            let mut b = [0.0f32];
            kinematic_extrapolate_into(&ks, k, &Sched::Measured, &mut a).unwrap();
            dd.extrapolate_into(f64::from(t0 + k as f32 * dt), 3, &mut b)
                .unwrap();
            assert_eq!(
                a[0].to_bits(),
                b[0].to_bits(),
                "k={k}: {} vs {}",
                a[0],
                b[0]
            );
            assert_eq!(b[0], traj(t0 + k as f32 * dt), "exact on the cubic, k={k}");
        }
        // Lower orders match the capped KinState rollout too.
        for order in 0u8..=2 {
            let mut a = [0.0f32];
            let mut b = [0.0f32];
            crate::kinematics::kinematic_extrapolate_capped_into(
                &ks,
                2,
                &Sched::Measured,
                order,
                &mut a,
            )
            .unwrap();
            dd.extrapolate_into(f64::from(t0 + 2.0 * dt), order, &mut b)
                .unwrap();
            assert_eq!(a[0], b[0], "order {order}");
        }
    }

    /// On random-ish float data (non-dyadic dt) the two agree to rounding.
    #[test]
    fn matches_kinstate_oracle_within_rounding_on_non_dyadic_data() {
        let dt = 1.0f32 / 240.0;
        let mut ks = KinState::<2>::new(dt).unwrap();
        let mut dd = DividedDiff::<2>::new();
        for i in 0..10u32 {
            let t = i as f32 * dt;
            let p = [
                100.0 + 300.0 * t + 40.0 * (7.0 * t).sin(),
                50.0 - 80.0 * t * t,
            ];
            ks.observe_into(&p, i).unwrap();
            dd.observe_into(&p, f64::from(i) * f64::from(dt)).unwrap();
        }
        let mut a = [0.0f32; 2];
        let mut b = [0.0f32; 2];
        kinematic_extrapolate_into(&ks, 2, &Sched::Measured, &mut a).unwrap();
        dd.extrapolate_into(11.0 * f64::from(dt), 3, &mut b)
            .unwrap();
        for ch in 0..2 {
            assert!(
                (a[ch] - b[ch]).abs() < 1e-3,
                "ch{ch}: {} vs {}",
                a[ch],
                b[ch]
            );
        }
    }

    /// The point of the module: irregular timestamps (tablet bursts) stay
    /// exact on a cubic, where a fixed-tick stencil would be biased.
    #[test]
    fn exact_on_cubic_with_irregular_timestamps() {
        let traj = |t: f64| 2.0 * t * t * t - 3.0 * t * t + 0.5 * t + 7.0;
        let ts = [0.0, 0.003, 0.0105, 0.0122, 0.0201, 0.0244];
        let mut dd = DividedDiff::<1>::new();
        for &t in &ts {
            dd.observe_into(&[traj(t) as f32], t).unwrap();
        }
        for target in [0.026, 0.030, 0.040] {
            let mut out = [0.0f32];
            dd.extrapolate_into(target, 3, &mut out).unwrap();
            let want = traj(target) as f32;
            assert!(
                (out[0] - want).abs() <= 1e-5 * want.abs().max(1.0),
                "t={target}"
            );
        }
        // Interpolating back to a window sample reproduces it.
        let mut out = [0.0f32];
        dd.extrapolate_into(0.0122, 3, &mut out).unwrap();
        assert!((out[0] - traj(0.0122) as f32).abs() < 1e-5);
    }

    #[test]
    fn derivatives_at_anchor_are_instantaneous() {
        // x(t) = t³ − 2t² + 5t: x' = 3t² − 4t + 5, x'' = 6t − 4, x''' = 6.
        let traj = |t: f64| t * t * t - 2.0 * t * t + 5.0 * t;
        let ts = [0.0, 0.1, 0.25, 0.3, 0.42];
        let mut dd = DividedDiff::<1>::new();
        for &t in &ts {
            dd.observe_into(&[traj(t) as f32], t).unwrap();
        }
        let t0 = 0.42f64;
        let (mut v, mut a, mut j) = ([0.0f32], [0.0f32], [0.0f32]);
        dd.derivatives_into(3, &mut v, &mut a, &mut j).unwrap();
        assert!((f64::from(v[0]) - (3.0 * t0 * t0 - 4.0 * t0 + 5.0)).abs() < 1e-4);
        assert!((f64::from(a[0]) - (6.0 * t0 - 4.0)).abs() < 1e-3);
        assert!((f64::from(j[0]) - 6.0).abs() < 1e-2);
        // Order 1: velocity is the last chord slope, no curvature terms.
        dd.derivatives_into(1, &mut v, &mut a, &mut j).unwrap();
        let chord = (traj(0.42) - traj(0.3)) / 0.12;
        assert!((f64::from(v[0]) - chord).abs() < 1e-4);
        assert_eq!((a[0], j[0]), (0.0, 0.0));
    }

    #[test]
    fn reset_forgets_everything() {
        let mut dd = DividedDiff::<3>::new();
        dd.observe_into(&[1.0, 2.0, 0.5], 1.0).unwrap();
        dd.reset();
        assert_eq!(dd, DividedDiff::new());
        // A fresh stroke may start at an earlier timestamp than the old one.
        dd.observe_into(&[0.0, 0.0, 0.0], 0.0).unwrap();
    }
}
