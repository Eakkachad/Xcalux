//! User pressure curve (CSP "Adjust pen pressure").
//!
//! A monotone cubic (PCHIP, Fritsch–Butland tangents) through 2 to
//! [`MAX_CURVE_POINTS`] points. The hot path never evaluates the cubic: every
//! edit rebuilds a [`CURVE_LUT_LEN`]-interval table and [`PressureCurve::eval`]
//! is one lookup and a lerp. Only the points are persisted; the table is rebuilt
//! on load.

use serde::{Deserialize, Serialize};

pub const MAX_CURVE_POINTS: usize = 8;
/// Intervals; the LUT has `LEN + 1` entries (≈4 KiB).
pub const CURVE_LUT_LEN: usize = 1024;
/// Minimum x distance between points.
pub const MIN_GAP: f32 = 0.04;

/// Slack on `MIN_GAP` checks: a point clamped to `neighbour ± MIN_GAP` can land
/// one f32 ulp short of it, and must still count as far enough.
const GAP_EPS: f32 = 1e-6;

/// Inputs of the legacy `p^gamma` approximation. Dense at the start, where
/// `p^gamma` bends hardest for gamma < 1; every gap is ≥ `MIN_GAP`.
const GAMMA_X: [f32; MAX_CURVE_POINTS] = [0.0, 0.04, 0.1, 0.2, 0.35, 0.55, 0.77, 1.0];

/// Maps raw pen pressure to brush pressure. Points are sorted by x, the first at
/// x = 0 and the last at x = 1, at least [`MIN_GAP`] apart, all within 0..=1.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(from = "CurveRepr", into = "CurveRepr")]
pub struct PressureCurve {
    pts: [[f32; 2]; MAX_CURVE_POINTS],
    len: u8,
    lut: [f32; CURVE_LUT_LEN + 1],
}

/// Persisted form: only the points (sanitized again on load).
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct CurveRepr {
    points: Vec<[f32; 2]>,
}

impl From<CurveRepr> for PressureCurve {
    fn from(r: CurveRepr) -> Self {
        Self::from_points(&r.points)
    }
}

impl From<PressureCurve> for CurveRepr {
    fn from(c: PressureCurve) -> Self {
        Self { points: c.points().to_vec() }
    }
}

impl std::fmt::Debug for PressureCurve {
    /// Points only: the LUT is derived from them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PressureCurve").field("points", &self.points()).finish_non_exhaustive()
    }
}

/// Same points, same curve: unused slots and the derived LUT are not compared.
impl PartialEq for PressureCurve {
    fn eq(&self, other: &Self) -> bool {
        self.points() == other.points()
    }
}

impl Default for PressureCurve {
    fn default() -> Self {
        Self::linear()
    }
}

/// `b` is at least `MIN_GAP` to the right of `a`.
fn far(a: f32, b: f32) -> bool {
    b - a >= MIN_GAP - GAP_EPS
}

impl PressureCurve {
    /// The identity: [(0,0), (1,1)].
    pub fn linear() -> Self {
        Self::with_points(&[[0.0, 0.0], [1.0, 1.0]])
    }

    /// Sanitize: drop non-finite, clamp to 0..=1, sort by x, pin first x = 0 and last x = 1,
    /// drop points closer than MIN_GAP (keeping endpoints), truncate to 8; < 2 points → linear.
    pub fn from_points(pts: &[[f32; 2]]) -> Self {
        let mut v: Vec<[f32; 2]> = pts
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();
        if v.len() < 2 {
            return Self::linear();
        }
        v.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let last = v.len() - 1;
        v[0][0] = 0.0;
        v[last][0] = 1.0;
        let mut out = vec![v[0]];
        for &p in &v[1..last] {
            if out.len() < MAX_CURVE_POINTS - 1 && far(out[out.len() - 1][0], p[0]) && far(p[0], 1.0) {
                out.push(p);
            }
        }
        out.push(v[last]);
        Self::with_points(&out)
    }

