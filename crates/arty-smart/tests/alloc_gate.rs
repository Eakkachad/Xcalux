//! G4 gate: every per-sample (stroke hot path) API in arty-smart must run
//! without touching the heap once its state / scratch exists.
//!
//! Construction (`ConformalIntervalCalibrator::new`, `PartitionScratch::
//! with_capacity`, …) may allocate; everything inside `count_allocs` below is
//! what F2/F3/F4/F6 would call per pen sample or at pen-up.

use arty_smart::conformal::{
    ConformalIntervalCalibrator, DecayUnit, PointForecaster, PredictiveInterval, ResidualMode,
    winkler_score,
};
use arty_smart::cumprodsum::{cumprodsum_reverse, cumprodsum_scalar};
use arty_smart::geom::{
    chord_cost_table_into, chord_max_deviation, circle_max_deviation, kasa_circle_fit,
    principal_tangent, turning_angle,
};
use arty_smart::kinematics::perception::{
    Eps, RegimeClassifier, RegimeConfig, RegimeSnapshot, ResidualConfig, ResidualMonitor,
    extrapolation_horizon, normal_two_sided_z, predictive_half_width_with_z,
};
use arty_smart::kinematics::{
    DividedDiff, KinState, Sched, kinematic_extrapolate_capped_into, kinematic_extrapolate_into,
};
use arty_smart::partition::{
    Block, PartitionScratch, SMatrix, make_block_costs_monotone, minmax_partition_into,
    minmax_partition_scratch_into,
};
use arty_smart::stats::{WelfordVariance, nearest_rank};
use arty_smart::temporal::{TemporalDerivativeKernel, sigmoid_surprise_gate};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

/// A synthetic pen path: a 240 Hz stroke with a curve and a hard corner,
/// slightly uneven timestamps (like Windows Ink bursts) and pressure.
fn pen_sample(i: usize) -> ([f32; 3], f64) {
    let t = i as f64 / 240.0 + if i.is_multiple_of(3) { 0.0007 } else { 0.0 };
    let s = i as f32;
    let (x, y) = if i < 120 {
        (100.0 + 2.0 * s, 300.0 + 40.0 * (s * 0.05).sin())
    } else {
        (340.0, 300.0 + 40.0 * (6.0f32).sin() + 2.5 * (s - 120.0))
    };
    let p = 0.4 + 0.3 * (s * 0.02).sin();
    ([x, y, p], t)
}

/// The F2 point forecaster seam: forecasts come from the caller's predictor.
struct LastPoint(f32);
impl PointForecaster for LastPoint {
    fn forecast_into(&mut self, _state: &[f32], _h: usize, out: &mut f32) {
        *out = self.0;
    }
}

#[test]
fn stroke_hot_path_is_allocation_free() {
    // ---- setup (may allocate) ----
    let mut ks = KinState::<3>::new(1.0 / 240.0).unwrap();
    let mut dd = DividedDiff::<3>::new();
    let mut clf = RegimeClassifier::new(RegimeConfig::default());
    let mut mon = ResidualMonitor::new(ResidualConfig::default());
    let mut cal = ConformalIntervalCalibrator::new(
        LastPoint(0.0),
        2,
        4,
        1,
        64, // small ring so the run below exercises eviction
        0.02,
        DecayUnit::Step,
        ResidualMode::HStep,
        false,
    );
    let mut kern = TemporalDerivativeKernel::<2>::new(0.3, 0.03);
    let mut welford = WelfordVariance::new();
    let z = normal_two_sided_z(0.05);
    let mut iv = PredictiveInterval::new(0.0, 0.0, 0.0, 0.05);
    let mut sink = 0.0f32;

    let n = arty_testkit::count_allocs(|| {
        let mut prev_vel = [0.0f32; 3];
        let mut running_acc = 0.0f32;
        let mut prev_t = 0.0f64;
        for i in 0..400 {
            let (pos, t) = pen_sample(i);

            // F2: predict one frame ahead on real timestamps, score it later.
            let mut pred = [0.0f32; 3];
            if dd.n_obs() > 0 {
                dd.extrapolate_into(t, 3, &mut pred).unwrap();
            }
            dd.observe_into(&pos, t).unwrap();
            let (mut v, mut a, mut j) = ([0.0f32; 3], [0.0f32; 3], [0.0f32; 3]);
            dd.derivatives_into(3, &mut v, &mut a, &mut j).unwrap();

            // Fixed-tick oracle path.
            let vel_before = ks.vel;
            let mut kpred = [0.0f32; 3];
            if ks.n_obs > 0 {
                kinematic_extrapolate_into(&ks, 1, &Sched::Measured, &mut kpred).unwrap();
                kinematic_extrapolate_capped_into(&ks, 4, &Sched::ZeroJerk, 2, &mut kpred).unwrap();
            }
            ks.observe_into(&pos, i as u32).unwrap();

            // Regime + surprise.
            let snap = RegimeSnapshot::from_state(&ks, &prev_vel, running_acc);
            let _regime = clf.classify(&snap);
            running_acc += 0.5 * (snap.acc_mag.min(10.0 * running_acc + 0.05) - running_acc);
            prev_vel = ks.vel;
            let _ev = mon.update(pred[0] - pos[0], &vel_before, &ks.vel, ks.dt);

            // Admission horizon + predictive and conformal half-widths.
            let eps = Eps::from_obs_noise(mon.eps_obs(1, 3).max(1e-3), ks.dt);
            let verdict = extrapolation_horizon(ks.dt, &eps, 1.0);
            sink += predictive_half_width_with_z(z, eps.pos, verdict.k_star.max(1), 3);
            for ch in 0..2 {
                cal.update_residual(pos[ch], pred[ch], ch, 1);
                cal.interval_from_point_into(pred[ch], ch, 1, 0.05, &mut iv);
                sink += winkler_score(&iv, pos[ch]);
            }
            cal.step();
            sink += f32::from(cal.coverage_violation(pos[0], 0, 2, 0.1));

            // F3: time-correct surprise on the velocity direction.
            let speed = (v[0] * v[0] + v[1] * v[1]).sqrt().max(1e-6);
            let dir = [v[0] / speed, v[1] / speed];
            let mut d = [0.0f32; 2];
            kern.observe_dt_into(&dir, (t - prev_t) as f32, 0.004, 0.04, &mut d);
            sink += sigmoid_surprise_gate(&d, 4.0);
            prev_t = t;

            welford.observe(pos[2]);
            sink += turning_angle([0.0, 0.0], [v[0], v[1]], [v[0] + a[0], v[1] + a[1]]);
        }
    });
    assert_eq!(n, 0, "per-sample path allocated {n} times");
    assert!(sink.is_finite());
    assert!(welford.variance().is_some());
}

