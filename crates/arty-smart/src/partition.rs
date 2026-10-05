//! Ported from katgpt-rs `crates/katgpt-core/src/partition.rs` (MIT, see
//! `NOTICE`). Changes: the DP runs on caller-owned scratch
//! ([`PartitionScratch`], `*_into`) so repeated calls do not allocate; the
//! DP core is exposed over a precomputed block-cost table
//! ([`minmax_partition_costs_into`] + [`make_block_costs_monotone`]) for
//! F4's chord / arc costs; dropped the LLM-specific parts (layer-type
//! constrained partition, `forced_min_blocks`, `PairCosineAccum`);
//! `SMatrix::refill_with_fn` added for buffer reuse.
//!
//! # Min-max contiguous partition
//!
//! Split an ordered sequence `0..n` into the fewest contiguous blocks whose
//! worst intra-block discrepancy stays ≤ ε, breaking ties by the smallest
//! worst case:
//!
//! ```text
//! min_P (m, max_j W[s_j, e_j])   s.t.   W[s_j, e_j] ≤ ε
//! ```
//!
//! Plan F4 (Stroke Grammar): the sequence is the resampled stroke (n ≤ 256),
//! `W[i, j]` is how far points `i..=j` stray from the line (or circle)
//! through them, and the blocks are the straight / arc pieces. F6 uses the
//! same DP to split sketch guides into straight runs.
//!
//! Two inputs are supported:
//! - a symmetric pairwise matrix [`SMatrix`] (`W[i,j] = max S[p][q]` over
//!   `i ≤ p < q ≤ j`, built in O(n²)) — [`minmax_partition_into`];
//! - a block-cost table you computed yourself — [`minmax_partition_costs_into`].
//!   The DP requires costs that never shrink when a block grows; use
//!   [`make_block_costs_monotone`] to enforce that on raw fit errors.
//!
//! # Determinism
//!
//! Every comparison is a total-order f32 compare; argmin keeps the first
//! index on ties, so identical inputs give identical partitions.

/// Errors returned by the min-max partition primitives.
#[derive(Clone, Debug, PartialEq)]
pub enum PartitionError {
    /// `eps` was negative or non-finite (a NaN ε would read every block
    /// feasible).
    InvalidEps(f32),
    /// A block-cost table was not `n × n`.
    CostShape { len: usize, n: usize },
}

impl std::fmt::Display for PartitionError {
    #[cold]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEps(eps) => write!(f, "invalid ε {eps}: must be finite and >= 0"),
            Self::CostShape { len, n } => {
                write!(f, "block-cost table has {len} entries, expected {n}×{n}")
            }
        }
    }
}

impl std::error::Error for PartitionError {}

/// One contiguous block, half-open `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

impl Block {
    /// Number of elements in the block.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// A finalized n×n symmetric discrepancy matrix (row-major, diagonal 0).
/// Non-finite entries are refused at construction (a NaN would read
/// feasible inside the DP).
#[derive(Debug, Clone, Default)]
pub struct SMatrix {
    n: usize,
    data: Vec<f32>,
}

impl SMatrix {
    pub fn n(&self) -> usize {
        self.n
    }

    pub fn get(&self, i: usize, j: usize) -> f32 {
        self.data[i * self.n + j]
    }

    /// Build from `f(i, j)` for `i < j` (mirrored; diagonal forced to 0).
    /// Panics on a non-finite value.
    pub fn from_fn(n: usize, f: impl FnMut(usize, usize) -> f32) -> Self {
        let mut s = Self::default();
        s.refill_with_fn(n, f);
        s
    }

    /// [`from_fn`](Self::from_fn) into an existing matrix, reusing its
    /// buffer (no allocation when it already held ≥ n² entries).
    pub fn refill_with_fn(&mut self, n: usize, mut f: impl FnMut(usize, usize) -> f32) {
        self.n = n;
        self.data.clear();
        self.data.resize(n * n, 0.0);
        for i in 0..n {
            for j in (i + 1)..n {
                let v = f(i, j);
                assert!(v.is_finite(), "SMatrix: non-finite at ({i},{j})");
                self.data[i * n + j] = v;
                self.data[j * n + i] = v;
            }
        }
    }