    /// 8-point approximation of p^gamma at GAMMA_X (legacy `pressure_gamma` migration).
    /// `gamma` is clamped to 0.2..=5 as the old setting was; ≈1 (or non-finite) gives linear.
    pub fn from_gamma(gamma: f32) -> Self {
        if !gamma.is_finite() {
            return Self::linear();
        }
        let g = gamma.clamp(0.2, 5.0);
        if (g - 1.0).abs() < 1e-3 {
            return Self::linear();
        }
        Self::from_points(&GAMMA_X.map(|x| [x, x.powf(g)]))
    }

    pub fn points(&self) -> &[[f32; 2]] {
        &self.pts[..self.len as usize]
    }

    /// Every point lies on the diagonal, so the curve is the identity.
    pub fn is_linear(&self) -> bool {
        self.points().iter().all(|p| p[0] == p[1])
    }

    /// Hot path: LUT + lerp, branch-light, allocation-free. NaN/≤0 → lut[0]; ≥1 → lut[LEN].
    #[inline]
    pub fn eval(&self, p: f32) -> f32 {
        if p.is_nan() || p <= 0.0 {
            return self.lut[0];
        }
        if p >= 1.0 {
            return self.lut[CURVE_LUT_LEN];
        }
        let f = p * CURVE_LUT_LEN as f32;
        let i = (f as usize).min(CURVE_LUT_LEN - 1);
        let (a, b) = (self.lut[i], self.lut[i + 1]);
        a + (b - a) * (f - i as f32)
    }

    /// Exact monotone cubic (tests, LUT build). NaN/≤0 evaluates at 0, ≥1 at 1.
    pub fn eval_exact(&self, p: f32) -> f32 {
        self.hermite(&self.tangents(), p)
    }

    /// Move point i (endpoints: y only; interior: x clamped to (prev+MIN_GAP, next−MIN_GAP), y to 0..=1).
    /// Rebuilds the LUT. Returns whether the curve changed.
    pub fn set_point(&mut self, i: usize, p: [f32; 2]) -> bool {
        let n = self.len as usize;
        if i >= n || !p[0].is_finite() || !p[1].is_finite() {
            return false;
        }
        let x = if i == 0 || i == n - 1 {
            self.pts[i][0]
        } else {
            let (lo, hi) = (self.pts[i - 1][0] + MIN_GAP, self.pts[i + 1][0] - MIN_GAP);
            if lo <= hi { p[0].clamp(lo, hi) } else { self.pts[i][0] }
        };
        let q = [x, p[1].clamp(0.0, 1.0)];
        if q == self.pts[i] {
            return false;
        }
        self.pts[i] = q;
        self.rebuild();
        true
    }

