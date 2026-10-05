//! Ported from katgpt-rs `crates/katgpt-core/src/cumprodsum.rs` (MIT, see
//! `NOTICE`). Changes: only the scalar and batched scans are ported (the
//! SIMD batched kernel, `segsum` and the SSM-specific helpers are dropped);
//! the scalar loop uses safe iterators instead of `get_unchecked`; added
//! [`cumprodsum_reverse`] for F3's zero-phase pass.
//!
//! # The `h = a·h + x` scan
//!
//! `out[t] = a[t]·out[t−1] + x[t]`, with `out[−1] = h_init`. Special cases:
//! `a ≡ 1` is a cumulative sum, `x ≡ 0` a cumulative product. With
//! `a[t] = 1 − α_t` and `x[t] = α_t·s[t]` it is an EMA with a per-sample
//! weight — exactly the time-correct stabilizer filter of plan F3, run over
//! a whole stroke at once (e.g. when `reshape_stroke` re-filters at pen-up).
//!
//! Zero allocation: outputs go to caller slices. O(T) time, O(1) space.
//! Each step is one fused multiply-add (`mul_add`), a single rounding.

/// Scalar scan: `out[t] = a[t]·h + x[t]; h = out[t]`, starting from
/// `h = h_init`.
///
/// Debug-asserts that `a`, `x` and `out` have equal length (release uses
/// the shortest).
#[inline]
pub fn cumprodsum_scalar(a: &[f32], x: &[f32], h_init: f32, out: &mut [f32]) {
    debug_assert_eq!(a.len(), x.len());
    debug_assert_eq!(a.len(), out.len());
    let mut h = h_init;
    for ((o, &at), &xt) in out.iter_mut().zip(a).zip(x) {
        h = at.mul_add(h, xt);
        *o = h;
    }
}

/// The same scan run backward in time: `out[t] = a[t]·out[t+1] + x[t]`,
/// starting from `h_init` past the end. Forward + reverse over the same
/// stroke gives a zero-phase (lag-free) smoothing pass (plan F3).
#[inline]
pub fn cumprodsum_reverse(a: &[f32], x: &[f32], h_init: f32, out: &mut [f32]) {
    debug_assert_eq!(a.len(), x.len());
    debug_assert_eq!(a.len(), out.len());
    let mut h = h_init;
    for ((o, &at), &xt) in out.iter_mut().zip(a).zip(x).rev() {
        h = at.mul_add(h, xt);
        *o = h;
    }
}