    /// From a fully computed row-major mirrored matrix. Validates finiteness.
    pub fn from_parts(n: usize, data: Vec<f32>) -> Self {
        assert_eq!(
            data.len(),
            n * n,
            "SMatrix::from_parts: data must be n×n row-major"
        );
        assert!(
            data.iter().all(|v| v.is_finite()),
            "SMatrix::from_parts: non-finite distance — the DP requires a finite oracle"
        );
        Self { n, data }
    }

    /// Upper-triangle iteration (i < j).
    pub fn entries(&self) -> impl Iterator<Item = (usize, usize, f32)> + '_ {
        let n = self.n;
        (0..n).flat_map(move |i| ((i + 1)..n).map(move |j| (i, j, self.data[i * n + j])))
    }
}

/// Reusable DP scratch. Size it once (e.g. at brush `configure` for the
/// largest resampled stroke) and every later call with `n ≤ capacity` is
/// allocation-free.
#[derive(Debug, Clone, Default)]
pub struct PartitionScratch {
    /// Block-cost table `[i*n + j]`, block `i..=j`.
    worst: Vec<f32>,
    /// `count[j]`: minimum blocks covering `0..=j`.
    count: Vec<usize>,
    /// `worst_pref[j]`: minimal worst case among count-optimal partitions.
    worst_pref: Vec<f32>,
}

impl PartitionScratch {
    /// Scratch for sequences of up to `n_max` elements.
    pub fn with_capacity(n_max: usize) -> Self {
        Self {
            worst: Vec::with_capacity(n_max * n_max),
            count: Vec::with_capacity(n_max),
            worst_pref: Vec::with_capacity(n_max),
        }
    }

    /// The block-cost table buffer, resized to `n × n` (zeroed). Fill
    /// `[i*n + j]` (`j ≥ i`) with the cost of block `i..=j`, then call
    /// [`minmax_partition_scratch_into`].
    pub fn costs_mut(&mut self, n: usize) -> &mut [f32] {
        self.worst.clear();
        self.worst.resize(n * n, 0.0);
        &mut self.worst
    }

    fn prepare_dp(&mut self, n: usize) {
        self.count.clear();
        self.count.resize(n, usize::MAX);
        self.worst_pref.clear();
        self.worst_pref.resize(n, f32::INFINITY);
    }
}

/// Fill `w` (n×n, row-major) with `max S[q][p]` over `i ≤ p < q ≤ j` for
/// block `i..=j`. O(n²) via the descending-i column-prefix recurrence.
fn worst_table_into(s: &SMatrix, w: &mut [f32]) {
    let n = s.n();
    debug_assert_eq!(w.len(), n * n);
    w.fill(0.0);
    for i in (0..n).rev() {
        let mut run = 0.0f32;
        for j in (i + 1)..n {
            let v = s.get(j, i);
            run = if v > run { v } else { run };
            let below = if i + 1 < n { w[(i + 1) * n + j] } else { 0.0 };
            w[i * n + j] = if run > below { run } else { below };
        }
    }
}

/// Make a raw block-cost table monotone: afterwards no block costs less
/// than any sub-block it contains (`W[i,j] ≥ W[i+1,j]` and `≥ W[i,j−1]`).
/// The DP's early exit relies on this; fit errors (chord / circle
/// deviation) do not satisfy it on their own. O(n²), in place.
pub fn make_block_costs_monotone(w: &mut [f32], n: usize) {
    debug_assert_eq!(w.len(), n * n);
    // Process blocks by increasing length so both sub-blocks are final.
    for len in 2..=n {
        for i in 0..=(n - len) {
            let j = i + len - 1;
            let inner = w[(i + 1) * n + j].max(w[i * n + j - 1]);
            if inner > w[i * n + j] {
                w[i * n + j] = inner;
            }
        }
    }
}

/// Convenience: allocates its own scratch and output. Prefer
/// [`minmax_partition_into`] anywhere it runs more than once.
pub fn minmax_partition(s: &SMatrix, eps: f32) -> Result<Vec<Block>, PartitionError> {
    let mut scratch = PartitionScratch::with_capacity(s.n());
    let mut out = Vec::with_capacity(s.n());
    minmax_partition_into(s, eps, &mut scratch, &mut out)?;
    Ok(out)
}

/// The count-optimal, worst-case-minimal contiguous partition of `0..n`
/// under the pairwise matrix `s`, written to `out` (cleared first). No
/// allocation when `scratch` and `out` already have capacity for `n`.
pub fn minmax_partition_into(
    s: &SMatrix,
    eps: f32,
    scratch: &mut PartitionScratch,
    out: &mut Vec<Block>,
) -> Result<(), PartitionError> {
    validate_eps(eps)?;
    let n = s.n();
    let w = scratch.costs_mut(n);
    worst_table_into(s, w);
    partition_dp(n, eps, scratch, out);
    Ok(())
}

