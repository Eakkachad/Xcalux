//! Ported from katgpt-rs `crates/katgpt-core/src/kinematics/mod.rs` (MIT, see
//! `NOTICE`). Changes: the coefficient lattice is computed inline instead of
//! in a `OnceLock<Vec<_>>` (no heap, even on first use); game/sync-boundary
//! prose replaced with ARTY usage notes; [`divided`] (real-timestamp
//! predictor) is ARTY-original.
//!
//! # Pen kinematics
//!
//! Finite-difference state estimation and closed-form extrapolation for a
//! stream of samples — in ARTY, pen (or stabilizer-output) `x, y, pressure`.
//! Used by F2 Predicted Ink (how far ahead can we draw the tip) and F3 the
//! Intent-Aware Stabilizer (is the pen turning a corner).
//!
//! | Operator | What it does |
//! |---|---|
//! | [`KinState::observe_into`] | finite-difference ladder on a uniform tick (order 0→3) |
//! | [`DividedDiff::observe_into`] | the same ladder on real, irregular timestamps (no resampling lag) |
//! | [`kinematic_extrapolate_into`] | O(1) closed-form k-step rollout (Newton backward form) |
//! | [`Sched`] | closed-form choices for the 3rd-order term |
//! | [`perception::RegimeClassifier`] | straight / constant-accel / decelerating / impulse, with hysteresis |
//! | [`perception::ResidualMonitor`] | prediction-residual surprise: z-score spike, CUSUM drift, impulse |
//! | [`perception::extrapolation_horizon`] | error-propagation bound B(k) and admission horizon k* |
//! | [`perception::predictive_half_width`] | i.i.d.-noise predictive interval half-width |
//!
//! # The math (and why it is exact)
//!
//! With observations `x[m-3..=m]` sampled `Δt` apart, form the **backward
//! differences** at the anchor (latest observation):
//!
//! ```text
//! ∇¹ = x[m] − x[m-1]
//! ∇² = x[m] − 2·x[m-1] + x[m-2]
//! ∇³ = x[m] − 3·x[m-1] + 3·x[m-2] − x[m-3]
//! ```
//!
//! and the **Newton backward (Gregory–Newton) closed form**
//!
//! ```text
//! x̂(k) = x[m] + C(k,1)·∇¹ + C(k+1,2)·∇² + C(k+2,3)·∇³
//! ```
//!
//! is the unique degree-≤3 polynomial through the window evaluated at
//! `m + k` — exactly, for any horizon, with zero loops.
//!
//! The state stores the differences scaled to physical units
//! (`vel = ∇¹/Δt`, `acc = ∇²/Δt²`, `jerk = ∇³/Δt³`); `acc` and `jerk` are the
//! physical acceleration / jerk exactly on quadratic / cubic motion, while
//! `vel` is the mean velocity over the last step (the backward difference
//! lags instantaneous velocity by half a step; use [`central_velocity`] for
//! an O(Δt²) unbiased instantaneous estimate).
//!
//! # Bit-identity with the step-by-step chain
//!
//! The O(1) closed form and the O(k) difference-engine chain
//! (`d2 += d3; d1 += d2; s += d1`, see [`reference_chain_extrapolate_into`])
//! evaluate the same polynomial through different float operation sequences.
//! On dyadic-representable trajectories every operation in both paths is
//! exact, so the two agree bit-for-bit; on arbitrary data they agree to a
//! few ULP.
//!
//! # f32 exactness horizon
//!
//! `C(k+2,3)` exceeds f32's 24-bit exact-integer range at `k ≥ 288`. Degree
//! ≤ 2 trajectories stay bit-exact through the full lattice. Pen prediction
//! only ever asks for a handful of ticks, far inside that range.
//!
//! # ARTY usage notes
//!
//! - Windows Ink delivers samples in bursts with jittery timestamps.
//!   [`KinState`] assumes a uniform tick, so either resample (and count the
//!   added lag) or use [`DividedDiff`], which works on real timestamps.
//!   [`KinState`] then serves as the test oracle on uniform input.
//! - Predicted positions are display-only. They must never be written to
//!   tiles or history (plan F2: "predicted ink disappears and is never
//!   saved").
//!
//! # Modelless
//!
//! Pure f32 arithmetic, zero heap, zero deps, `#[repr(C)]` POD state,
//! per-channel independent.

