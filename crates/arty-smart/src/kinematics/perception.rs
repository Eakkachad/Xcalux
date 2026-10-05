//! Ported from katgpt-rs `crates/katgpt-core/src/kinematics/perception.rs`
//! (MIT, see `NOTICE`). Changes: dropped the game-specific operators
//! (looming / time-to-contact, the `Looming` regime and its extent channel,
//! two-body closest approach / intercept / elastic resolve); `sigmoid` is the
//! crate's exact form; prose rewritten for pen input.
//!
//! Three operator families over the kinematic state, all closed-form:
//!
//! 1. **Regimes** — [`regime_gates`] / [`RegimeClassifier`]: is the pen
//!    moving straight, under constant acceleration, decelerating, or did it
//!    just change direction abruptly? Sigmoid gates + hysteresis, so the label
//!    does not flicker sample to sample (F3 uses this to pick τ_t; F6 to
//!    release a ruler).
//! 2. **Surprise** — [`ResidualMonitor`]: one-step prediction residuals →
//!    z-score spike gate, CUSUM sustained-drift gate, impulse discriminator
//!    (F2 uses it to stop predicting at a corner).
//! 3. **Admission** — [`extrapolation_horizon`] / [`predictive_half_width`]:
//!    how many ticks ahead the prediction stays inside an error budget (F2's
//!    "≤ 1 screen-px" gate, combined with the conformal half-width).
//!
//! All gates use sigmoid, never softmax: each predicate is an independent
//! `[0, 1]` confidence, classified by priority + hysteresis.

use crate::kinematics::{
    K_MAX, KinState, SQRT2, SQRT6, SQRT20, extrapolation_weight_ss, lattice_row,
};
use crate::sigmoid;

// ===== regimes =====

/// Kinematic regime labels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Regime {
    /// Constant velocity (degree ≤ 1) — a straight, steady stroke.
    Uniform,
    /// Constant acceleration magnitude ≈ `g` (degree 2).
    Parabolic { g: f32 },
    /// Transient velocity discontinuity (a sharp corner or flick).
    Impulse,
    /// Acceleration opposing velocity and decaying — slowing into a stop.
    Drag,
}

/// Aggregated per-target statistics the predicates consume (the caller
/// computes the vector dot products once; the classifier stays scalar).
#[derive(Clone, Copy, Debug, Default)]
pub struct RegimeSnapshot {
    /// Sample period.
    pub dt: f32,
    /// ‖vel‖ at the anchor.
    pub speed: f32,
    /// ‖acc‖ at the anchor.
    pub acc_mag: f32,
    /// ‖jerk‖ at the anchor (fresh one-step estimate).
    pub jerk_mag: f32,
    /// `|Δvel|/Δt` this tick — the impulse statistic.
    pub dv_dt: f32,
    /// Running ‖acc‖ scale the impulse is judged against. Track it robustly
    /// (winsorize: update toward `min(acc, C·running + floor)`): a raw EMA
    /// absorbs transients and then masks a genuine impulse a few ticks later,
    /// while a hard exclusion of high-dv ticks starves the scale on sustained
    /// force onsets (both failure modes were found on the upstream fixtures).
    pub running_acc: f32,
    /// `acc·vel` — negative when acceleration opposes motion.
    pub acc_vel_dot: f32,
    /// `jerk·acc` — negative when |acc| is decaying.
    pub jerk_acc_dot: f32,
}

impl RegimeSnapshot {
    /// Build a snapshot from a kinematic state plus the caller-tracked
    /// previous velocity (for the impulse statistic) and the running-|acc|
    /// scale.
    pub fn from_state<const D: usize>(
        state: &KinState<D>,
        prev_vel: &[f32; D],
        running_acc: f32,
    ) -> Self {
        let mut speed2 = 0.0f32;
        let mut acc2 = 0.0f32;
        let mut jerk2 = 0.0f32;
        let mut dv2 = 0.0f32;
        let mut av_dot = 0.0f32;
        let mut ja_dot = 0.0f32;
        for ch in 0..D {
            speed2 += state.vel[ch] * state.vel[ch];
            acc2 += state.acc[ch] * state.acc[ch];
            jerk2 += state.jerk[ch] * state.jerk[ch];
            let dv = (state.vel[ch] - prev_vel[ch]) / state.dt;
            dv2 += dv * dv;
            av_dot += state.acc[ch] * state.vel[ch];
            ja_dot += state.jerk[ch] * state.acc[ch];
        }
        Self {
            dt: state.dt,
            speed: speed2.sqrt(),
            acc_mag: acc2.sqrt(),
            jerk_mag: jerk2.sqrt(),
            dv_dt: dv2.sqrt(),
            running_acc,
            acc_vel_dot: av_dot,
            jerk_acc_dot: ja_dot,
        }
    }
}