/// Batched scan over `n_channels` independent channels laid out `[ch][t]`
/// (`a[ch * seq_len + t]`), each with its own `h_init[ch]`.
#[inline]
pub fn cumprodsum_batched(
    a: &[f32],
    x: &[f32],
    h_init: &[f32],
    out: &mut [f32],
    n_channels: usize,
    seq_len: usize,
) {
    let total = n_channels * seq_len;
    debug_assert_eq!(a.len(), total);
    debug_assert_eq!(x.len(), total);
    debug_assert_eq!(out.len(), total);
    debug_assert_eq!(h_init.len(), n_channels);
    if seq_len == 0 {
        return;
    }
    for (ch, ((oc, ac), xc)) in out
        .chunks_exact_mut(seq_len)
        .zip(a.chunks_exact(seq_len))
        .zip(x.chunks_exact(seq_len))
        .enumerate()
    {
        cumprodsum_scalar(ac, xc, h_init[ch], oc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumprodsum_basic() {
        let a = [0.5, 0.5, 0.5, 0.5];
        let x = [1.0, 2.0, 3.0, 4.0];
        let mut out = [0.0f32; 4];
        cumprodsum_scalar(&a, &x, 0.0, &mut out);
        // 1, 2.5, 4.25, 6.125
        assert_eq!(out, [1.0, 2.5, 4.25, 6.125]);
    }

    #[test]
    fn cumprodsum_with_init() {
        let mut out = [0.0f32; 2];
        cumprodsum_scalar(&[0.9, 0.9], &[1.0, 1.0], 5.0, &mut out);
        assert!((out[0] - 5.5).abs() < 1e-5);
        assert!((out[1] - 5.95).abs() < 1e-5);
    }

    #[test]
    fn cumprodsum_cumsum_special_case() {
        let mut out = [0.0f32; 5];
        cumprodsum_scalar(&[1.0; 5], &[1.0, 2.0, 3.0, 4.0, 5.0], 0.0, &mut out);
        assert_eq!(out, [1.0, 3.0, 6.0, 10.0, 15.0]);
    }

    #[test]
    fn cumprodsum_cumprod_special_case() {
        let mut out = [0.0f32; 4];
        cumprodsum_scalar(&[0.5; 4], &[0.0; 4], 1.0, &mut out);
        assert_eq!(out, [0.5, 0.25, 0.125, 0.0625]);
    }

    #[test]
    fn cumprodsum_empty_and_single() {
        let mut empty: [f32; 0] = [];
        cumprodsum_scalar(&[], &[], 0.0, &mut empty);
        let mut out = [0.0f32; 1];
        cumprodsum_scalar(&[0.7], &[3.0], 1.0, &mut out);
        assert!((out[0] - 3.7).abs() < 1e-5);
    }

    #[test]
    fn cumprodsum_large_sequence() {
        let t = 1024;
        let a = vec![0.99; t];
        let x = vec![1.0; t];
        let mut out = vec![0.0; t];
        cumprodsum_scalar(&a, &x, 0.0, &mut out);
        for i in 0..t {
            assert!(out[i].is_finite());
            if i > 0 {
                assert!(out[i] > out[i - 1], "not increasing at {i}");
            }
        }
    }

    #[test]
    fn cumprodsum_batched_basic() {
        let a = [0.5, 0.5, 0.5, 0.9, 0.9, 0.9];
        let x = [1.0; 6];
        let mut out = [0.0; 6];
        cumprodsum_batched(&a, &x, &[0.0, 0.0], &mut out, 2, 3);
        let want = [1.0, 1.5, 1.75, 1.0, 1.9, 2.71];
        for (g, w) in out.iter().zip(want) {
            assert!((g - w).abs() < 1e-5);
        }
    }

    #[test]
    fn cumprodsum_batched_matches_scalar() {
        let (n_ch, t) = (4, 32);
        let a: Vec<f32> = (0..n_ch * t)
            .map(|i| 0.5 + 0.01 * (i as f32 % 10.0))
            .collect();
        let x: Vec<f32> = (0..n_ch * t).map(|i| (i as f32) * 0.1).collect();
        let h_init = vec![0.5; n_ch];
        let mut out_batched = vec![0.0; n_ch * t];
        cumprodsum_batched(&a, &x, &h_init, &mut out_batched, n_ch, t);
        for ch in 0..n_ch {
            let o = ch * t;
            let mut out_scalar = vec![0.0; t];
            cumprodsum_scalar(&a[o..o + t], &x[o..o + t], h_init[ch], &mut out_scalar);
            assert_eq!(&out_batched[o..o + t], &out_scalar[..]);
        }
    }

    #[test]
    fn reverse_is_forward_on_reversed_input() {
        let a = [0.9f32, 0.7, 0.5, 0.8, 0.6];
        let x = [1.0f32, -2.0, 0.5, 3.0, 0.25];
        let mut rev = [0.0f32; 5];
        cumprodsum_reverse(&a, &x, 0.25, &mut rev);
        let (mut ar, mut xr) = (a, x);
        ar.reverse();
        xr.reverse();
        let mut fwd = [0.0f32; 5];
        cumprodsum_scalar(&ar, &xr, 0.25, &mut fwd);
        fwd.reverse();
        assert_eq!(rev, fwd);
    }

    /// Forward then backward EMA over a symmetric bump keeps the peak where
    /// it was (zero phase), while the forward pass alone shifts it later.
    #[test]
    fn forward_backward_ema_is_zero_phase() {
        let n = 41;
        let s: Vec<f32> = (0..n)
            .map(|i| (-((i as f32 - 20.0) / 4.0).powi(2)).exp())
            .collect();
        let alpha = 0.3f32;
        let a = vec![1.0 - alpha; n];
        let xs: Vec<f32> = s.iter().map(|v| alpha * v).collect();
        let mut fwd = vec![0.0; n];
        cumprodsum_scalar(&a, &xs, 0.0, &mut fwd);
        let xb: Vec<f32> = fwd.iter().map(|v| alpha * v).collect();
        let mut both = vec![0.0; n];
        cumprodsum_reverse(&a, &xb, 0.0, &mut both);
        let argmax = |v: &[f32]| {
            v.iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
                .unwrap()
        };
        assert!(argmax(&fwd) > 20, "forward EMA lags");
        assert_eq!(argmax(&both), 20, "forward-backward has no lag");
    }
}
