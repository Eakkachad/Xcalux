//! Stroke shaping: taper, post correction, clipped replay (TRACK STROKE).
//!
//! Entry taper is applied live by the [`crate::StrokeEngine`]. Exit taper and
//! post correction (CSP "Starting and ending", SAI/CSP "post correction")
//! need the whole stroke, so the engine logs every stabilized sample and, at
//! pen-up, replays the log from the stroke's initial brush state. Everything
//! here is pure and allocation-free once its buffers are warm.

use arty_core::{TILE_SIZE, TileCoord};

/// Highest post correction level.
pub const MAX_CORRECTION: u8 = 10;
/// Gaussian sigma in *screen* px per level (CSP "adjust by display"); tune by eye.
const CORRECTION_SIGMA: [f32; 11] = [0.0, 0.75, 1.5, 2.25, 3.0, 4.0, 5.0, 6.5, 8.0, 10.0, 12.0];
/// Samples one stroke may log (32 B each = 2 MiB); longer strokes stay as drawn.
pub(crate) const MAX_LOGGED_SAMPLES: usize = 1 << 16;
/// Most samples a corrected path may hold.
pub(crate) const MAX_CORRECTED_SAMPLES: usize = 1 << 16;
/// Largest stroke (Σ dab bounding-box px, as drawn live) given a Full replay,
/// and the largest clipped (Tail) replay. Calibrated in
/// `plans/bench/B004_stroke_replay.md`: ≤ 12.7 ns per px, so ≤ 100 ms.
pub(crate) const MAX_FULL_REPLAY_PX: u64 = 7_500_000;
/// Smoothing kernel width (half-width ≤ 128).
const MAX_TAPS: usize = 257;
/// Largest tile set a [`TileClip`] covers (bits); beyond it the clip stays empty.
const MAX_CLIP_TILES: i64 = 1 << 22;

/// One logged stroke sample: a stabilized [`crate::InputSample`] with the
/// (clamped) time step hokusai was given for it. Pressure is before taper.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShapeSample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub dt: f64,
}

#[inline]
pub fn seg_len(a: &ShapeSample, b: &ShapeSample) -> f32 {
    (b.x - a.x).hypot(b.y - a.y)
}

/// Pressure factor at arc length `s`; `total = None` while live (entry side only).
///
/// With the exit side ≥ 1 the result is bit-identical to the live,
/// entry-only factor, which is what makes a clipped (Tail) replay exact.
#[inline]
pub fn taper(s: f32, total: Option<f32>, taper_in: f32, taper_out: f32) -> f32 {
    let mut k = 1.0f32;
    if taper_in > 0.0 {
        k = k.min(s / taper_in);
    }
    if let Some(t) = total
        && taper_out > 0.0
    {
        k = k.min((t - s) / taper_out);
    }
    k.clamp(0.0, 1.0)
}

/// Smoothing sigma in document px for post correction `level` at `view_zoom`
/// screen px per document px: the same level looks the same at any zoom.
pub fn correction_sigma_px(level: u8, view_zoom: f32) -> f32 {
    CORRECTION_SIGMA[level.min(MAX_CORRECTION) as usize] / view_zoom.max(1e-3)
}