/// Predicate gates — one sigmoid confidence per regime, `[0, 1]`, independent.
#[derive(Clone, Copy, Debug, Default)]
pub struct RegimeGates {
    pub uniform: f32,
    pub parabolic: f32,
    pub impulse: f32,
    pub drag: f32,
}

/// Classifier tuning. `g` is in position-units per Δt².
#[derive(Clone, Copy, Debug)]
pub struct RegimeConfig {
    /// Expected constant-acceleration magnitude (pos/Δt²).
    pub g: f32,
    /// Gate value at which a regime is admitted.
    pub enter: f32,
    /// Gate value below which the current regime is released.
    pub exit: f32,
    /// Impulse: required multiple of the running |acc| for |Δv|/Δt.
    pub impulse_ratio: f32,
    /// Uniform: curvature (|a|·Δt/|v|) below which motion reads as straight.
    pub uniform_curv: f32,
}

impl Default for RegimeConfig {
    fn default() -> Self {
        Self {
            g: 1.0,
            enter: 0.6,
            exit: 0.3,
            impulse_ratio: 8.0,
            uniform_curv: 0.05,
        }
    }
}

/// Resolution floor for drag/parabolic predicates (2⁻¹⁰): below this the
/// deceleration is within a few ULPs of the accumulated position and the
/// finite differences flicker between 0 and ±ULP.
pub const DRAG_ACC_FLOOR: f32 = 9.765_625e-4; // 2^-10

/// Closed-form regime predicates: one sigmoid gate per regime.
///
/// Priority order used by the classifier: Impulse > Drag > Parabolic >
/// Uniform (transient events outrank sustained regimes).
#[must_use]
pub fn regime_gates(snap: &RegimeSnapshot, cfg: &RegimeConfig) -> RegimeGates {
    let speed_floor = (snap.speed.abs() + f32::EPSILON).max(1e-6);
    // Curvature per step: |a|·Δt/|v| (dimensionless).
    let curv = snap.acc_mag * snap.dt / speed_floor;
    // Jerk relative to the speed: |j|·Δt²/|v|.
    let j_rel = snap.jerk_mag * snap.dt * snap.dt / speed_floor;

    let uniform = sigmoid((cfg.uniform_curv - curv) / 0.02) * sigmoid((0.05 - j_rel) / 0.02);

    // Parabolic: |‖a‖ − g| small AND jerk small.
    let g_err = (snap.acc_mag - cfg.g).abs() / cfg.g.max(1e-6);
    let parabolic = sigmoid((0.3 - g_err) / 0.1) * sigmoid((0.05 - j_rel) / 0.02);

    // Impulse: |Δv|/Δt versus the running force scale.
    let force_scale = snap.running_acc.max(1e-6);
    let impulse = sigmoid((snap.dv_dt / force_scale - cfg.impulse_ratio) / 2.0);

    // Drag: acceleration opposing velocity AND |acc| decaying, above the
    // resolution floor. Zero guards are relative (product == 0), so a
    // decaying drag holds its gate until the FD acceleration is exactly 0.
    let av_norm = snap.acc_mag * snap.speed;
    let align = if av_norm > 0.0 {
        -snap.acc_vel_dot / av_norm
    } else {
        0.0
    }; // 1 = head-on opposing
    let ja_norm = snap.jerk_mag * snap.acc_mag;
    let decay = if ja_norm > 0.0 {
        -snap.jerk_acc_dot / ja_norm
    } else {
        0.0
    }; // 1 = decaying
    let drag = if snap.acc_mag > DRAG_ACC_FLOOR {
        sigmoid((align - 0.6) / 0.1) * sigmoid((decay - 0.2) / 0.1)
    } else {
        0.0
    };

    RegimeGates {
        uniform,
        parabolic,
        impulse,
        drag,
    }
}

