//! ARTY-original (not ported): small 2-D geometry the plan names for its
//! assists. katgpt-rs has no equivalents (the plan's §2.2 marks these as
//! "write ourselves").
//!
//! | Helper | Plan use |
//! |---|---|
//! | [`principal_tangent`] | F1 baseline: tangent at a line-art endpoint from PCA of the last 8–16 skeleton px |
//! | [`point_line_distance`], [`chord_max_deviation`], [`chord_cost_table_into`] | F4: how straight a stretch of stroke is; the block-cost table for [`crate::partition`] |
//! | [`kasa_circle_fit`], [`circle_max_deviation`] | F4: arc pieces |
//! | [`turning_angle`] | F4 corner vs G1 joint; F3 corner HUD |
//!
//! Points are `[x, y]` in any consistent unit (canvas px). Accumulation is
//! f64 where cancellation matters (covariances, the circle fit). Everything
//! is allocation-free.

/// Tangent line estimated by PCA over a run of points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tangent {
    /// Centroid of the points (a point on the line).
    pub origin: [f32; 2],
    /// Unit direction of the principal axis, oriented from the first input
    /// point toward the last (so for points ordered toward an endpoint it
    /// points outward, the way a gap-closing ray should go).
    pub dir: [f32; 2],
    /// `1 − λ_min/λ_max` in [0, 1]: 1 for a perfect line, 0 for an isotropic
    /// blob (no meaningful direction). F1 should only trust high values.
    pub straightness: f32,
}

/// Principal axis of `points` (2×2 covariance, closed-form eigenvector).
///
/// `None` for fewer than two points or when all points coincide.
#[must_use]
pub fn principal_tangent(points: &[[f32; 2]]) -> Option<Tangent> {
    let n = points.len();
    if n < 2 {
        return None;
    }
    let inv_n = 1.0 / n as f64;
    let (mut mx, mut my) = (0.0f64, 0.0f64);
    for p in points {
        mx += f64::from(p[0]);
        my += f64::from(p[1]);
    }
    mx *= inv_n;
    my *= inv_n;
    let (mut sxx, mut sxy, mut syy) = (0.0f64, 0.0f64, 0.0f64);
    for p in points {
        let dx = f64::from(p[0]) - mx;
        let dy = f64::from(p[1]) - my;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let tr = sxx + syy;
    if tr <= 0.0 {
        return None;
    }
    let disc = ((sxx - syy) * (sxx - syy) + 4.0 * sxy * sxy).sqrt();
    let l_max = 0.5 * (tr + disc);
    let l_min = 0.5 * (tr - disc);
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (mut dx, mut dy) = (theta.cos(), theta.sin());
    let first = points[0];
    let last = points[n - 1];
    let run_x = f64::from(last[0]) - f64::from(first[0]);
    let run_y = f64::from(last[1]) - f64::from(first[1]);
    if dx * run_x + dy * run_y < 0.0 {
        dx = -dx;
        dy = -dy;
    }
    Some(Tangent {
        origin: [mx as f32, my as f32],
        dir: [dx as f32, dy as f32],
        straightness: (1.0 - (l_min.max(0.0) / l_max)) as f32,
    })
}

/// Distance from `p` to the infinite line through `a` and `b` (to `a` when
/// `a == b`).
#[inline]
#[must_use]
pub fn point_line_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
    let (vx, vy) = (p[0] - a[0], p[1] - a[1]);
    let len = (ux * ux + uy * uy).sqrt();
    if len == 0.0 {
        return (vx * vx + vy * vy).sqrt();
    }
    (ux * vy - uy * vx).abs() / len
}

/// Largest distance of any point from the chord `points[0] → points[last]`
/// (0 for fewer than three points).
#[must_use]
pub fn chord_max_deviation(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let (a, b) = (points[0], points[n - 1]);
    points[1..n - 1]
        .iter()
        .map(|&p| point_line_distance(p, a, b))
        .fold(0.0f32, f32::max)
}

