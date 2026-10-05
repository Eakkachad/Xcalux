//! Ported from katgpt-rs `crates/katgpt-core/src/welford.rs` and
//! `crates/katgpt-core/src/stats.rs` (MIT, see `NOTICE`). Changes: merged
//! into one module; Welford gained `std_dev` and `merge` (Chan et al.
//! parallel combine, for per-worker bench stats); provenance prose about
//! other repos removed.
//!
//! Online statistics for F2's residual pool, the HUD (latency / p99 error)
//! and `plans/bench/` reports.

/// Welford online mean / variance accumulator — single pass, zero heap.
///
/// Tracks `(count, mean, M2)` per Welford 1962 in f64. Variance is the
/// sample variance `M2 / (n − 1)`; `None` until two observations. NaN inputs
/// are silently rejected (no state change).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WelfordVariance {
    count: usize,
    mean: f64,
    m2: f64,
}

impl WelfordVariance {
    /// New empty accumulator.
    #[inline]
    pub const fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
        }
    }

    /// Reset to empty.
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Number of observations accumulated.
    #[inline]
    pub const fn n(&self) -> usize {
        self.count
    }

    /// Push a new observation (widened to f64). NaN is silently rejected.
    #[inline]
    pub fn observe(&mut self, x: f32) {
        if x.is_nan() {
            return;
        }
        let x = f64::from(x);
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / (self.count as f64);
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    /// Sample variance `M2 / (n − 1)`, or `None` until `n >= 2`.
    ///
    /// Captures dispersion only — NOT bias; for "which stream has smaller
    /// error" use [`mse`](Self::mse).
    #[inline]
    pub fn variance(&self) -> Option<f32> {
        if self.count < 2 {
            None
        } else {
            Some((self.m2 / ((self.count - 1) as f64)) as f32)
        }
    }

    /// Sample standard deviation, or `None` until `n >= 2`.
    #[inline]
    pub fn std_dev(&self) -> Option<f32> {
        self.variance().map(|v| v.max(0.0).sqrt())
    }

    /// Mean squared value vs a zero target: `Var_pop + mean²` (dispersion and
    /// bias). `None` until one observation.
    #[inline]
    pub fn mse(&self) -> Option<f32> {
        if self.count < 1 {
            None
        } else {
            let var_pop = self.m2 / (self.count as f64);
            Some((var_pop + self.mean * self.mean) as f32)
        }
    }

    /// Sample mean, or `0.0` when empty.
    #[inline]
    pub const fn mean(&self) -> f64 {
        self.mean
    }

    /// Fold another accumulator into this one (Chan, Golub & LeVeque
    /// pairwise combine) — the result equals observing both streams.
    pub fn merge(&mut self, other: &Self) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = *other;
            return;
        }
        let n_a = self.count as f64;
        let n_b = other.count as f64;
        let n = n_a + n_b;
        let delta = other.mean - self.mean;
        self.mean += delta * n_b / n;
        self.m2 += other.m2 + delta * delta * n_a * n_b / n;
        self.count += other.count;
    }
}