/// Sigmoid-gated hysteresis regime classifier.
///
/// Switch rules (the anti-flip-flop contract):
/// - **Impulse bypasses hysteresis**: it is emitted for the firing tick
///   without touching the sticky state.
/// - `proposed` = the highest-priority regime whose gate ≥ `enter`
///   (priority: Drag > Parabolic > Uniform).
/// - Switch to `proposed` when it out-prioritizes the held regime, or when
///   the held regime's own gate has fallen below `exit`.
/// - Nothing proposed: hold through dips while the held gate ≥ `exit`; once
///   below, clear and fall back to `Uniform`.
#[derive(Clone, Copy, Debug)]
pub struct RegimeClassifier {
    cfg: RegimeConfig,
    held: Option<Regime>,
}

/// Regime precedence for switching (higher wins).
fn regime_priority(r: &Regime) -> u8 {
    match r {
        Regime::Impulse => 4,
        Regime::Drag => 3,
        Regime::Parabolic { .. } => 2,
        Regime::Uniform => 1,
    }
}

impl RegimeClassifier {
    /// New classifier with explicit config.
    #[must_use]
    pub fn new(cfg: RegimeConfig) -> Self {
        Self { cfg, held: None }
    }

    /// Forget the held regime (pen-down).
    pub fn reset(&mut self) {
        self.held = None;
    }

    /// Classify one tick; updates the hysteresis state.
    pub fn classify(&mut self, snap: &RegimeSnapshot) -> Regime {
        let gates = regime_gates(snap, &self.cfg);
        if gates.impulse >= self.cfg.enter {
            return Regime::Impulse;
        }
        let proposed = if gates.drag >= self.cfg.enter {
            Some(Regime::Drag)
        } else if gates.parabolic >= self.cfg.enter {
            Some(Regime::Parabolic { g: snap.acc_mag })
        } else if gates.uniform >= self.cfg.enter {
            Some(Regime::Uniform)
        } else {
            None
        };
        let held_gate = match self.held {
            Some(Regime::Drag) => gates.drag,
            Some(Regime::Parabolic { .. }) => gates.parabolic,
            Some(Regime::Uniform) => gates.uniform,
            Some(Regime::Impulse) | None => 0.0,
        };
        match (proposed, self.held) {
            (Some(p), Some(h)) => {
                if p == h || regime_priority(&p) > regime_priority(&h) || held_gate < self.cfg.exit
                {
                    self.held = Some(p);
                }
            }
            (Some(p), None) => self.held = Some(p),
            (None, _) => {
                if held_gate < self.cfg.exit {
                    self.held = None;
                }
            }
        }
        match self.held {
            Some(Regime::Parabolic { .. }) => Regime::Parabolic { g: snap.acc_mag },
            Some(r) => r,
            None => Regime::Uniform,
        }
    }
}

// ===== residual surprise =====

/// Surprise-event taxonomy for prediction residuals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventKind {
    /// Sudden velocity discontinuity (a corner). `axis`/`e` describe a sign
    /// flip on one channel if there was one (reversal + speed ratio).
    Impulse {
        /// Channel whose velocity flipped sign with the largest |Δv|.
        axis: Option<usize>,
        /// `|v_after|/|v_before|` on that channel (≤ 1).
        e: Option<f32>,
    },
    /// Sustained one-sided drift (CUSUM over the slack threshold).
    Drift {
        /// CUSUM statistic at the alarm.
        cusum: f32,
    },
    /// Isolated spike: z-score gate fired without drift or impulse.
    Spike {
        /// Standardized residual at the event.
        z: f32,
        /// Sigmoid gate value `[0, 1]`.
        gate: f32,
    },
}

/// Tuning for [`ResidualMonitor`].
#[derive(Clone, Copy, Debug)]
pub struct ResidualConfig {
    /// EMA decay for the residual mean/variance and the running |acc|.
    pub ema_beta: f32,
    /// z-score center of the spike gate.
    pub z_ref: f32,
    /// z-score sigmoid width.
    pub z_width: f32,
    /// CUSUM slack (in residual σ units).
    pub cusum_k: f32,
    /// CUSUM alarm threshold (in residual σ units).
    pub cusum_h: f32,
    /// Impulse ratio: |Δv|/Δt multiple over the running |acc|.
    pub impulse_ratio: f32,
    /// Warmup observations before any event can fire.
    pub warmup: u32,
}

