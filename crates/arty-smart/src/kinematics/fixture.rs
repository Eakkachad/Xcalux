//! Ported from katgpt-rs `crates/katgpt-core/src/kinematics/fixture.rs` (MIT,
//! see `NOTICE`). Test-only here. Changes: the extent channel and the
//! looming segment are gone (segment 4 is now a plain horizontal glide,
//! tagged `Uniform`), and with them the extent parameters.
//!
//! Deterministic piecewise-polynomial fixture generator: uniform /
//! constant-acceleration / bounce / glide / drag segments, with per-frame
//! regime tags, event ticks, and an ID/OOD speed range label.
//!
//! **Dyadic parameter grids:** every sampled coefficient is a dyadic
//! rational, so generated trajectories, their differences, and the
//! closed-form predictions are exactly representable in f32 — the gap-table
//! zeros are bit-exact.
//!
//! **Honest tagging:** regime tags are derived from the emitted sequence's
//! own properties, never from the generator's intent.

use crate::kinematics::perception::{DRAG_ACC_FLOOR, Regime};
use crate::kinematics::{KinState, Sched, kinematic_extrapolate_into};

/// Clip length per segment.
pub const T: usize = 31;

/// Segment count per fixture.
pub const N_SEGMENTS: usize = 5;

/// ID / OOD range label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeLabel {
    Id,
    Ood,
}

/// Sampled fixture parameters (dyadic grids).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhyParams {
    /// Initial speed (units/step).
    pub v: f32,
    /// Acceleration magnitude for the constant-acceleration segment.
    pub g: f32,
    /// Geometric drag ratio for the drag segment.
    pub rho: f32,
    /// Speed ratio for the bounce segment.
    pub e: f32,
}

/// One emitted sample.
#[derive(Clone, Copy, Debug)]
pub struct FixtureFrame {
    pub tick: u32,
    /// 2-D position.
    pub pos: [f32; 2],
}

/// A generated fixture: frames + per-frame tags + event ticks.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub params: PhyParams,
    pub frames: Vec<FixtureFrame>,
    /// Per-frame regime tag (same length as `frames`).
    pub tags: Vec<Regime>,
    /// Event ticks (the bounce tick).
    pub events: Vec<u32>,
    /// Segment start indices into `frames` (`N_SEGMENTS` segments of `T`).
    pub segments: Vec<usize>,
}

// ===== deterministic RNG =====

/// SplitMix64 — deterministic, seedable, zero-dep.
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform [0, 1).
    #[inline]
    pub fn next_unit(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) * (1.0_f32 / (1u64 << 24) as f32)
    }

    /// Uniform pick from a grid slice.
    #[inline]
    pub fn pick<'a>(&mut self, grid: &'a [f32]) -> &'a f32 {
        let idx = ((self.next_unit() * grid.len() as f32) as usize).min(grid.len() - 1);
        &grid[idx]
    }
}

const V_ID: [f32; 7] = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0];
const V_OOD: [f32; 8] = [0.0625, 0.125, 0.25, 0.5, 4.5, 5.0, 5.5, 6.0];

/// Sample parameters from the ID or OOD speed range (dyadic grids).
#[must_use]
pub fn sample_params(rng: &mut SplitMix64, label: RangeLabel) -> PhyParams {
    let v = match label {
        RangeLabel::Id => *rng.pick(&V_ID),
        RangeLabel::Ood => *rng.pick(&V_OOD),
    };
    PhyParams {
        v,
        g: 0.25,
        rho: 0.5,
        e: 0.5,
    }
}