/// Nearest-rank percentile of a pre-sorted (ascending) slice, with the
/// rank's **tail support**.
///
/// Returns `(value, support)` where `support = n − idx` is the number of
/// samples at or above the returned rank. `1` means "this is just the
/// maximum of n ≤ 1/(1−p) samples — the percentile name is decoration";
/// `≥ 10` means the tail has real footing. Print it in bench reports.
///
/// The rank is the **ceiling** form `ceil(p·n)` clamped to `1..=n`, so
/// `p = 1.0` is the maximum and `p = 0.5` at even `n` is the upper median.
/// (The naive `sorted[(n·p) as usize]` reports the maximum under a
/// percentile's name for every `n ≤ 1/(1−p)`.)
///
/// Contract: `sorted` must be pre-sorted ascending by the caller (floats:
/// `sort_unstable_by(f32::total_cmp)`); `p` is a fraction in `(0, 1]`.
/// Empty input panics — an empty sample has no percentile.
#[inline]
pub fn nearest_rank<T: Copy>(sorted: &[T], p: f64) -> (T, usize) {
    let n = sorted.len();
    assert!(
        n > 0,
        "nearest_rank on an empty sample set — an empty sample has no percentile",
    );
    debug_assert!(
        p > 0.0 && p <= 1.0,
        "nearest_rank p is a fraction in (0, 1], got {p}",
    );
    let idx = ((p * n as f64).ceil() as usize).clamp(1, n) - 1;
    (sorted[idx], n - idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welford_matches_definition() {
        let mut w = WelfordVariance::new();
        let xs = [1.0f32, 2.0, 3.0, 4.0];
        for &x in &xs {
            w.observe(x);
        }
        assert_eq!(w.n(), 4);
        assert!((w.mean() - 2.5).abs() < 1e-12);
        let var = w.variance().unwrap();
        let expected: f32 = xs.iter().map(|x| (x - 2.5).powi(2)).sum::<f32>() / 3.0;
        assert!((var - expected).abs() < 1e-5, "{var} vs {expected}");
        assert!((w.std_dev().unwrap() - expected.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn nan_rejected_and_cold_start_defined() {
        let mut w = WelfordVariance::new();
        assert_eq!(w.n(), 0);
        assert_eq!(w.variance(), None);
        assert_eq!(w.mse(), None);
        assert_eq!(w.mean(), 0.0);
        w.observe(f32::NAN);
        assert_eq!(w.n(), 0, "NaN must be silently rejected");
        w.observe(3.0);
        assert_eq!(w.n(), 1);
        assert_eq!(w.mse(), Some(9.0));
        assert_eq!(w.variance(), None);
        w.reset();
        assert_eq!(w.n(), 0);
    }

    #[test]
    fn merge_equals_single_stream() {
        let xs: Vec<f32> = (0..50)
            .map(|i| ((i * 37) % 11) as f32 * 0.7 - 2.0)
            .collect();
        let mut all = WelfordVariance::new();
        for &x in &xs {
            all.observe(x);
        }
        let (a, b) = xs.split_at(17);
        let mut wa = WelfordVariance::new();
        let mut wb = WelfordVariance::new();
        a.iter().for_each(|&x| wa.observe(x));
        b.iter().for_each(|&x| wb.observe(x));
        wa.merge(&wb);
        assert_eq!(wa.n(), all.n());
        assert!((wa.mean() - all.mean()).abs() < 1e-12);
        assert!((wa.variance().unwrap() - all.variance().unwrap()).abs() < 1e-5);
        // Merging into / from empty is the identity.
        let mut e = WelfordVariance::new();
        e.merge(&all);
        assert_eq!(e, all);
        e.merge(&WelfordVariance::new());
        assert_eq!(e, all);
    }

    #[test]
    fn ends_are_exact() {
        let v: Vec<f64> = (0..10).map(f64::from).collect();
        assert_eq!(nearest_rank(&v, 1.0), (9.0, 1), "p100 is the max");
        assert_eq!(nearest_rank(&v, 0.5), (4.0, 6), "upper median, support 6");
        assert_eq!(
            nearest_rank(&v, 0.99),
            (9.0, 1),
            "p99 of 10 samples IS the max"
        );
    }

    #[test]
    fn support_grows_with_n_at_fixed_p() {
        let v: Vec<f64> = (0..1000).map(f64::from).collect();
        assert_eq!(nearest_rank(&v, 0.99), (989.0, 11));
        let v100: Vec<f64> = (0..100).map(f64::from).collect();
        assert_eq!(nearest_rank(&v100, 0.99), (98.0, 2), "exact-tie boundary");
        let v99: Vec<f64> = (0..99).map(f64::from).collect();
        assert_eq!(nearest_rank(&v99, 0.99), (98.0, 1));
    }

    #[test]
    fn works_for_integer_elements() {
        let sorted = [1u64, 3, 5, 9];
        assert_eq!(nearest_rank(&sorted, 0.95), (9, 1));
    }

    #[test]
    fn empty_input_is_loud() {
        let result = std::panic::catch_unwind(|| nearest_rank::<f64>(&[], 0.99));
        assert!(
            result.is_err(),
            "empty input must assert, not fabricate 0.0"
        );
    }

    #[test]
    fn generic_over_f32_with_caller_total_cmp_sort() {
        let mut v: Vec<f32> = vec![3.0, 1.0, 2.0];
        v.sort_unstable_by(f32::total_cmp);
        assert_eq!(nearest_rank(&v, 0.5), (2.0, 2));
    }
}