/// Resample `src` at uniform arc spacing, then Gaussian-smooth x/y. Returns the largest
/// displacement the smoothing applied (px). Reuses `out`/`scratch`; no allocation once warm.
///
/// Pressure, tilt and time are interpolated linearly in arc length (the
/// total time is kept); only positions are smoothed. Odd reflection at both
/// ends keeps the endpoints and straight lines exact.
pub fn correct_path(src: &[ShapeSample], sigma_px: f32, out: &mut Vec<ShapeSample>, scratch: &mut Vec<[f32; 2]>) -> f32 {
    out.clear();
    scratch.clear();
    let total: f32 = src.windows(2).map(|w| seg_len(&w[0], &w[1])).sum();
    // NaN coordinates (a broken driver) also take the copy path.
    if src.len() < 3 || total.is_nan() || total < 0.5 {
        out.extend_from_slice(src);
        return 0.0;
    }
    let sigma = if sigma_px.is_finite() { sigma_px.max(0.0) } else { 0.0 };
    let mut h = (sigma / 3.0).clamp(0.5, 4.0);
    if total / h + 2.0 > MAX_CORRECTED_SAMPLES as f32 {
        h = total / (MAX_CORRECTED_SAMPLES - 2) as f32;
    }

    // Uniform stations s_j = j·h, then the exact last point. h is shrunk so
    // the path holds a whole number of steps: the last gap is h too, and the
    // odd reflection then continues a straight end exactly.
    let stations = ((total / h).ceil() as usize).clamp(2, MAX_CORRECTED_SAMPLES - 2);
    h = total / stations as f32;
    let last = src[src.len() - 1];
    // Walk the source polyline once. `seg` is the current segment's start
    // index; `seg_s`/`seg_t` its arc length and cumulative time (excluding
    // the first sample's dt, which `out[0]` keeps).
    let (mut seg, mut seg_s, mut seg_t) = (0usize, 0.0f32, 0.0f64);
    let mut prev_t = 0.0f64;
    for j in 0..stations {
        let s = j as f32 * h;
        let mut len = seg_len(&src[seg], &src[seg + 1]);
        while seg + 2 < src.len() && seg_s + len < s {
            seg_s += len;
            seg_t += src[seg + 1].dt;
            seg += 1;
            len = seg_len(&src[seg], &src[seg + 1]);
        }
        let (a, b) = (&src[seg], &src[seg + 1]);
        let u = if len > 0.0 { ((s - seg_s) / len).clamp(0.0, 1.0) } else { 0.0 };
        let lerp = |p: f32, q: f32| p + (q - p) * u;
        let t = seg_t + b.dt * u as f64;
        let dt = if j == 0 { src[0].dt } else { t - prev_t };
        prev_t = t;
        out.push(ShapeSample {
            x: lerp(a.x, b.x),
            y: lerp(a.y, b.y),
            pressure: lerp(a.pressure, b.pressure),
            tilt_x: lerp(a.tilt_x, b.tilt_x),
            tilt_y: lerp(a.tilt_y, b.tilt_y),
            dt,
        });
    }
    let t_end: f64 = src[1..].iter().map(|s| s.dt).sum();
    out.push(ShapeSample { dt: t_end - prev_t, ..last });
    if let Some(first) = out.first_mut() {
        let s0 = src[0];
        (first.x, first.y) = (s0.x, s0.y);
    }
    scratch.extend(out.iter().map(|s| [s.x, s.y]));

    let n = out.len();
    let taps = if sigma > 0.0 { ((3.0 * sigma / h).ceil() as usize).min(MAX_TAPS / 2).min(n - 1) } else { 0 };
    if taps == 0 {
        return 0.0;
    }
    let mut w = [0.0f32; MAX_TAPS];
    let mut wsum = 0.0f32;
    for (k, wk) in w.iter_mut().enumerate().take(taps + 1) {
        let d = k as f32 * h;
        *wk = (-(d * d) / (2.0 * sigma * sigma)).exp();
        wsum += if k == 0 { *wk } else { 2.0 * *wk };
    }
    for wk in &mut w[..=taps] {
        *wk /= wsum;
    }
    // Odd reflection: p[-i] = 2p₀ − p[i], p[n-1+i] = 2p[n-1] − p[n-1-i].
    let at = |i: isize| -> [f32; 2] {
        let last = n as isize - 1;
        if i < 0 {
            let (o, p) = (scratch[0], scratch[(-i) as usize]);
            [2.0 * o[0] - p[0], 2.0 * o[1] - p[1]]
        } else if i > last {
            let (o, p) = (scratch[last as usize], scratch[(2 * last - i) as usize]);
            [2.0 * o[0] - p[0], 2.0 * o[1] - p[1]]
        } else {
            scratch[i as usize]
        }
    };
    let mut shift = 0.0f32;
    // Endpoints stay exactly where the pen touched and lifted.
    for (j, o) in out.iter_mut().enumerate().take(n - 1).skip(1) {
        let c = scratch[j];
        let (mut x, mut y) = (w[0] * c[0], w[0] * c[1]);
        for (k, &wk) in w.iter().enumerate().take(taps + 1).skip(1) {
            let (l, r) = (at(j as isize - k as isize), at((j + k) as isize));
            x += wk * (l[0] + r[0]);
            y += wk * (l[1] + r[1]);
        }
        shift = shift.max((x - c[0]).hypot(y - c[1]));
        (o.x, o.y) = (x, y);
    }
    shift
}