impl Default for ResidualConfig {
    fn default() -> Self {
        Self {
            ema_beta: 0.05,
            z_ref: 3.0,
            z_width: 0.5,
            cusum_k: 0.5,
            cusum_h: 5.0,
            impulse_ratio: 8.0,
            warmup: 16,
        }
    }
}

/// One-step prediction-residual surprise monitor (scalar — one per channel,
/// or feed it a combined residual such as the tip distance).
///
/// Maintains the residual mean/variance (the noise model), a two-sided
/// CUSUM, and an EMA of the running force scale. Feed it
/// `observed − predicted` each tick plus the velocities before and after the
/// observation; it returns the first firing event, if any.
///
/// Cold start: `warmup` observations seed the statistics before any event
/// can fire. During warmup the mean is the running average (not a lagging
/// EMA) and the CUSUM resets at the warmup boundary, so the ladder-fill
/// transient does not read as drift.
#[derive(Clone, Copy, Debug)]
pub struct ResidualMonitor {
    cfg: ResidualConfig,
    mean_r: f32,
    var_r: f32,
    mean_acc: f32,
    cusum_pos: f32,
    cusum_neg: f32,
    warm_sum: f32,
    n: u32,
}

impl ResidualMonitor {
    /// New monitor with explicit config.
    #[must_use]
    pub fn new(cfg: ResidualConfig) -> Self {
        Self {
            cfg,
            mean_r: 0.0,
            var_r: 0.0,
            mean_acc: 0.0,
            cusum_pos: 0.0,
            cusum_neg: 0.0,
            warm_sum: 0.0,
            n: 0,
        }
    }

    /// Forget all statistics (keeps the config).
    pub fn reset(&mut self) {
        *self = Self::new(self.cfg);
    }

    /// Current residual-σ estimate (sqrt of the EMA variance).
    #[must_use]
    pub fn sigma(&self) -> f32 {
        self.var_r.max(0.0).sqrt()
    }

    /// Current observation-noise estimate: the residual σ deconvolved by the
    /// k-step predictive amplification `√(wss(k, order) + 1)` — the σ̂_obs
    /// for [`extrapolation_horizon`]'s bound and the predictive interval.
    #[must_use]
    pub fn eps_obs(&self, k: u32, order: u8) -> f32 {
        let amp = (extrapolation_weight_ss(k, order) + 1.0).sqrt().max(1.0);
        self.sigma() / amp
    }

    /// Absorb one tick. Returns the firing event (if any).
    pub fn update<const D: usize>(
        &mut self,
        residual: f32,
        vel_before: &[f32; D],
        vel_after: &[f32; D],
        dt: f32,
    ) -> Option<EventKind> {
        let event = if self.n >= self.cfg.warmup && residual.is_finite() {
            let sig = self.sigma().max(1e-9);
            let z = ((residual - self.mean_r).abs()) / sig;
            let gate = sigmoid((z - self.cfg.z_ref) / self.cfg.z_width);

            // CUSUM (two-sided, slack k·σ).
            let dev = residual - self.mean_r;
            let slack = self.cfg.cusum_k * sig;
            self.cusum_pos = (self.cusum_pos + dev - slack).max(0.0);
            self.cusum_neg = (self.cusum_neg + dev + slack).min(0.0);

            // Impulse discriminator: |Δv|/Δt vs the running force scale.
            let dv_dt = dv_norm(vel_before, vel_after, dt);
            let force_scale = self.mean_acc.max(1e-6);
            if dv_dt > self.cfg.impulse_ratio * force_scale {
                Some(match impulse_report(vel_before, vel_after) {
                    ImpulseRaw::Flip { axis, e } => EventKind::Impulse {
                        axis: Some(axis),
                        e: Some(e),
                    },
                    ImpulseRaw::Free => EventKind::Impulse {
                        axis: None,
                        e: None,
                    },
                })
            } else if self.cusum_pos > self.cfg.cusum_h * sig
                || -self.cusum_neg > self.cfg.cusum_h * sig
            {
                Some(EventKind::Drift {
                    cusum: self.cusum_pos.max(-self.cusum_neg),
                })
            } else if gate >= 0.5 {
                Some(EventKind::Spike { z, gate })
            } else {
                None
            }
        } else {
            None
        };

        // Statistics update AFTER the decision (a residual must not deflate
        // its own z-score).
        let b = self.cfg.ema_beta;
        if self.n < self.cfg.warmup {
            self.warm_sum += residual;
            self.mean_r = self.warm_sum / (self.n + 1) as f32;
        } else {
            self.mean_r += b * (residual - self.mean_r);
            if self.n == self.cfg.warmup {
                self.cusum_pos = 0.0;
                self.cusum_neg = 0.0;
            }
        }
        let dm = residual - self.mean_r;
        self.var_r += b * (dm * dm - self.var_r);
        let a_mag = dv_norm(vel_before, vel_after, dt); // |Δv|/Δt ≈ |acc| this tick
        self.mean_acc += b * (a_mag - self.mean_acc);
        self.n = self.n.saturating_add(1);
        event
    }
}