pub mod divided;
#[cfg(test)]
mod fixture;
pub mod perception;

#[cfg(test)]
mod tests;

pub use divided::DividedDiff;

/// Maximum extrapolation horizon supported by the coefficient lattice.
pub const K_MAX: usize = 1024;

/// Ladder cap: the observation budget that saturates the order ladder.
pub const MAX_LADDER_OBS: u8 = 4;

// ===== coefficient lattice =====

/// One lattice row: the three Newton-backward binomials at horizon k.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LatticeRow {
    /// C(k,1) = k
    pub(crate) b1: f32,
    /// C(k+1,2) = k(k+1)/2
    pub(crate) b2: f32,
    /// C(k+2,3) = k(k+1)(k+2)/6
    pub(crate) b3: f32,
}

/// Lattice row at horizon `k`, computed in f64 then rounded to f32 (the
/// best available rounding, and bit-identical to katgpt-rs's precomputed
/// table, which used the same expression). Callers clamp `k ≤ K_MAX`.
#[inline]
pub(crate) fn lattice_row(k: usize) -> LatticeRow {
    let kf = k as f64;
    LatticeRow {
        b1: kf as f32,
        b2: (kf * (kf + 1.0) * 0.5) as f32,
        b3: (kf * (kf + 1.0) * (kf + 2.0) / 6.0) as f32,
    }
}

/// Observation / extrapolation errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KinError {
    /// A position sample (or derived quantity) was NaN / infinite.
    NonFinite,
    /// The sample period was zero, negative, or non-finite.
    BadDt,
    /// Observation ticks (or timestamps) must be strictly increasing.
    NonMonotonicTick,
    /// Requested horizon exceeds [`K_MAX`].
    HorizonTooFar,
    /// Not enough observations for the requested operation.
    NotEnoughObs,
}

impl core::fmt::Display for KinError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KinError::NonFinite => write!(f, "non-finite sample"),
            KinError::BadDt => write!(f, "sample period must be finite and > 0"),
            KinError::NonMonotonicTick => write!(f, "ticks must be strictly increasing"),
            KinError::HorizonTooFar => write!(f, "horizon exceeds K_MAX"),
            KinError::NotEnoughObs => write!(f, "not enough observations"),
        }
    }
}

impl std::error::Error for KinError {}

/// Finite-difference kinematic state over `D` independent channels.
///
/// The anchor is the **latest observation**; `vel`/`acc`/`jerk` are the
/// backward differences scaled to physical units (see the module doc).
/// `#[repr(C)]` POD — `Copy`, zero heap, per-channel independent.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct KinState<const D: usize> {
    /// Anchor position = the latest observation (order-0 estimate).
    pub pos: [f32; D],
    /// `∇¹/Δt` — order-1 coefficient (mean velocity over the last step).
    pub vel: [f32; D],
    /// `∇²/Δt²` — order-2 coefficient (physical acceleration, exact on
    /// quadratics).
    pub acc: [f32; D],
    /// `∇³/Δt³` — order-3 coefficient (physical jerk, exact on cubics).
    /// Zero until `n_obs ≥ 4`.
    pub jerk: [f32; D],
    /// Tick of the anchor observation.
    pub tick: u32,
    /// Number of observations absorbed (ladder position; saturates at 4).
    pub n_obs: u8,
    /// Uniform sample period (set once at construction).
    pub dt: f32,
}

impl<const D: usize> KinState<D> {
    /// Construct an empty state with sample period `dt`.
    ///
    /// Screens: `dt` must be finite and > 0 ([`KinError::BadDt`]).
    pub fn new(dt: f32) -> Result<Self, KinError> {
        if !dt.is_finite() || dt <= 0.0 {
            return Err(KinError::BadDt);
        }
        Ok(Self {
            pos: [0.0; D],
            vel: [0.0; D],
            acc: [0.0; D],
            jerk: [0.0; D],
            tick: 0,
            n_obs: 0,
            dt,
        })
    }

    /// Forget all observations (keeps `dt`). Call at pen-down.
    pub fn reset(&mut self) {
        *self = Self {
            dt: self.dt,
            ..Self::new(1.0).expect("dt=1 is valid")
        };
    }