    /// Insert keeping x order; None if full or within MIN_GAP of a neighbour. Rebuilds LUT.
    pub fn insert_point(&mut self, p: [f32; 2]) -> Option<usize> {
        let n = self.len as usize;
        if n >= MAX_CURVE_POINTS || !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        let p = [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)];
        let j = self.points().iter().position(|q| q[0] > p[0]).filter(|&j| j > 0)?;
        if !far(self.pts[j - 1][0], p[0]) || !far(p[0], self.pts[j][0]) {
            return None;
        }
        self.pts.copy_within(j..n, j + 1);
        self.pts[j] = p;
        self.len += 1;
        self.rebuild();
        Some(j)
    }

    /// Remove an interior point; endpoints are never removed. Rebuilds LUT.
    pub fn remove_point(&mut self, i: usize) -> bool {
        let n = self.len as usize;
        if i == 0 || i + 1 >= n {
            return false;
        }
        self.pts.copy_within(i + 1..n, i);
        self.len -= 1;
        self.rebuild();
        true
    }

    /// `pts` must already be sanitized (see [`Self::from_points`]).
    fn with_points(pts: &[[f32; 2]]) -> Self {
        debug_assert!((2..=MAX_CURVE_POINTS).contains(&pts.len()));
        let mut c = Self { pts: [[0.0; 2]; MAX_CURVE_POINTS], len: pts.len() as u8, lut: [0.0; CURVE_LUT_LEN + 1] };
        c.pts[..pts.len()].copy_from_slice(pts);
        c.rebuild();
        c
    }

    fn rebuild(&mut self) {
        let m = self.tangents();
        for i in 0..=CURVE_LUT_LEN {
            self.lut[i] = self.hermite(&m, i as f32 / CURVE_LUT_LEN as f32);
        }
    }

    /// PCHIP tangents (Fritsch–Butland interior, three-point limited ends).
    fn tangents(&self) -> [f64; MAX_CURVE_POINTS] {
        let p = self.points();
        let n = p.len();
        let mut h = [0.0f64; MAX_CURVE_POINTS];
        let mut d = [0.0f64; MAX_CURVE_POINTS];
        for k in 0..n - 1 {
            h[k] = (p[k + 1][0] - p[k][0]) as f64;
            d[k] = (p[k + 1][1] - p[k][1]) as f64 / h[k];
        }
        let mut m = [0.0f64; MAX_CURVE_POINTS];
        if n == 2 {
            m[0] = d[0];
            m[1] = d[0];
            return m;
        }
        for k in 1..n - 1 {
            m[k] = if d[k - 1] * d[k] <= 0.0 {
                0.0
            } else {
                let (w1, w2) = (2.0 * h[k] + h[k - 1], h[k] + 2.0 * h[k - 1]);
                (w1 + w2) / (w1 / d[k - 1] + w2 / d[k])
            };
        }
        m[0] = end_tangent(h[0], h[1], d[0], d[1]);
        m[n - 1] = end_tangent(h[n - 2], h[n - 3], d[n - 2], d[n - 3]);
        m
    }

    /// Cubic Hermite on the interval holding `x`, clamped to 0..=1.
    fn hermite(&self, m: &[f64; MAX_CURVE_POINTS], x: f32) -> f32 {
        let p = self.points();
        let x = if x > 0.0 { x.min(1.0) as f64 } else { 0.0 };
        let mut k = 0;
        while k + 2 < p.len() && p[k + 1][0] as f64 <= x {
            k += 1;
        }
        let (x0, y0, y1) = (p[k][0] as f64, p[k][1] as f64, p[k + 1][1] as f64);
        let h = p[k + 1][0] as f64 - x0;
        let t = (x - x0) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * h * m[k]
            + (3.0 * t2 - 2.0 * t3) * y1
            + (t3 - t2) * h * m[k + 1];
        (y as f32).clamp(0.0, 1.0)
    }
}