/// The DP over a caller-filled block-cost table `costs` (`n × n`,
/// `[i*n + j]` = cost of block `i..=j`, monotone — see
/// [`make_block_costs_monotone`]). Copies `costs` into the scratch; if you
/// built the table in [`PartitionScratch::costs_mut`] already, use
/// [`minmax_partition_scratch_into`] and skip the copy.
pub fn minmax_partition_costs_into(
    costs: &[f32],
    n: usize,
    eps: f32,
    scratch: &mut PartitionScratch,
    out: &mut Vec<Block>,
) -> Result<(), PartitionError> {
    validate_eps(eps)?;
    if costs.len() != n * n {
        return Err(PartitionError::CostShape {
            len: costs.len(),
            n,
        });
    }
    scratch.costs_mut(n).copy_from_slice(costs);
    partition_dp(n, eps, scratch, out);
    Ok(())
}

/// The DP over the table already in [`PartitionScratch::costs_mut`]`(n)`.
pub fn minmax_partition_scratch_into(
    n: usize,
    eps: f32,
    scratch: &mut PartitionScratch,
    out: &mut Vec<Block>,
) -> Result<(), PartitionError> {
    validate_eps(eps)?;
    if scratch.worst.len() != n * n {
        return Err(PartitionError::CostShape {
            len: scratch.worst.len(),
            n,
        });
    }
    partition_dp(n, eps, scratch, out);
    Ok(())
}

fn validate_eps(eps: f32) -> Result<(), PartitionError> {
    if !eps.is_finite() || eps < 0.0 {
        return Err(PartitionError::InvalidEps(eps));
    }
    Ok(())
}

/// The two-pass min-max DP over `scratch.worst`. Feasibility monotonicity
/// (a block only gets worse as it grows leftward) lets each candidate scan
/// stop at the first infeasible boundary.
fn partition_dp(n: usize, eps: f32, scratch: &mut PartitionScratch, out: &mut Vec<Block>) {
    out.clear();
    if n == 0 {
        return;
    }
    scratch.prepare_dp(n);
    let worst = &scratch.worst;
    let count = &mut scratch.count;
    let worst_pref = &mut scratch.worst_pref;

    const INF: usize = usize::MAX;
    // Pass 1 — count[j]: minimum blocks covering 0..=j.
    for j in 0..n {
        let mut i = j + 1;
        while i > 0 {
            i -= 1;
            if worst[i * n + j] > eps {
                break; // everything further left is worse
            }
            let prev = if i == 0 { 0 } else { count[i - 1] };
            if prev == INF {
                continue;
            }
            if prev + 1 < count[j] {
                count[j] = prev + 1;
            }
        }
    }
    debug_assert_ne!(count[n - 1], INF, "eps >= 0: singletons always feasible");

    // Pass 2 — worst_pref[j]: minimal worst case over count-optimal
    // partitions of 0..=j.
    for j in 0..n {
        let target = count[j] - 1;
        let mut i = j + 1;
        while i > 0 {
            i -= 1;
            if worst[i * n + j] > eps {
                break;
            }
            let prev_count = if i == 0 { 0 } else { count[i - 1] };
            if prev_count != target {
                continue;
            }
            let prev_worst = if i == 0 { 0.0 } else { worst_pref[i - 1] };
            let cand = prev_worst.max(worst[i * n + j]);
            if cand < worst_pref[j] {
                worst_pref[j] = cand;
            }
        }
    }

    // Reconstruct: walk boundaries backward, picking the first boundary
    // achieving the stored optimum (index-ordered argmin — deterministic).
    let mut j = n - 1;
    loop {
        let target = count[j] - 1;
        let target_worst = worst_pref[j];
        let mut i = j + 1;
        let mut chosen = 0usize;
        while i > 0 {
            i -= 1;
            if worst[i * n + j] > eps {
                break;
            }
            let prev_count = if i == 0 { 0 } else { count[i - 1] };
            if prev_count != target {
                continue;
            }
            let prev_worst = if i == 0 { 0.0 } else { worst_pref[i - 1] };
            if prev_worst.max(worst[i * n + j]) == target_worst {
                chosen = i;
                break;
            }
        }
        out.push(Block {
            start: chosen,
            end: j + 1,
        });
        if chosen == 0 {
            break;
        }
        j = chosen - 1;
    }
    out.reverse();
}