    /// Absorb one observation, advancing the finite-difference ladder.
    ///
    /// ```text
    /// vel_new  = (x − pos_old) / Δt
    /// acc_new  = (vel_new − vel_old) / Δt      (once n_obs ≥ 2)
    /// jerk_new = (acc_new − acc_old) / Δt      (once n_obs ≥ 3)
    /// ```
    ///
    /// Ladder: 1 obs → order 0, 2 → order 1, 3 → order 2, 4+ → order 3.
    ///
    /// Screens: non-finite samples refused; ticks must strictly increase
    /// (the stencil assumes uniform Δt — resample irregular streams first, or
    /// use [`DividedDiff`]).
    pub fn observe_into(&mut self, pos: &[f32; D], tick: u32) -> Result<(), KinError> {
        if pos.iter().any(|x| !x.is_finite()) {
            return Err(KinError::NonFinite);
        }
        if self.n_obs > 0 && tick <= self.tick {
            return Err(KinError::NonMonotonicTick);
        }
        let dt = self.dt;
        for ch in 0..D {
            let x = pos[ch];
            if self.n_obs == 0 {
                self.pos[ch] = x;
                continue;
            }
            let vel_new = (x - self.pos[ch]) / dt;
            if self.n_obs >= 2 {
                let acc_new = (vel_new - self.vel[ch]) / dt;
                if self.n_obs >= 3 {
                    let jerk_new = (acc_new - self.acc[ch]) / dt;
                    self.jerk[ch] = jerk_new;
                }
                self.acc[ch] = acc_new;
            }
            self.vel[ch] = vel_new;
            self.pos[ch] = x;
        }
        self.tick = tick;
        self.n_obs = self.n_obs.saturating_add(1).min(MAX_LADDER_OBS);
        Ok(())
    }

    /// Effective ladder order given an observation-noise scale `eps_obs`:
    /// drops velocity / acceleration / jerk terms that are statistically
    /// insignificant against the noise propagated into their difference
    /// estimators (√2·ε for ∇¹, √6·ε for ∇², √20·ε for ∇³; 2σ screen).
    ///
    /// Order 0/1/2/3 → keeps pos / +vel / +acc / +jerk. With `eps_obs = 0`
    /// this always returns the full ladder order.
    #[must_use]
    pub fn significant_order(&self, eps_obs: f32) -> u8 {
        let ladder = match self.n_obs {
            0 | 1 => 0,
            2 => 1,
            3 => 2,
            _ => 3,
        };
        if eps_obs <= 0.0 || !eps_obs.is_finite() {
            return ladder;
        }
        // Per-channel significant order; aggregate to the max so a single
        // genuinely-curving channel keeps its order.
        let mut order = 0u8;
        for ch in 0..D {
            let mut o = 0u8;
            if ladder >= 1 && self.vel[ch].abs() * self.dt > 2.0 * SQRT2 * eps_obs {
                o = 1;
            }
            if ladder >= 2 && self.acc[ch].abs() * self.dt * self.dt > 2.0 * SQRT6 * eps_obs {
                o = 2;
            }
            if ladder >= 3
                && self.jerk[ch].abs() * self.dt * self.dt * self.dt > 2.0 * SQRT20 * eps_obs
            {
                o = 3;
            }
            order = order.max(o);
        }
        order
    }

    /// A copy of this state with ladder orders above `max_order` zeroed (for
    /// noise-aware reduced-order extrapolation). `KinState` is `Copy`.
    #[must_use]
    pub fn capped(&self, max_order: u8) -> Self {
        let mut s = *self;
        if max_order < 1 {
            s.vel = [0.0; D];
        }
        if max_order < 2 {
            s.acc = [0.0; D];
        }
        if max_order < 3 {
            s.jerk = [0.0; D];
        }
        s
    }
}

/// √2 — i.i.d.-noise standard-deviation factor of the first backward
/// difference (variance factor 1+1 = 2).
pub const SQRT2: f32 = core::f32::consts::SQRT_2;

/// √6 — i.i.d.-noise standard-deviation factor of the second backward
/// difference (variance factor 1+4+1 = 6).
pub const SQRT6: f32 = 2.449_489_7;

/// √20 — i.i.d.-noise standard-deviation factor of the third backward
/// difference (variance factor 1+9+9+1 = 20).
pub const SQRT20: f32 = 4.472_136;