#[test]
fn pen_up_paths_are_allocation_free_with_warm_scratch() {
    const N: usize = 128;
    // Resampled stroke + per-sample filter inputs, allocated up front.
    let pts: Vec<[f32; 2]> = (0..N)
        .map(|i| {
            let (p, _) = pen_sample(i * 3);
            [p[0], p[1]]
        })
        .collect();
    let alpha = vec![0.7f32; N];
    let xs: Vec<f32> = pts.iter().map(|p| 0.3 * p[0]).collect();
    let mut fwd = vec![0.0f32; N];
    let mut both = vec![0.0f32; N];
    let mut scratch = PartitionScratch::with_capacity(N);
    let mut blocks: Vec<Block> = Vec::with_capacity(N);
    let mut smat = SMatrix::from_fn(N, |_, _| 0.0);
    let mut sorted: Vec<f32> = pts.iter().map(|p| p[1]).collect();
    sorted.sort_unstable_by(f32::total_cmp);

    // Warm-up once so the scratch reaches its working size.
    let w = scratch.costs_mut(N);
    chord_cost_table_into(&pts, w);
    make_block_costs_monotone(w, N);
    minmax_partition_scratch_into(N, 0.5, &mut scratch, &mut blocks).unwrap();

    let mut sink = 0.0f32;
    let n = arty_testkit::count_allocs(|| {
        // F3 zero-phase pass.
        cumprodsum_scalar(&alpha, &xs, xs[0], &mut fwd);
        cumprodsum_reverse(&alpha, &fwd, fwd[N - 1], &mut both);
        sink += both[N / 2];

        // F4 stroke grammar: chord-cost DP.
        let w = scratch.costs_mut(N);
        chord_cost_table_into(&pts, w);
        make_block_costs_monotone(w, N);
        minmax_partition_scratch_into(N, 0.5, &mut scratch, &mut blocks).unwrap();
        for b in &blocks {
            let seg = &pts[b.start..b.end];
            sink += chord_max_deviation(seg);
            if let Some(c) = kasa_circle_fit(seg) {
                sink += circle_max_deviation(seg, &c);
            }
        }

        // F6-style pairwise DP with a reused matrix.
        smat.refill_with_fn(N, |i, j| (pts[i][1] - pts[j][1]).abs() * 0.01);
        minmax_partition_into(&smat, 0.3, &mut scratch, &mut blocks).unwrap();
        sink += blocks.len() as f32;

        // F1 endpoint tangent.
        if let Some(t) = principal_tangent(&pts[N - 16..]) {
            sink += t.straightness;
        }

        // HUD percentile.
        let (p99, _support) = nearest_rank(&sorted, 0.99);
        sink += p99;
    });
    assert_eq!(n, 0, "pen-up path allocated {n} times");
    assert!(sink.is_finite());
}