/// A set of tiles a clipped replay may repaint, as a bitmap over its bounding box.
#[derive(Default)]
pub struct TileClip {
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    bits: Vec<u64>,
}

/// Tile range `[lo, hi]` covering pixels `a..=b` (clamped to avoid overflow).
#[inline]
fn tile_span(a: f32, b: f32) -> (i32, i32) {
    const LIM: f32 = (1i64 << 30) as f32;
    let t = |v: f32| (v.clamp(-LIM, LIM).floor() as i32).div_euclid(TILE_SIZE as i32);
    (t(a), t(b))
}

impl TileClip {
    /// Every tile within `r_max` of a segment of `path` (segment capsule bbox ± r_max).
    /// Allocates only when the set outgrows every previous one.
    pub(crate) fn build(&mut self, path: &[ShapeSample], r_max: f32) {
        self.bits.clear();
        (self.x0, self.y0, self.w, self.h) = (0, 0, 0, 0);
        let r = if r_max.is_finite() { r_max.max(0.0) } else { 0.0 };
        let Some(first) = path.first() else { return };
        let (mut lo_x, mut lo_y, mut hi_x, mut hi_y) = (first.x, first.y, first.x, first.y);
        for s in path {
            lo_x = lo_x.min(s.x);
            lo_y = lo_y.min(s.y);
            hi_x = hi_x.max(s.x);
            hi_y = hi_y.max(s.y);
        }
        if !(lo_x.is_finite() && lo_y.is_finite() && hi_x.is_finite() && hi_y.is_finite()) {
            return;
        }
        let (tx0, tx1) = tile_span(lo_x - r, hi_x + r);
        let (ty0, ty1) = tile_span(lo_y - r, hi_y + r);
        let (w, h) = (tx1 as i64 - tx0 as i64 + 1, ty1 as i64 - ty0 as i64 + 1);
        if w * h > MAX_CLIP_TILES {
            return;
        }
        (self.x0, self.y0, self.w, self.h) = (tx0, ty0, w as i32, h as i32);
        self.bits.resize(((w * h) as usize).div_ceil(64), 0);
        let mut mark = |a: &ShapeSample, b: &ShapeSample| {
            let (x0, x1) = tile_span(a.x.min(b.x) - r, a.x.max(b.x) + r);
            let (y0, y1) = tile_span(a.y.min(b.y) - r, a.y.max(b.y) + r);
            for ty in y0..=y1 {
                for tx in x0..=x1 {
                    let i = ((ty - self.y0) * self.w + (tx - self.x0)) as usize;
                    self.bits[i / 64] |= 1 << (i % 64);
                }
            }
        };
        mark(first, first);
        for s in path.windows(2) {
            mark(&s[0], &s[1]);
        }
    }

    #[inline]
    fn index(&self, tx: i32, ty: i32) -> Option<usize> {
        let (dx, dy) = (tx.wrapping_sub(self.x0), ty.wrapping_sub(self.y0));
        (dx >= 0 && dy >= 0 && dx < self.w && dy < self.h).then(|| (dy * self.w + dx) as usize)
    }

    #[inline]
    fn bit(&self, tx: i32, ty: i32) -> bool {
        self.index(tx, ty).is_some_and(|i| self.bits[i / 64] & (1 << (i % 64)) != 0)
    }

    pub fn contains(&self, c: TileCoord) -> bool {
        self.bit(c.x, c.y)
    }

    /// Whether a dab of radius `r` at `(x, y)` (its bounding box) reaches the set.
    pub(crate) fn touches(&self, x: f32, y: f32, r: f32) -> bool {
        if self.w == 0 {
            return false;
        }
        let (x0, x1) = tile_span(x - r, x + r);
        let (y0, y1) = tile_span(y - r, y + r);
        let (x0, x1) = (x0.max(self.x0), x1.min(self.x0 + self.w - 1));
        let (y0, y1) = (y0.max(self.y0), y1.min(self.y0 + self.h - 1));
        (y0..=y1).any(|ty| (x0..=x1).any(|tx| self.bit(tx, ty)))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bits.iter().all(|&b| b == 0)
    }
}

/// Dabs drawn and the pixels their bounding boxes cover (cost of a replay).
#[derive(Debug, Default, Clone, Copy)]
pub struct DabStats {
    pub dabs: u64,
    pub px: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> ShapeSample {
        ShapeSample { x, y, pressure: 1.0, dt: 0.01, ..Default::default() }
    }