// ===== schedules =====

/// Closed-form choices for the third-order term of the rollout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sched {
    /// `j ≡ 0` — exact on degree ≤ 2 motion.
    ZeroJerk,
    /// Constant physical jerk `j` — exact on degree ≤ 3 motion when `j` is
    /// the true jerk.
    ConstJerk { j: f32 },
    /// Use the ladder's measured 3rd-order term — exact on degree ≤ 3 with
    /// ≥ 4 clean observations.
    Measured,
    /// `j_max·tanh(λ·|vel|)`: a saturating jerk evaluated once at the anchor
    /// (then a constant-jerk rollout).
    ClampedCorrection { j_max: f32, lambda: f32 },
    /// Geometric drag: `a_{n+1} = ρ·a_n`, ρ ∈ (0,1), with the chain order
    /// `v += a·Δt; s += v·Δt; a *= ρ` — a pen decelerating into a stop.
    GeometricDrag { rho: f32 },
}

impl Sched {
    /// The effective third-order coefficient (physical jerk units) this
    /// schedule contributes for one channel, or `None` for the drag family
    /// (not a jerk rollout — it has its own closed form).
    #[inline]
    fn jerk_for(&self, vel_ch: f32, measured_jerk: f32) -> Option<f32> {
        match *self {
            Self::ZeroJerk => Some(0.0),
            Self::ConstJerk { j } => Some(j),
            Self::Measured => Some(measured_jerk),
            Self::ClampedCorrection { j_max, lambda } => {
                Some(j_max * (lambda * vel_ch.abs()).tanh())
            }
            Self::GeometricDrag { .. } => None,
        }
    }
}

/// Terminal velocity under geometric drag (chain order `v += aΔt; s += vΔt;
/// a *= ρ`, state `acc` = the `a` of the last update):
/// `ṡ_∞ = ṡ₀ + Δt·acc·ρ/(1−ρ)`. `None` for ρ outside (0,1).
#[must_use]
pub fn terminal_velocity(vel: f32, acc: f32, dt: f32, rho: f32) -> Option<f32> {
    if !(0.0..1.0).contains(&rho) {
        return None;
    }
    Some(vel + dt * acc * rho / (1.0 - rho))
}

// ===== extrapolation =====

/// O(1) closed-form k-step kinematic rollout (Newton backward form).
///
/// ```text
/// out[ch] = pos + C(k,1)·Δt·vel + C(k+1,2)·Δt²·acc + C(k+2,3)·Δt³·j₃
/// ```
///
/// where `j₃` comes from `sched` (drag uses its own closed form). Requires
/// `k ≤ K_MAX` and `n_obs ≥ 1`. Accumulation order is `((pos + t1) + t2) +
/// t3`.
pub fn kinematic_extrapolate_into<const D: usize>(
    state: &KinState<D>,
    k: u32,
    sched: &Sched,
    out: &mut [f32; D],
) -> Result<(), KinError> {
    if state.n_obs == 0 {
        return Err(KinError::NotEnoughObs);
    }
    if k as usize > K_MAX {
        return Err(KinError::HorizonTooFar);
    }
    let row = lattice_row(k as usize);
    let dt = state.dt;
    let dt2 = dt * dt;
    let dt3 = dt2 * dt;
    for ch in 0..D {
        out[ch] = if let Sched::GeometricDrag { rho } = *sched {
            let g = drag_schedule_weight(row.b1, rho);
            state.pos[ch] + row.b1 * dt * state.vel[ch] + dt2 * state.acc[ch] * g
        } else {
            let j3 = sched.jerk_for(state.vel[ch], state.jerk[ch]).unwrap_or(0.0);
            let t1 = row.b1 * dt * state.vel[ch];
            let t2 = row.b2 * dt2 * state.acc[ch];
            let t3 = row.b3 * dt3 * j3;
            (state.pos[ch] + t1) + t2 + t3
        };
    }
    Ok(())
}

/// Noise-aware reduced-order variant: masks ladder orders above `max_order`
/// (see [`KinState::significant_order`]) then runs the full closed form.
pub fn kinematic_extrapolate_capped_into<const D: usize>(
    state: &KinState<D>,
    k: u32,
    sched: &Sched,
    max_order: u8,
    out: &mut [f32; D],
) -> Result<(), KinError> {
    kinematic_extrapolate_into(&state.capped(max_order), k, sched, out)
}