/// Fill the `n × n` block-cost table (`n = points.len()`, row-major) with
/// `out[i*n + j] = chord_max_deviation(&points[i..=j])` for `j ≥ i` (lower
/// triangle left 0). O(n³) — at n = 256 about 2.8 M distance evaluations,
/// inside F4's pen-up budget. Feed it to
/// [`crate::partition::make_block_costs_monotone`] before the DP.
pub fn chord_cost_table_into(points: &[[f32; 2]], out: &mut [f32]) {
    let n = points.len();
    assert_eq!(out.len(), n * n, "chord_cost_table_into: out must be n×n");
    out.fill(0.0);
    for i in 0..n {
        for j in (i + 2)..n {
            out[i * n + j] = chord_max_deviation(&points[i..=j]);
        }
    }
}

/// A circle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub center: [f32; 2],
    pub radius: f32,
}

/// Algebraic (Kåsa) least-squares circle fit, solved in centred
/// coordinates for stability.
///
/// Minimises `Σ ((x−a)² + (y−b)² − r²)²`. Exact on points that lie on a
/// circle; slightly biased toward smaller radii on short noisy arcs, which
/// is acceptable for F4's classify-then-snap use. `None` for fewer than
/// three points or (near-)collinear input — use the chord fit then.
#[must_use]
pub fn kasa_circle_fit(points: &[[f32; 2]]) -> Option<Circle> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let inv_n = 1.0 / n as f64;
    let (mut mx, mut my) = (0.0f64, 0.0f64);
    for p in points {
        mx += f64::from(p[0]);
        my += f64::from(p[1]);
    }
    mx *= inv_n;
    my *= inv_n;
    let (mut suu, mut suv, mut svv) = (0.0f64, 0.0f64, 0.0f64);
    let (mut suuu, mut svvv, mut suvv, mut svuu) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for p in points {
        let u = f64::from(p[0]) - mx;
        let v = f64::from(p[1]) - my;
        let (uu, vv) = (u * u, v * v);
        suu += uu;
        suv += u * v;
        svv += vv;
        suuu += uu * u;
        svvv += vv * v;
        suvv += u * vv;
        svuu += v * uu;
    }
    // [suu suv; suv svv]·[uc; vc] = ½·[suuu + suvv; svvv + svuu]
    let det = suu * svv - suv * suv;
    let scale = (suu + svv) * (suu + svv);
    if scale <= 0.0 || det.abs() <= 1e-12 * scale {
        return None;
    }
    let r1 = 0.5 * (suuu + suvv);
    let r2 = 0.5 * (svvv + svuu);
    let uc = (r1 * svv - r2 * suv) / det;
    let vc = (suu * r2 - suv * r1) / det;
    let r2c = uc * uc + vc * vc + (suu + svv) * inv_n;
    if !(r2c.is_finite() && r2c > 0.0) {
        return None;
    }
    Some(Circle {
        center: [(uc + mx) as f32, (vc + my) as f32],
        radius: r2c.sqrt() as f32,
    })
}

/// Largest radial distance of any point from `circle`.
#[must_use]
pub fn circle_max_deviation(points: &[[f32; 2]], circle: &Circle) -> f32 {
    points
        .iter()
        .map(|p| {
            let (dx, dy) = (p[0] - circle.center[0], p[1] - circle.center[1]);
            ((dx * dx + dy * dy).sqrt() - circle.radius).abs()
        })
        .fold(0.0f32, f32::max)
}