/// Generate one fixture: 5 interleaved segments × `T` frames.
///
/// Segment order: Uniform → Parabolic → Bounce → Glide → Drag.
#[must_use]
pub fn generate(seed: u64, label: RangeLabel) -> Fixture {
    let mut rng = SplitMix64::new(seed);
    let p = sample_params(&mut rng, label);
    let mut frames: Vec<FixtureFrame> = Vec::with_capacity(N_SEGMENTS * T);
    let mut tags: Vec<Regime> = Vec::with_capacity(N_SEGMENTS * T);
    let mut events: Vec<u32> = Vec::new();
    let mut segments: Vec<usize> = Vec::with_capacity(N_SEGMENTS);

    // ── Segment 1: uniform motion ─────────────────────────────
    segments.push(frames.len());
    {
        let (vx, vy) = (p.v, p.v * 0.5);
        let (mut x, mut y) = (0.0f32, 0.0f32);
        for _ in 0..T {
            x += vx;
            y += vy;
            frames.push(FixtureFrame {
                tick: frames.len() as u32,
                pos: [x, y],
            });
            tags.push(Regime::Uniform);
        }
    }

    // ── Segment 2: constant downward acceleration g ─────
    segments.push(frames.len());
    {
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let (vx, mut vy) = (p.v, p.v * 0.5);
        for _ in 0..T {
            vy -= p.g; // semi-implicit: velocity first…
            x += vx;
            y += vy; // …then position — a quadratic in the step index
            frames.push(FixtureFrame {
                tick: frames.len() as u32,
                pos: [x, y],
            });
            tags.push(Regime::Parabolic { g: p.g });
        }
    }

    // ── Segment 3: bounce (impulse event mid-segment) ───────────
    segments.push(frames.len());
    let bounce_at = T / 2;
    {
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let (vx, mut vy) = (p.v, p.v * 0.25);
        for i in 0..T {
            if i == bounce_at {
                // Flip BEFORE this frame's update so the detection tick is
                // exactly `bounce_at` and the ratio reads e.
                vy = -vy * p.e;
                events.push(frames.len() as u32);
            }
            y += vy;
            x += vx;
            frames.push(FixtureFrame {
                tick: frames.len() as u32,
                pos: [x, y],
            });
            tags.push(if i == bounce_at {
                Regime::Impulse
            } else {
                Regime::Uniform
            });
        }
    }

    // ── Segment 4: horizontal glide (uniform) ──────────────
    segments.push(frames.len());
    {
        let (mut x, y) = (0.0f32, 0.0f32);
        for _ in 0..T {
            x += p.v;
            frames.push(FixtureFrame {
                tick: frames.len() as u32,
                pos: [x, y],
            });
            tags.push(Regime::Uniform);
        }
    }

    // ── Segment 5: drag (geometric deceleration, no reversal) ───────
    segments.push(frames.len());
    {
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let (mut vx, vy) = (p.v, p.v * 0.25);
        // Total Δv = a₀·ρ/(1−ρ) = a₀ at ρ=0.5 → a₀ = v/4 never reverses.
        let mut a = p.v * 0.25;
        for _ in 0..T {
            vx -= a;
            x += vx;
            y += vy;
            frames.push(FixtureFrame {
                tick: frames.len() as u32,
                pos: [x, y],
            });
            // Honest tag: Drag while the emitted x's second difference is
            // still decisively negative — above the shared resolution floor.
            let d2 = if frames.len() >= 3 {
                let n = frames.len();
                frames[n - 1].pos[0] - 2.0 * frames[n - 2].pos[0] + frames[n - 3].pos[0]
            } else {
                f32::INFINITY
            };
            tags.push(if d2 < -DRAG_ACC_FLOOR {
                Regime::Drag
            } else {
                Regime::Uniform
            });
            a *= p.rho;
        }
    }

    Fixture {
        params: p,
        frames,
        tags,
        events,
        segments,
    }
}

/// Max-abs extrapolation error (channels + anchors) per horizon in `ks` for
/// ONE segment, computed within the segment and across no event.
#[must_use]
pub fn extrapolation_errors(fix: &Fixture, seg: usize, ks: &[u32], sched: &Sched) -> Vec<f32> {
    let mut worst = vec![0.0f32; ks.len()];
    let seg_start = fix.segments[seg];
    let seg_end = if seg + 1 < fix.segments.len() {
        fix.segments[seg + 1]
    } else {
        fix.frames.len()
    };
    let seg_events: Vec<u32> = fix
        .events
        .iter()
        .filter(|&&e| (seg_start as u32..seg_end as u32).contains(&e))
        .copied()
        .collect();
    let mut state = KinState::<2>::new(1.0).expect("dt=1");
    let mut predicted = [0.0f32; 2];
    for i in seg_start..seg_end {
        let f = &fix.frames[i];
        state
            .observe_into(&f.pos, f.tick)
            .expect("fixture ticks are monotonic");
        if state.n_obs < 4 {
            continue;
        }
        // Drag arm: below the resolution floor the emitted positions have
        // frozen at their ULP grid while the closed form continues the exact
        // geometric series.
        if matches!(sched, Sched::GeometricDrag { .. }) {
            let acc_mag = state.acc[0].abs().max(state.acc[1].abs());
            if acc_mag <= DRAG_ACC_FLOOR {
                continue;
            }
        }
        for (ki, &k) in ks.iter().enumerate() {
            let target = i + k as usize;
            if target >= seg_end {
                continue;
            }
            // Exclude anchors whose 4-deep window spans the event, and
            // predictions crossing it.
            if seg_events.iter().any(|&e| {
                let e = e as usize;
                e + 3 > i && e <= target
            }) {
                continue;
            }
            kinematic_extrapolate_into(&state, k, sched, &mut predicted)
                .expect("in-lattice horizon");
            for (pi, fp) in predicted.iter().zip(fix.frames[target].pos.iter()) {
                worst[ki] = worst[ki].max((pi - fp).abs());
            }
        }
    }
    worst
}