/// Geometric-drag position weight for the chain `v += aΔt; s += vΔt; a *= ρ`:
///
/// ```text
/// G(k,ρ) = [kρ − kρ² − ρ² + ρ^(k+2)] / (1−ρ)²
/// ```
///
/// `ŝ(k) = pos + kΔt·vel + Δt²·acc·G(k,ρ)` (k=1 → ρ, k=2 → 2ρ + ρ²).
#[inline]
fn drag_schedule_weight(b1: f32, rho: f32) -> f32 {
    let k = b1; // b1 == k exactly (integer-valued float)
    let omr = 1.0 - rho;
    (k * rho - k * rho * rho - rho * rho + rho.powf(k + 2.0)) / (omr * omr)
}

/// O(k) reference rollout — the difference-engine chain the closed form
/// replaces. Per step, top-down: `d2 += d3; d1 += d2; s += d1`. The drag arm
/// runs the drag recurrence directly. Public so tests (and the F2 offline
/// harness) can cross-check the O(1) form against the definition.
pub fn reference_chain_extrapolate_into<const D: usize>(
    state: &KinState<D>,
    k: u32,
    sched: &Sched,
    out: &mut [f32; D],
) -> Result<(), KinError> {
    if state.n_obs == 0 {
        return Err(KinError::NotEnoughObs);
    }
    let dt = state.dt;
    let dt2 = dt * dt;
    let dt3 = dt2 * dt;
    for ch in 0..D {
        if let Sched::GeometricDrag { rho } = *sched {
            // The state's `acc` is the a used in the LAST update, so every
            // future step first decays then applies.
            let mut s = state.pos[ch];
            let mut v = state.vel[ch];
            let mut a = state.acc[ch];
            for _ in 0..k {
                a *= rho;
                v += a * dt;
                s += v * dt;
            }
            out[ch] = s;
            continue;
        }
        // Differences in raw (unscaled) units.
        let mut s = state.pos[ch];
        let mut d1 = state.vel[ch] * dt;
        let mut d2 = state.acc[ch] * dt2;
        let d3 = sched.jerk_for(state.vel[ch], state.jerk[ch]).unwrap_or(0.0) * dt3;
        for _ in 0..k {
            d2 += d3;
            d1 += d2;
            s += d1;
        }
        out[ch] = s;
    }
    Ok(())
}

/// Instantaneous velocity via the central 3-point stencil:
/// `(x[m] − x[m−2]) / (2Δt)` — O(Δt²) unbiased, unlike the backward
/// difference (which lags half a step). The middle sample is kept in the
/// signature for window clarity.
#[must_use]
pub fn central_velocity(x_m: f32, _x_m1: f32, x_m2: f32, dt: f32) -> f32 {
    (x_m - x_m2) / (2.0 * dt)
}

/// Sum of squared observation weights of the k-step extrapolator at a given
/// ladder order — the i.i.d.-noise variance amplification factor.
///
/// The extrapolation is a fixed linear combination of the last ≤ 4
/// observations, so `Var(x̂) = σ² · wss(k, order)`. This is the propagation
/// kernel behind [`perception::extrapolation_horizon`]'s bound and the
/// predictive interval.
#[must_use]
pub fn extrapolation_weight_ss(k: u32, order: u8) -> f32 {
    let row = lattice_row(k.min(K_MAX as u32) as usize);
    match order {
        0 => 1.0,
        1 => {
            let w0 = 1.0 + row.b1;
            let w1 = -row.b1;
            w0 * w0 + w1 * w1
        }
        2 => {
            let w0 = 1.0 + row.b1 + row.b2;
            let w1 = -(row.b1 + 2.0 * row.b2);
            let w2 = row.b2;
            w0 * w0 + w1 * w1 + w2 * w2
        }
        _ => {
            let w0 = 1.0 + row.b1 + row.b2 + row.b3;
            let w1 = -(row.b1 + 2.0 * row.b2 + 3.0 * row.b3);
            let w2 = row.b2 + 3.0 * row.b3;
            let w3 = -row.b3;
            w0 * w0 + w1 * w1 + w2 * w2 + w3 * w3
        }
    }
}