/// Worst intra-block discrepancy of a concrete partition under `s`.
pub fn partition_worst(s: &SMatrix, blocks: &[Block]) -> f32 {
    let mut worst = 0.0f32;
    for b in blocks {
        for p in b.start..b.end {
            for q in (p + 1)..b.end {
                worst = worst.max(s.get(q, p));
            }
        }
    }
    worst
}

/// Brute-force lexicographic optimum `(count, worst)` over all 2^(n−1)
/// contiguous partitions. Exponential — a test instrument only (n ≤ 14).
pub fn brute_force_optimal(s: &SMatrix, eps: f32) -> (usize, f32) {
    let n = s.n();
    assert!(n <= 14, "brute force is 2^(n-1) — test instrument only");
    let mut best: Option<(usize, f32)> = None;
    for mask in 0u32..(1u32 << (n - 1)) {
        let mut blocks: Vec<Block> = Vec::with_capacity(n);
        let mut start = 0usize;
        for c in 0..(n - 1) {
            if mask & (1 << c) != 0 {
                blocks.push(Block { start, end: c + 1 });
                start = c + 1;
            }
        }
        blocks.push(Block { start, end: n });
        let mut feasible = true;
        let mut worst = 0.0f32;
        'blk: for b in &blocks {
            for p in b.start..b.end {
                for q in (p + 1)..b.end {
                    let v = s.get(q, p);
                    if v > eps {
                        feasible = false;
                        break 'blk;
                    }
                    worst = worst.max(v);
                }
            }
        }
        if !feasible {
            continue;
        }
        let cand = (blocks.len(), worst);
        let better = match best {
            None => true,
            Some((bc, bw)) => bc > cand.0 || (bc == cand.0 && bw > cand.1),
        };
        if better {
            best = Some(cand);
        }
    }
    best.expect("eps >= 0: singleton partition always feasible")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s_of(n: usize, f: impl FnMut(usize, usize) -> f32) -> SMatrix {
        SMatrix::from_fn(n, f)
    }

    /// Deterministic LCG draws.
    fn lcg_draw(seed: u64) -> impl FnMut() -> f32 {
        let mut st = seed;
        move || {
            st = st
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (st >> 11) as f32 / (1u64 << 53) as f32
        }
    }

    #[test]
    fn dp_recovers_one_planted_block() {
        let s = s_of(6, |i, j| {
            let grp = |l: usize| matches!(l, 2..=4);
            if grp(i) && grp(j) { 0.0 } else { 1.0 }
        });
        let p = minmax_partition(&s, 0.1).unwrap();
        assert_eq!(
            p,
            vec![
                Block { start: 0, end: 1 },
                Block { start: 1, end: 2 },
                Block { start: 2, end: 5 },
                Block { start: 5, end: 6 },
            ]
        );
    }

    #[test]
    fn count_and_worst_match_brute_force_on_random_matrices() {
        let mut draw = lcg_draw(0x243F_6A88_85A3_08D3);
        for n in [2usize, 5, 8, 11] {
            let s = s_of(n, |i, j| if i == j { 0.0 } else { draw() * 1.5 });
            for eps in [0.05f32, 0.3, 0.7, 1.2] {
                let p = minmax_partition(&s, eps).unwrap();
                let got = (p.len(), partition_worst(&s, &p));
                assert_eq!(got, brute_force_optimal(&s, eps), "n={n} eps={eps}");
                for b in &p {
                    for x in b.start..b.end {
                        for y in (x + 1)..b.end {
                            assert!(s.get(y, x) <= eps, "constraint violated n={n} eps={eps}");
                        }
                    }
                }
            }
        }
    }

    const MONOTONE_EPS_GRID: [f32; 7] = [0.05, 0.10, 0.20, 0.30, 0.50, 0.80, 1.20];

    #[test]
    fn m_monotone_non_increasing_in_eps() {
        let mut draw = lcg_draw(0xDEAD_BEEF_CAFE_F00D);
        let s = s_of(12, |i, j| if i == j { 0.0 } else { draw() });
        let mut prev = usize::MAX;
        for eps in MONOTONE_EPS_GRID {
            let m = minmax_partition(&s, eps).unwrap().len();
            assert!(m <= prev, "m grew at eps={eps}");
            prev = m;
        }
    }

    #[test]
    fn invalid_eps_refused() {
        let s = s_of(3, |_, _| 0.5);
        for eps in [-0.1, f32::NAN, f32::INFINITY] {
            assert!(matches!(
                minmax_partition(&s, eps),
                Err(PartitionError::InvalidEps(_))
            ));
        }
    }

    #[test]
    fn empty_and_single() {
        assert!(
            minmax_partition(&SMatrix::from_fn(0, |_, _| 0.0), 0.1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            minmax_partition(&SMatrix::from_fn(1, |_, _| 0.0), 0.0).unwrap(),
            vec![Block { start: 0, end: 1 }]
        );
    }

    #[test]
    fn smatrix_from_parts_validates_finiteness() {
        assert!(
            std::panic::catch_unwind(|| {
                let mut data = vec![0.0f32; 9];
                data[4] = f32::NAN;
                SMatrix::from_parts(3, data)
            })
            .is_err()
        );
        let s = SMatrix::from_parts(2, vec![0.0, 0.5, 0.5, 0.0]);
        assert_eq!(s.entries().collect::<Vec<_>>(), vec![(0, 1, 0.5)]);
    }

    // ===== ARTY additions =====

    #[test]
    fn scratch_path_matches_allocating_path_and_reuses() {
        let mut draw = lcg_draw(0x0B0B_5EED_1D1E_CAFE);
        let mut scratch = PartitionScratch::with_capacity(16);
        let mut out = Vec::with_capacity(16);
        let mut s = SMatrix::default();
        for n in [16usize, 3, 11, 16] {
            s.refill_with_fn(n, |_, _| draw());
            for eps in [0.1f32, 0.5, 0.9] {
                minmax_partition_into(&s, eps, &mut scratch, &mut out).unwrap();
                assert_eq!(out, minmax_partition(&s, eps).unwrap(), "n={n} eps={eps}");
            }
        }
    }

    #[test]
    fn costs_path_matches_pairwise_path() {
        let mut draw = lcg_draw(0x1234_ABCD_5678_EF90);
        let n = 10;
        let s = s_of(n, |_, _| draw());
        let mut scratch = PartitionScratch::with_capacity(n);
        let mut costs = vec![0.0f32; n * n];
        worst_table_into(&s, &mut costs);
        let mut a = Vec::new();
        let mut b = Vec::new();
        for eps in MONOTONE_EPS_GRID {
            minmax_partition_into(&s, eps, &mut scratch, &mut a).unwrap();
            minmax_partition_costs_into(&costs, n, eps, &mut scratch, &mut b).unwrap();
            assert_eq!(a, b, "eps={eps}");
        }
        assert!(matches!(
            minmax_partition_costs_into(&costs[1..], n, 0.5, &mut scratch, &mut b),
            Err(PartitionError::CostShape { .. })
        ));
    }

    #[test]
    fn monotone_closure_dominates_sub_blocks() {
        let n = 6;
        let mut draw = lcg_draw(99);
        let mut w = vec![0.0f32; n * n];
        for i in 0..n {
            for j in (i + 1)..n {
                w[i * n + j] = draw();
            }
        }
        make_block_costs_monotone(&mut w, n);
        for i in 0..n {
            for j in (i + 1)..n {
                assert!(w[i * n + j] >= w[(i + 1) * n + j]);
                assert!(w[i * n + j] >= w[i * n + j - 1]);
            }
        }
    }

    /// F4 shape: an L-shaped polyline (two straight runs) splits into
    /// exactly two blocks under a chord-deviation cost.
    #[test]
    fn splits_an_l_shaped_stroke_at_the_corner() {
        let mut pts = Vec::new();
        for i in 0..=10 {
            pts.push([i as f32, 0.0]);
        }
        for i in 1..=10 {
            pts.push([10.0, i as f32]);
        }
        let n = pts.len();
        let mut scratch = PartitionScratch::with_capacity(n);
        let w = scratch.costs_mut(n);
        crate::geom::chord_cost_table_into(&pts, w);
        make_block_costs_monotone(w, n);
        let mut out = Vec::new();
        minmax_partition_scratch_into(n, 0.25, &mut scratch, &mut out).unwrap();
        assert_eq!(out.len(), 2, "{out:?}");
        // The corner point (10, 0) can belong to either run.
        assert!(out[0].end == 10 || out[0].end == 11, "{out:?}");
    }
}