#[inline]
fn dv_norm<const D: usize>(vel_before: &[f32; D], vel_after: &[f32; D], dt: f32) -> f32 {
    let mut dv2 = 0.0f32;
    for ch in 0..D {
        let dv = (vel_after[ch] - vel_before[ch]) / dt;
        dv2 += dv * dv;
    }
    dv2.sqrt()
}

/// Internal impulse classification.
enum ImpulseRaw {
    Flip {
        axis: usize,
        e: f32,
    },
    /// No sign flip anywhere — a free change of direction.
    Free,
}

/// The channel with the largest |Δv| among sign-flipping channels, and the
/// speed ratio there.
fn impulse_report<const D: usize>(vel_before: &[f32; D], vel_after: &[f32; D]) -> ImpulseRaw {
    let mut best_axis = None;
    let mut best_dv = 0.0f32;
    for (ch, (&vb, &va)) in vel_before.iter().zip(vel_after.iter()).enumerate() {
        let dv = (va - vb).abs();
        if vb * va < 0.0 && dv > best_dv {
            best_dv = dv;
            best_axis = Some(ch);
        }
    }
    match best_axis {
        Some(axis) => ImpulseRaw::Flip {
            axis,
            e: (vel_after[axis].abs() / vel_before[axis].abs()).min(1.0),
        },
        None => ImpulseRaw::Free,
    }
}

// ===== extrapolation horizon (UQ-bearing) =====

/// Independent uncertainty scales of the four state coefficients.
///
/// The bound composes them with the triangle inequality (worst case); for
/// the i.i.d. observation-noise model use [`Eps::from_obs_noise`].
///
/// Upstream verdict (katgpt-rs bench 680): the bound's k* *ordering* is the
/// claim, not calibrated coverage. On curving motion it beats the
/// conformal-naive floor decisively; on straight motion at h=1 it loses
/// ~1.08× to it. F2 therefore combines it with the conformal half-width
/// rather than trusting either alone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Eps {
    /// Anchor position uncertainty ε_p.
    pub pos: f32,
    /// Velocity-coefficient uncertainty ε_v.
    pub vel: f32,
    /// Acceleration-coefficient uncertainty ε_a.
    pub acc: f32,
    /// Jerk-coefficient uncertainty ε_j.
    pub jerk: f32,
}

impl Eps {
    /// From a single i.i.d. observation-noise scale: ε_p = ε, ε_v = √2·ε/Δt,
    /// ε_a = √6·ε/Δt², ε_j = √20·ε/Δt³.
    #[must_use]
    pub fn from_obs_noise(eps_obs: f32, dt: f32) -> Self {
        Self {
            pos: eps_obs,
            vel: SQRT2 * eps_obs / dt,
            acc: SQRT6 * eps_obs / (dt * dt),
            jerk: SQRT20 * eps_obs / (dt * dt * dt),
        }
    }
}

/// Error-propagation bound at horizon k:
///
/// ```text
/// B(k) = ε_p + C(k,1)·Δt·ε_v + C(k+1,2)·Δt²·ε_a + C(k+2,3)·Δt³·ε_j
/// ```
///
/// Monotone in k.
#[must_use]
pub fn horizon_bound(k: u32, dt: f32, eps: &Eps) -> f32 {
    let row = lattice_row((k as usize).min(K_MAX));
    eps.pos + row.b1 * dt * eps.vel + row.b2 * dt * dt * eps.acc + row.b3 * dt * dt * dt * eps.jerk
}