/// Signed turning angle at `b` going `a → b → c`, in radians (−π, π]:
/// positive = counter-clockwise (in y-up coordinates; clockwise on a y-down
/// canvas). 0 when either segment has zero length.
#[inline]
#[must_use]
pub fn turning_angle(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
    let (vx, vy) = (c[0] - b[0], c[1] - b[1]);
    if (ux == 0.0 && uy == 0.0) || (vx == 0.0 && vy == 0.0) {
        return 0.0;
    }
    (ux * vy - uy * vx).atan2(ux * vx + uy * vy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn tangent_of_a_line_points_toward_the_last_point() {
        let pts: Vec<[f32; 2]> = (0..12)
            .map(|i| [3.0 - i as f32, 2.0 + 0.5 * i as f32])
            .collect();
        let t = principal_tangent(&pts).unwrap();
        let norm = (1.0f32 + 0.25).sqrt();
        assert!(
            close(t.dir[0], -1.0 / norm, 1e-5) && close(t.dir[1], 0.5 / norm, 1e-5),
            "{t:?}"
        );
        assert!(close(t.straightness, 1.0, 1e-6));
        // Centroid lies on the line.
        assert!(point_line_distance(t.origin, pts[0], pts[11]) < 1e-4);
        // Reversed input flips the direction.
        let mut rev = pts.clone();
        rev.reverse();
        let r = principal_tangent(&rev).unwrap();
        assert!(close(r.dir[0], -t.dir[0], 1e-6) && close(r.dir[1], -t.dir[1], 1e-6));
    }

    #[test]
    fn tangent_degenerate_and_blob() {
        assert!(principal_tangent(&[]).is_none());
        assert!(principal_tangent(&[[1.0, 1.0]]).is_none());
        assert!(principal_tangent(&[[1.0, 1.0]; 5]).is_none());
        // Square corners: isotropic → straightness ~0.
        let blob = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(principal_tangent(&blob).unwrap().straightness < 1e-6);
        // A noisy line is still mostly straight.
        let noisy: Vec<[f32; 2]> = (0..16)
            .map(|i| [i as f32, if i % 2 == 0 { 0.2 } else { -0.2 }])
            .collect();
        let t = principal_tangent(&noisy).unwrap();
        assert!(t.straightness > 0.99 && t.dir[0] > 0.99, "{t:?}");
    }

    #[test]
    fn distances_and_chord_deviation() {
        assert!(close(
            point_line_distance([0.0, 3.0], [0.0, 0.0], [5.0, 0.0]),
            3.0,
            1e-6
        ));
        assert!(close(
            point_line_distance([3.0, 4.0], [0.0, 0.0], [0.0, 0.0]),
            5.0,
            1e-6
        ));
        assert_eq!(chord_max_deviation(&[[0.0, 0.0], [1.0, 1.0]]), 0.0);
        let arch = [[0.0, 0.0], [1.0, 0.5], [2.0, 1.0], [3.0, 0.5], [4.0, 0.0]];
        assert!(close(chord_max_deviation(&arch), 1.0, 1e-6));
    }

    #[test]
    fn chord_cost_table_matches_direct_evaluation() {
        let pts: Vec<[f32; 2]> = (0..9).map(|i| [i as f32, ((i * i) % 5) as f32]).collect();
        let n = pts.len();
        let mut w = vec![-1.0f32; n * n];
        chord_cost_table_into(&pts, &mut w);
        for i in 0..n {
            for j in 0..n {
                let want = if j > i {
                    chord_max_deviation(&pts[i..=j])
                } else {
                    0.0
                };
                assert_eq!(w[i * n + j], want, "({i},{j})");
            }
        }
    }

    #[test]
    fn kasa_recovers_an_exact_circle_from_a_short_arc() {
        let (cx, cy, r) = (120.0f32, -40.0f32, 35.0f32);
        let pts: Vec<[f32; 2]> = (0..20)
            .map(|i| {
                let a = 0.3 + i as f32 * 0.04; // ~45° of arc
                [cx + r * a.cos(), cy + r * a.sin()]
            })
            .collect();
        let c = kasa_circle_fit(&pts).unwrap();
        assert!(
            close(c.center[0], cx, 1e-2) && close(c.center[1], cy, 1e-2),
            "{c:?}"
        );
        assert!(close(c.radius, r, 1e-2), "{c:?}");
        assert!(circle_max_deviation(&pts, &c) < 1e-2);
    }

    #[test]
    fn kasa_rejects_collinear_and_tiny_input() {
        assert!(kasa_circle_fit(&[[0.0, 0.0], [1.0, 1.0]]).is_none());
        let line: Vec<[f32; 2]> = (0..10).map(|i| [i as f32, 2.0 * i as f32]).collect();
        assert!(kasa_circle_fit(&line).is_none());
    }

    #[test]
    fn turning_angle_signs_and_degenerates() {
        let (a, b) = ([0.0, 0.0], [1.0, 0.0]);
        assert!(close(turning_angle(a, b, [2.0, 0.0]), 0.0, 1e-7));
        assert!(close(turning_angle(a, b, [1.0, 1.0]), FRAC_PI_2, 1e-6));
        assert!(close(turning_angle(a, b, [1.0, -1.0]), -FRAC_PI_2, 1e-6));
        assert!(close(turning_angle(a, b, [0.0, 0.0]).abs(), PI, 1e-6));
        assert_eq!(turning_angle(a, a, [1.0, 1.0]), 0.0);
    }
}