/// Three-point end tangent, limited so the end interval stays monotone.
fn end_tangent(h0: f64, h1: f64, d0: f64, d1: f64) -> f64 {
    let m = ((2.0 * h0 + h1) * d0 - h0 * d1) / (h0 + h1);
    if m * d0 <= 0.0 {
        0.0
    } else if d0 * d1 <= 0.0 && m.abs() > 3.0 * d0.abs() {
        3.0 * d0
    } else {
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift for the randomized checks.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    /// A sanitized curve with 2..=8 points; `monotone` makes y non-decreasing.
    fn random_curve(rng: &mut Rng, monotone: bool) -> PressureCurve {
        let n = 2 + (rng.next() * 7.0) as usize;
        let mut xs: Vec<f32> = (0..n).map(|_| rng.next()).collect();
        let mut ys: Vec<f32> = (0..n).map(|_| rng.next()).collect();
        xs.sort_by(f32::total_cmp);
        if monotone {
            ys.sort_by(f32::total_cmp);
        }
        let pts: Vec<[f32; 2]> = xs.iter().zip(&ys).map(|(&x, &y)| [x, y]).collect();
        let c = PressureCurve::from_points(&pts);
        // Sanitizing may drop points but keeps the y order of the survivors.
        if monotone {
            assert!(c.points().windows(2).all(|w| w[0][1] <= w[1][1]));
        }
        c
    }

    fn grid(n: usize) -> impl Iterator<Item = f32> {
        (0..=n).map(move |i| i as f32 / n as f32)
    }

    /// Steepest legal curves: a full step across one MIN_GAP interval.
    fn step_curves() -> Vec<PressureCurve> {
        vec![
            PressureCurve::from_points(&[[0.0, 0.0], [0.5, 0.0], [0.54, 1.0], [1.0, 1.0]]),
            PressureCurve::from_points(&[[0.0, 0.0], [0.04, 1.0], [1.0, 1.0]]),
            PressureCurve::from_points(&[[0.0, 0.0], [0.96, 0.0], [1.0, 1.0]]),
            PressureCurve::from_points(&[[0.0, 1.0], [0.3, 1.0], [0.34, 0.0], [1.0, 0.0]]),
            PressureCurve::from_points(&[[0.0, 0.0], [0.04, 1.0], [0.08, 0.0], [0.12, 1.0], [0.16, 0.0], [1.0, 1.0]]),
        ]
    }

    #[test]
    fn linear_is_identity() {
        let c = PressureCurve::linear();
        assert!(c.is_linear());
        for p in [0.0, 0.5, 1.0] {
            assert_eq!(c.eval(p), p);
            assert_eq!(c.eval_exact(p), p);
        }
        // Bit-exact over the whole range: the old gamma 1.0 path was `p.powf(1.0)`.
        for p in grid(4096) {
            assert_eq!(c.eval(p), p, "{p}");
        }
    }

    #[test]
    fn passes_through_points() {
        let mut rng = Rng(0x5eed);
        for _ in 0..200 {
            let c = random_curve(&mut rng, false);
            for &[x, y] in c.points() {
                assert_eq!(c.eval_exact(x), y, "{c:?} at {x}");
                assert!((c.eval(x) - y).abs() <= 2e-3, "{c:?} at {x}: {} vs {y}", c.eval(x));
            }
        }
    }

    #[test]
    fn monotone_points_give_monotone_curve() {
        let mut rng = Rng(0x00c0_ffee);
        for _ in 0..1000 {
            let c = random_curve(&mut rng, true);
            let (mut prev, mut prev_lut) = (0.0f32, 0.0f32);
            for p in grid(4096) {
                let (e, l) = (c.eval_exact(p), c.eval(p));
                assert!(e >= prev && l >= prev_lut, "{c:?} decreases at {p}");
                (prev, prev_lut) = (e, l);
            }
        }
    }

    #[test]
    fn no_overshoot_between_points() {
        let mut rng = Rng(0xfeed_beef);
        let mut curves: Vec<_> = (0..500).map(|i| random_curve(&mut rng, i % 2 == 0)).collect();
        curves.extend(step_curves());
        for c in curves {
            for w in c.points().windows(2) {
                let (lo, hi) = (w[0][1].min(w[1][1]), w[0][1].max(w[1][1]));
                for i in 0..=64 {
                    let x = w[0][0] + (w[1][0] - w[0][0]) * i as f32 / 64.0;
                    let y = c.eval_exact(x);
                    assert!(y >= lo && y <= hi, "{c:?} overshoots at {x}: {y} not in {lo}..={hi}");
                }
            }
        }
    }

    #[test]
    fn lut_matches_exact_within_2e_3() {
        let mut rng = Rng(0xabcdef);
        let mut curves: Vec<_> = (0..300).map(|i| random_curve(&mut rng, i % 3 != 0)).collect();
        curves.extend(step_curves());
        curves.push(PressureCurve::from_gamma(0.2));
        curves.push(PressureCurve::from_gamma(5.0));
        for c in curves {
            for p in grid(8192) {
                let (e, l) = (c.eval_exact(p), c.eval(p));
                assert!((e - l).abs() <= 2e-3, "{c:?} at {p}: exact {e} lut {l}");
            }
        }
    }

    #[test]
    fn eval_edges_and_nan() {
        let c = PressureCurve::from_points(&[[0.0, 0.1], [0.5, 0.3], [1.0, 0.9]]);
        for p in [0.0, -0.0, -1.0, f32::NAN, f32::NEG_INFINITY, -f32::MIN_POSITIVE] {
            assert_eq!(c.eval(p), 0.1, "{p}");
            assert_eq!(c.eval_exact(p), 0.1, "{p}");
        }
        for p in [1.0, 1.5, f32::INFINITY, f32::MAX] {
            assert_eq!(c.eval(p), 0.9, "{p}");
            assert_eq!(c.eval_exact(p), 0.9, "{p}");
        }
        // Just below 1 still interpolates into the last LUT interval.
        let p = 1.0 - f32::EPSILON;
        assert!((c.eval(p) - 0.9).abs() < 1e-4);
    }

    #[test]
    fn from_points_sanitizes() {
        // Unsorted, out of range, NaN/inf, duplicate x.
        let c = PressureCurve::from_points(&[
            [0.7, 0.8],
            [f32::NAN, 0.5],
            [0.2, f32::INFINITY],
            [-0.5, -1.0],
            [0.4, 0.3],
            [0.4, 0.9],
            [1.5, 2.0],
        ]);
        assert_eq!(c.points(), &[[0.0, 0.0], [0.4, 0.3], [0.7, 0.8], [1.0, 1.0]]);

        // Endpoints are pinned to x = 0 and x = 1.
        let c = PressureCurve::from_points(&[[0.3, 0.2], [0.6, 0.9]]);
        assert_eq!(c.points(), &[[0.0, 0.2], [1.0, 0.9]]);

        // Closer than MIN_GAP to the previous kept point or to the end: dropped.
        let c = PressureCurve::from_points(&[[0.0, 0.0], [0.03, 0.5], [0.5, 0.5], [0.52, 0.6], [0.98, 0.7], [1.0, 1.0]]);
        assert_eq!(c.points(), &[[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]);

        // More than 8: the first 7 and the last endpoint.
        let many: Vec<[f32; 2]> = (0..=20).map(|i| [i as f32 / 20.0, i as f32 / 20.0]).collect();
        let c = PressureCurve::from_points(&many);
        assert_eq!(c.points().len(), MAX_CURVE_POINTS);
        assert_eq!(c.points()[0], [0.0, 0.0]);
        assert_eq!(c.points()[7], [1.0, 1.0]);
        assert!(c.points().windows(2).all(|w| far(w[0][0], w[1][0])));

        // Fewer than 2 usable points: linear.
        assert_eq!(PressureCurve::from_points(&[]), PressureCurve::linear());
        assert_eq!(PressureCurve::from_points(&[[0.5, 0.5]]), PressureCurve::linear());
        assert_eq!(PressureCurve::from_points(&[[f32::NAN, 0.0], [0.5, f32::NAN]]), PressureCurve::linear());
    }

    #[test]
    fn set_point_constraints() {
        let mut c = PressureCurve::from_points(&[[0.0, 0.0], [0.3, 0.3], [0.6, 0.6], [1.0, 1.0]]);
        // Endpoints: x pinned, y clamped.
        assert!(c.set_point(0, [0.4, 0.2]));
        assert_eq!(c.points()[0], [0.0, 0.2]);
        assert!(!c.set_point(3, [0.2, 1.7]), "already at the clamped value");
        assert_eq!(c.points()[3], [1.0, 1.0]);
        assert!(c.set_point(3, [0.5, -3.0]));
        assert_eq!(c.points()[3], [1.0, 0.0]);
        // Interior: x clamped between the neighbours ± MIN_GAP, y to 0..=1.
        assert!(c.set_point(1, [0.9, 1.5]));
        assert_eq!(c.points()[1], [0.6 - MIN_GAP, 1.0]);
        assert!(c.set_point(1, [-1.0, 0.5]));
        assert_eq!(c.points()[1], [MIN_GAP, 0.5]);
        assert!(c.points().windows(2).all(|w| far(w[0][0], w[1][0])));
        // No change, bad index, non-finite: false, curve untouched.
        let before = c;
        assert!(!c.set_point(1, [MIN_GAP, 0.5]));
        assert!(!c.set_point(4, [0.5, 0.5]));
        assert!(!c.set_point(2, [f32::NAN, 0.5]));
        assert_eq!(c, before);
        // The LUT follows the edit.
        assert!((c.eval(MIN_GAP) - 0.5).abs() <= 2e-3);
    }

    #[test]
    fn insert_and_remove_rules() {
        let mut c = PressureCurve::linear();
        assert_eq!(c.insert_point([0.5, 0.2]), Some(1));
        assert_eq!(c.insert_point([0.25, 0.1]), Some(1));
        assert_eq!(c.points(), &[[0.0, 0.0], [0.25, 0.1], [0.5, 0.2], [1.0, 1.0]]);
        assert!((c.eval(0.5) - 0.2).abs() <= 2e-3, "LUT rebuilt on insert");
        // Within MIN_GAP of a neighbour (endpoints included): refused.
        assert_eq!(c.insert_point([0.52, 0.5]), None);
        assert_eq!(c.insert_point([0.0, 0.5]), None);
        assert_eq!(c.insert_point([0.99, 0.5]), None);
        assert_eq!(c.insert_point([1.0, 0.5]), None);
        assert_eq!(c.insert_point([f32::NAN, 0.5]), None);
        // Exactly MIN_GAP away is allowed.
        assert_eq!(c.insert_point([0.25 + MIN_GAP, 0.15]), Some(2));
        // Fill up to 8; the ninth is refused.
        for x in [0.7, 0.8, 0.9] {
            assert!(c.insert_point([x, 0.5]).is_some(), "{x}");
        }
        assert_eq!(c.points().len(), MAX_CURVE_POINTS);
        assert_eq!(c.insert_point([0.6, 0.5]), None, "full");
        assert!(c.points().windows(2).all(|w| w[0][0] < w[1][0]));
        // Endpoints are never removed; interior points are.
        let n = c.points().len();
        assert!(!c.remove_point(0));
        assert!(!c.remove_point(n - 1));
        assert!(!c.remove_point(n));
        let gone = c.points()[2];
        assert!(c.remove_point(2));
        assert_eq!(c.points().len(), n - 1);
        assert!(!c.points().contains(&gone));
        assert_eq!(c.points()[0][0], 0.0);
        assert_eq!(c.points()[n - 2][0], 1.0);
        while c.points().len() > 2 {
            assert!(c.remove_point(1));
        }
        assert!(!c.remove_point(1));
        // Back to two points: equal to (and evaluates as) the identity.
        assert_eq!(c, PressureCurve::linear());
        assert!(grid(1024).all(|p| c.eval(p) == p));
    }

    /// Largest |curve(x) − x^g| over 4000 steps of `lo..=hi`.
    fn gamma_error(g: f32, lo: f32, hi: f32) -> f32 {
        let c = PressureCurve::from_gamma(g);
        (0..=4000).map(|i| lo + (hi - lo) * i as f32 / 4000.0).map(|x| (c.eval_exact(x) - x.powf(g)).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn from_gamma_error_bounds() {
        assert!(PressureCurve::from_gamma(1.0).is_linear());
        assert!(PressureCurve::from_gamma(1.0005).is_linear());
        assert!(PressureCurve::from_gamma(f32::NAN).is_linear());
        let gammas = || (0..=27).map(|i| 0.3 + i as f32 * 0.1);
        for g in gammas() {
            if (g - 1.0).abs() > 1e-3 {
                assert_eq!(PressureCurve::from_gamma(g).points().len(), MAX_CURVE_POINTS, "{g}");
            }
            let tail = gamma_error(g, 0.04, 1.0);
            assert!(tail <= 0.008, "g = {g}: {tail} on [0.04, 1]");
            let whole = gamma_error(g, 0.0, 1.0);
            assert!(whole <= 0.15, "g = {g}: {whole} on [0, 1]");
            if g >= 1.0 {
                assert!(whole <= 0.003, "g = {g}: {whole} on [0, 1]");
            }
        }
        // The old setting was clamped to 0.2..=5; so is the migration.
        assert_eq!(PressureCurve::from_gamma(9.0), PressureCurve::from_gamma(5.0));
        assert_eq!(PressureCurve::from_gamma(0.01), PressureCurve::from_gamma(0.2));
    }

    #[test]
    fn serde_keeps_only_points() {
        let c = PressureCurve::from_points(&[[0.0, 0.1], [0.4, 0.2], [1.0, 0.95]]);
        let r: CurveRepr = c.into();
        assert_eq!(r.points, c.points());
        assert_eq!(PressureCurve::from(r), c);
    }
}