/// Admission-horizon verdict: the largest k whose bound stays under `thr`,
/// plus a sigmoid confidence from the remaining margin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizonVerdict {
    /// Largest admitted horizon (0 = not even the anchor is within `thr`).
    pub k_star: u32,
    /// `sigmoid((thr − B(k*)) / (0.25·thr))` — margin confidence, `[0, 1]`.
    pub conf: f32,
    /// `B(k*)` — the bound at the admitted horizon.
    pub bound: f32,
}

/// The admission gate: scan for the largest k with `B(k) ≤ thr` (the bound
/// is monotone in k, so the scan is exact). The scan stops at the first
/// failing k, so for pen-scale thresholds it is a few iterations.
#[must_use]
pub fn extrapolation_horizon(dt: f32, eps: &Eps, thr: f32) -> HorizonVerdict {
    let mut k_star = 0u32;
    let mut bound = horizon_bound(0, dt, eps);
    if bound > thr {
        return HorizonVerdict {
            k_star: 0,
            conf: sigmoid((thr - bound) / (0.25 * thr.abs() + 1e-9)),
            bound,
        };
    }
    for k in 1..=(K_MAX as u32) {
        let b = horizon_bound(k, dt, eps);
        if b > thr {
            break;
        }
        k_star = k;
        bound = b;
    }
    HorizonVerdict {
        k_star,
        conf: sigmoid((thr - bound) / (0.25 * thr.abs() + 1e-9)),
        bound,
    }
}

/// State-based convenience wrapper: derives the ε scales from an
/// observation-noise estimate and the state's `dt`.
#[must_use]
pub fn extrapolation_horizon_for_state<const D: usize>(
    state: &KinState<D>,
    eps_obs: f32,
    thr: f32,
) -> HorizonVerdict {
    extrapolation_horizon(state.dt, &Eps::from_obs_noise(eps_obs, state.dt), thr)
}

// ===== UQ interval construction =====

/// `erf(x)` via Abramowitz & Stegun 7.1.26 (max |error| 1.5e-7).
#[allow(clippy::excessive_precision)] // verbatim A&S 7.1.26 coefficients
#[must_use]
fn erf_as261(x: f32) -> f32 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Standard normal CDF `Φ(z) = 0.5·(1 + erf(z/√2))`.
#[must_use]
fn normal_cdf(z: f32) -> f32 {
    0.5 * (1.0 + erf_as261(z * std::f32::consts::FRAC_1_SQRT_2))
}

/// Two-sided normal quantile `z_{1−α/2}` — bisection on the normal CDF
/// (50 iterations over [−9, 9]; deterministic, accurate to the erf's 1.5e-7).
#[must_use]
pub fn normal_two_sided_z(alpha: f32) -> f32 {
    debug_assert!((0.0..=0.5).contains(&alpha), "alpha in [0, 0.5]");
    let target = 1.0 - (alpha * 0.5).clamp(1e-9, 0.5);
    let (mut lo, mut hi) = (-9.0f32, 9.0f32);
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if normal_cdf(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Predictive half-width for the k-step extrapolation at a given ladder
/// order: `z_{1−α/2} · σ_obs · √(wss(k, order) + 1)` — the i.i.d.
/// propagation of observation noise through the extrapolator's weights plus
/// the new observation's own noise (the +1; without it the interval
/// under-covers). [`horizon_bound`] remains the conservative triangle form.
///
/// `normal_two_sided_z` costs 50 bisection steps; on the hot path compute
/// `z` once per α and use [`predictive_half_width_with_z`].
#[must_use]
pub fn predictive_half_width(eps_obs: f32, k: u32, order: u8, alpha: f32) -> f32 {
    predictive_half_width_with_z(normal_two_sided_z(alpha), eps_obs, k, order)
}

/// [`predictive_half_width`] with a precomputed `z = normal_two_sided_z(α)`
/// (ARTY addition: keeps the per-sample cost to a square root).
#[inline]
#[must_use]
pub fn predictive_half_width_with_z(z: f32, eps_obs: f32, k: u32, order: u8) -> f32 {
    z * eps_obs * (extrapolation_weight_ss(k, order) + 1.0).sqrt()
}