    /// Deterministic 0..1 values (xorshift).
    fn rand_iter(mut seed: u32) -> impl Iterator<Item = f32> {
        std::iter::from_fn(move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            Some((seed >> 8) as f32 / (1u32 << 24) as f32)
        })
    }

    #[test]
    fn taper_is_linear_ramp_in_and_out() {
        let (tin, tout, total) = (40.0, 80.0, 400.0);
        for s in [0.0f32, 10.0, 20.0, 39.0] {
            assert!((taper(s, Some(total), tin, tout) - s / tin).abs() < 1e-6);
        }
        assert_eq!(taper(200.0, Some(total), tin, tout), 1.0);
        for s in [320.0f32, 360.0, 399.0, 400.0] {
            assert!((taper(s, Some(total), tin, tout) - (total - s) / tout).abs() < 1e-6);
        }
        // Live: entry side only.
        assert_eq!(taper(390.0, None, tin, tout), 1.0);
        assert_eq!(taper(20.0, None, tin, tout), 0.5);
    }

    #[test]
    fn short_stroke_taper_takes_min() {
        // 60 px stroke with 40 in / 80 out: the ramps overlap, the lower wins.
        let (tin, tout, total) = (40.0, 80.0, 60.0);
        for i in 0..=60 {
            let s = i as f32;
            let k = taper(s, Some(total), tin, tout);
            let want = (s / tin).min((total - s) / tout).clamp(0.0, 1.0);
            assert_eq!(k, want);
            assert!(k < 1.0);
        }
    }

    #[test]
    fn taper_off_is_exactly_one() {
        for s in [0.0f32, 1e-6, 3.0, 1e6] {
            assert_eq!(taper(s, Some(10.0), 0.0, 0.0).to_bits(), 1.0f32.to_bits());
            assert_eq!(taper(s, None, 0.0, 0.0).to_bits(), 1.0f32.to_bits());
        }
    }

    #[test]
    fn live_and_final_taper_agree_before_tail() {
        let mut r = rand_iter(7);
        for _ in 0..10_000 {
            let tin = r.next().unwrap() * 100.0;
            let tout = r.next().unwrap() * 200.0 + 0.01;
            let total = r.next().unwrap() * 5000.0;
            let s = r.next().unwrap() * total;
            if total - s >= tout {
                let live = taper(s, None, tin, tout);
                let fin = taper(s, Some(total), tin, tout);
                assert_eq!(live.to_bits(), fin.to_bits(), "s {s} total {total} in {tin} out {tout}");
            }
        }
    }

    #[test]
    fn resample_keeps_endpoints_spacing_and_total_dt() {
        // Uneven input spacing and times along an L.
        let src: Vec<ShapeSample> = [(0.0, 0.0, 0.0), (3.0, 0.0, 0.004), (10.0, 0.0, 0.02), (10.0, 7.5, 0.008), (10.0, 20.0, 0.03)]
            .iter()
            .map(|&(x, y, dt)| ShapeSample { x, y, pressure: x / 10.0, dt, ..Default::default() })
            .collect();
        let (mut out, mut scratch) = (Vec::new(), Vec::new());
        // sigma 0: resampling only (h = 0.5).
        let shift = correct_path(&src, 0.0, &mut out, &mut scratch);
        assert_eq!(shift, 0.0);
        assert_eq!((out[0].x, out[0].y), (0.0, 0.0));
        let l = out.last().unwrap();
        assert_eq!((l.x, l.y), (10.0, 20.0));
        let gaps: Vec<f32> = out.windows(2).map(|w| seg_len(&w[0], &w[1])).collect();
        // Uniform ≤ 0.5 px steps (in arc length; a chord across the corner is shorter).
        let h = 30.0 / gaps.len() as f32;
        assert!(h <= 0.5 && h > 0.45);
        for (i, g) in gaps.iter().enumerate() {
            assert!((g - h).abs() < 1e-3 || (i > 0 && *g < h && *g > 0.7 * h), "gap {i} = {g}");
        }
        let dt_in: f64 = src.iter().map(|s| s.dt).sum();
        let dt_out: f64 = out.iter().map(|s| s.dt).sum();
        assert!((dt_in - dt_out).abs() < 1e-9, "{dt_in} vs {dt_out}");
        assert!(out.iter().all(|s| s.dt >= -1e-12));
        // Pressure is linear in arc length along the first leg.
        let mid = out.iter().find(|s| s.y == 0.0 && (s.x - 5.0).abs() < 1e-4).unwrap();
        assert!((mid.pressure - 0.5).abs() < 1e-4);
    }

    #[test]
    fn smoothing_keeps_endpoints_and_straight_lines() {
        let src: Vec<ShapeSample> = (0..50).map(|i| at(3.0 + i as f32 * 2.3, 7.0 + i as f32 * 1.1)).collect();
        let (mut out, mut scratch) = (Vec::new(), Vec::new());
        let shift = correct_path(&src, 6.0, &mut out, &mut scratch);
        assert_eq!((out[0].x, out[0].y), (src[0].x, src[0].y));
        let (l, sl) = (out.last().unwrap(), src.last().unwrap());
        assert_eq!((l.x, l.y), (sl.x, sl.y));
        // Every point stays on the line (any along-line shift is harmless).
        for s in &out {
            let off = ((s.x - 3.0) * 1.1 - (s.y - 7.0) * 2.3) / 2.3f32.hypot(1.1);
            assert!(off.abs() < 1e-3, "off the line by {off}");
        }
        assert!(shift < 1e-3, "a straight line does not move: {shift}");
    }

    #[test]
    fn smoothing_damps_zigzag() {
        let src: Vec<ShapeSample> = (0..200).map(|i| at(i as f32 * 2.0, if i % 2 == 0 { 1.5 } else { -1.5 })).collect();
        let (mut out, mut scratch) = (Vec::new(), Vec::new());
        let shift = correct_path(&src, 4.0, &mut out, &mut scratch);
        assert!(shift > 0.5);
        let inner = &out[out.len() / 10..out.len() * 9 / 10];
        let dev = inner.iter().map(|s| s.y.abs()).fold(0.0f32, f32::max);
        assert!(dev < 0.15, "zigzag left {dev} px");
        // Buffers are reused: no growth on a second run.
        let caps = (out.capacity(), scratch.capacity());
        correct_path(&src, 4.0, &mut out, &mut scratch);
        assert_eq!(caps, (out.capacity(), scratch.capacity()));
    }

    #[test]
    fn clip_covers_tail_capsules() {
        let path = [at(10.0, 10.0), at(200.0, 10.0), at(200.0, 150.0)];
        let mut clip = TileClip::default();
        clip.build(&path, 20.0);
        assert!(!clip.is_empty());
        // Every point within r of a segment lies in a clip tile.
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            for (a, b) in [(path[0], path[1]), (path[1], path[2])] {
                let (x, y) = (a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
                for (dx, dy) in [(-20.0, 0.0), (20.0, 0.0), (0.0, -20.0), (0.0, 20.0), (14.0, 14.0)] {
                    let c = TileCoord::from_pixel((x + dx).floor() as i32, (y + dy).floor() as i32);
                    assert!(clip.contains(c), "({}, {}) not covered", x + dx, y + dy);
                }
            }
        }
        // Far tiles are not.
        assert!(!clip.contains(TileCoord::new(0, 3)), "inside the L's corner, beyond r");
        assert!(!clip.contains(TileCoord::new(10, 10)));
        assert!(clip.touches(100.0, 40.0, 1.0), "inside (10,10)-(200,10) capsule tile row");
        assert!(!clip.touches(20.0, 200.0, 10.0));
        assert!(clip.touches(20.0, 200.0, 160.0), "a big dab reaches the set");

        clip.build(&[], 5.0);
        assert!(clip.is_empty() && !clip.contains(TileCoord::new(0, 0)) && !clip.touches(0.0, 0.0, 5.0));
        clip.build(&[at(0.0, 0.0), at(1e9, 1e9)], 5.0);
        assert!(clip.is_empty(), "absurd extents give up instead of allocating gigabytes");
    }

    #[test]
    fn correction_sigma_scales_with_zoom() {
        assert_eq!(correction_sigma_px(0, 1.0), 0.0);
        assert_eq!(correction_sigma_px(4, 1.0), 3.0);
        assert_eq!(correction_sigma_px(4, 2.0), 1.5);
        assert_eq!(correction_sigma_px(4, 0.5), 6.0);
        assert_eq!(correction_sigma_px(200, 1.0), 12.0);
        assert!(correction_sigma_px(3, 0.0).is_finite());
    }
}
