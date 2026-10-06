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
/// `plans/bench/B004_stroke_replay.md`: worst 12.95 ns per px, so ≤ 97 ms (≤ 100 ms).
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

/// Station step `h` (px) for post correction `sigma_px`.
#[inline]
pub fn station_step(sigma_px: f32) -> f32 {
    let sigma = if sigma_px.is_finite() { sigma_px.max(0.0) } else { 0.0 };
    (sigma / 3.0).clamp(0.5, 4.0)
}

/// Kernel half-width in stations for post-correction with `sigma` and station step `h`.
#[inline]
pub fn correction_taps(sigma_px: f32, h: f32) -> usize {
    let sigma = if sigma_px.is_finite() { sigma_px.max(0.0) } else { 0.0 };
    if sigma > 0.0 && h > 0.0 {
        ((3.0 * sigma / h).ceil() as usize).min(MAX_TAPS / 2)
    } else {
        0
    }
}

#[inline]
fn point_at_arc(src: &[ShapeSample], s: f32) -> [f32; 2] {
    let s = s.max(0.0);
    let mut seg_s = 0.0f32;
    for w in src.windows(2) {
        let len = seg_len(&w[0], &w[1]);
        if seg_s + len >= s || len == 0.0 {
            let u = if len > 0.0 { ((s - seg_s) / len).clamp(0.0, 1.0) } else { 0.0 };
            return [w[0].x + (w[1].x - w[0].x) * u, w[0].y + (w[1].y - w[0].y) * u];
        }
        seg_s += len;
    }
    if let Some(last) = src.last() {
        [last.x, last.y]
    } else {
        [0.0, 0.0]
    }
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
    let mut c = Corrector::new(sigma_px);
    c.take(src);
    let total = c.total;
    // NaN coordinates (a broken driver) also take the copy path.
    if src.len() < 3 || total.is_nan() || total < 0.5 {
        out.extend_from_slice(src);
        return 0.0;
    }
    let mut h = station_step(sigma_px);
    if total / h + 2.0 > MAX_CORRECTED_SAMPLES as f32 {
        h = total / (MAX_CORRECTED_SAMPLES - 2) as f32;
    }
    c.set_step(h);
    c.complete(src, out, scratch);
    c.shift
}

/// [`correct_path`] as a running computation, so the speculative worker can
/// correct the log as it grows with the very arithmetic of the one-shot call:
/// stations `s_j = j·h` never move, a station is resampled once the log
/// reaches it and smoothed once its whole kernel support is resampled.
pub(crate) struct Corrector {
    sigma: f32,
    h: f32,
    /// Kernel half-width (stations) and its normalized weights.
    taps: usize,
    w: [f32; MAX_TAPS],
    /// Samples taken, their arc length and their time after the first, summed in order.
    taken: usize,
    total: f32,
    t_end: f64,
    /// The resampling walk: current segment start, its arc length and
    /// cumulative time (excluding the first sample's dt, which station 0
    /// keeps); the previous station's time.
    seg: usize,
    seg_s: f32,
    seg_t: f64,
    prev_t: f64,
    /// Stations before this one are final (station 0 is never smoothed).
    smoothed: usize,
    shift: f32,
}

impl Corrector {
    /// Starts with step [`station_step`], the step of every log short enough to correct.
    pub(crate) fn new(sigma_px: f32) -> Self {
        let sigma = if sigma_px.is_finite() { sigma_px.max(0.0) } else { 0.0 };
        let mut c = Self {
            sigma,
            h: 0.0,
            taps: 0,
            w: [0.0; MAX_TAPS],
            taken: 0,
            total: 0.0,
            t_end: -0.0,
            seg: 0,
            seg_s: 0.0,
            seg_t: 0.0,
            prev_t: 0.0,
            smoothed: 1,
            shift: 0.0,
        };
        c.set_step(station_step(sigma_px));
        c
    }

    fn set_step(&mut self, h: f32) {
        self.h = h;
        self.set_taps(if self.sigma > 0.0 { ((3.0 * self.sigma / h).ceil() as usize).min(MAX_TAPS / 2) } else { 0 });
    }

    fn set_taps(&mut self, taps: usize) {
        self.taps = taps;
        if taps == 0 {
            return;
        }
        let (sigma, h) = (self.sigma, self.h);
        let mut wsum = 0.0f32;
        for (k, wk) in self.w.iter_mut().enumerate().take(taps + 1) {
            let d = k as f32 * h;
            *wk = (-(d * d) / (2.0 * sigma * sigma)).exp();
            wsum += if k == 0 { *wk } else { 2.0 * *wk };
        }
        for wk in &mut self.w[..=taps] {
            *wk /= wsum;
        }
    }

    /// How many leading stations of `out` no later sample can change.
    pub(crate) fn final_len(&self, out: &[ShapeSample]) -> usize {
        self.smoothed.min(out.len())
    }

    /// Add the samples `src` gained since the last call to the running sums.
    fn take(&mut self, src: &[ShapeSample]) {
        for i in self.taken.max(1)..src.len() {
            self.total += seg_len(&src[i - 1], &src[i]);
            self.t_end += src[i].dt;
        }
        self.taken = self.taken.max(src.len());
    }

    /// Take the samples `src` (the log so far) gained, then resample and smooth
    /// every station no later sample can change. False once `src` can no longer
    /// be corrected with this step (NaN, or so long that [`correct_path`] picks another).
    pub(crate) fn advance(&mut self, src: &[ShapeSample], out: &mut Vec<ShapeSample>, scratch: &mut Vec<[f32; 2]>) -> bool {
        self.take(src);
        if self.total.is_nan() || self.total / self.h + 2.0 > MAX_CORRECTED_SAMPLES as f32 {
            return false;
        }
        if src.len() < 3 {
            return true;
        }
        // Stations the log already reaches sit at the same s_j in the final path.
        let reach = (self.total / self.h).floor() as usize;
        while out.len() <= reach && out.len() as f32 * self.h <= self.total {
            self.station(src, out, scratch);
        }
        // Smoothed once the kernel support is resampled, short of the last
        // station (where the final path may put the lift-off point).
        while self.taps > 0 && self.smoothed + self.taps + 2 <= out.len() {
            self.smooth(self.smoothed, src, out, scratch);
            self.smoothed += 1;
        }
        true
    }

    /// Complete the path for the whole log `src`; false (and nothing done) if
    /// [`correct_path`] would not correct it with this step.
    pub(crate) fn finish(&mut self, src: &[ShapeSample], out: &mut Vec<ShapeSample>, scratch: &mut Vec<[f32; 2]>) -> bool {
        self.take(src);
        let exact = self.taken >= 3 && self.total >= 0.5 && self.total / self.h + 2.0 <= MAX_CORRECTED_SAMPLES as f32;
        if exact {
            self.complete(src, out, scratch);
        }
        exact
    }

    /// The remaining stations, the exact lift-off point and total time, and
    /// the smoothing near the end.
    fn complete(&mut self, src: &[ShapeSample], out: &mut Vec<ShapeSample>, scratch: &mut Vec<[f32; 2]>) {
        // Fixed stations s_j = j·h up to total, then the exact last point. h is fixed
        // so existing stations s_j never shift as new samples arrive, making the stable
        // prefix invariant to future points!
        let num_steps = ((self.total / self.h).floor() as usize).clamp(1, MAX_CORRECTED_SAMPLES - 2);
        while out.len() <= num_steps {
            self.station(src, out, scratch);
        }
        let last = src[src.len() - 1];
        let rest = (self.t_end - self.prev_t).max(0.0);
        if (num_steps as f32 * self.h) < self.total - 1e-4 {
            out.push(ShapeSample { dt: rest, ..last });
            scratch.push([last.x, last.y]);
        } else if let Some(last_out) = out.last_mut() {
            last_out.dt += rest;
        }
        // Endpoints stay exactly where the pen touched and lifted.
        if let (Some(end), Some(p)) = (out.last_mut(), scratch.last_mut()) {
            (end.x, end.y) = (last.x, last.y);
            *p = [last.x, last.y];
        }

        let n = out.len();
        let taps = self.taps.min(n - 1);
        if taps == 0 {
            return;
        }
        if taps != self.taps {
            // Only a path too short to have smoothed anything yet.
            self.set_taps(taps);
        }
        for j in self.smoothed..n - 1 {
            self.smooth(j, src, out, scratch);
        }
        self.smoothed = self.smoothed.max(n - 1);
    }

    /// Resample the next station, walking the source polyline on from the last one.
    fn station(&mut self, src: &[ShapeSample], out: &mut Vec<ShapeSample>, scratch: &mut Vec<[f32; 2]>) {
        let j = out.len();
        let s = (j as f32 * self.h).min(self.total);
        let mut len = seg_len(&src[self.seg], &src[self.seg + 1]);
        while self.seg + 2 < src.len() && self.seg_s + len < s {
            self.seg_s += len;
            self.seg_t += src[self.seg + 1].dt;
            self.seg += 1;
            len = seg_len(&src[self.seg], &src[self.seg + 1]);
        }
        let (a, b) = (&src[self.seg], &src[self.seg + 1]);
        let u = if len > 0.0 { ((s - self.seg_s) / len).clamp(0.0, 1.0) } else { 0.0 };
        let lerp = |p: f32, q: f32| p + (q - p) * u;
        let t = self.seg_t + b.dt * u as f64;
        let dt = if j == 0 { src[0].dt } else { t - self.prev_t };
        self.prev_t = t;
        let mut o = ShapeSample {
            x: lerp(a.x, b.x),
            y: lerp(a.y, b.y),
            pressure: lerp(a.pressure, b.pressure),
            tilt_x: lerp(a.tilt_x, b.tilt_x),
            tilt_y: lerp(a.tilt_y, b.tilt_y),
            dt,
        };
        if j == 0 {
            (o.x, o.y) = (src[0].x, src[0].y);
        }
        out.push(o);
        scratch.push([o.x, o.y]);
    }

    /// Smooth station `j` in place from the resampled positions in `scratch`.
    fn smooth(&mut self, j: usize, src: &[ShapeSample], out: &mut [ShapeSample], scratch: &[[f32; 2]]) {
        let (h, total, w) = (self.h, self.total, &self.w);
        // Odd reflection: p[-i] = 2p₀ − p[i], and for points past the end, reflect
        // arc length across total: s_refl = 2*total - i*h, ensuring odd reflection
        // continues straight lines exactly even when total is not an integer multiple of h.
        let last = src[src.len() - 1];
        let at = |idx: isize| -> [f32; 2] {
            let s = idx as f32 * h;
            if idx < 0 {
                let (o, p) = (scratch[0], scratch[(-idx) as usize]);
                [2.0 * o[0] - p[0], 2.0 * o[1] - p[1]]
            } else if s > total {
                let s_refl = (2.0 * total - s).max(0.0);
                let p = point_at_arc(src, s_refl);
                let o = [last.x, last.y];
                [2.0 * o[0] - p[0], 2.0 * o[1] - p[1]]
            } else if (idx as usize) < scratch.len() {
                scratch[idx as usize]
            } else {
                [last.x, last.y]
            }
        };
        let c = scratch[j];
        let (mut x, mut y) = (w[0] * c[0], w[0] * c[1]);
        for (k, &wk) in w.iter().enumerate().take(self.taps + 1).skip(1) {
            let (l, r) = (at(j as isize - k as isize), at((j + k) as isize));
            x += wk * (l[0] + r[0]);
            y += wk * (l[1] + r[1]);
        }
        self.shift = self.shift.max((x - c[0]).hypot(y - c[1]));
        (out[j].x, out[j].y) = (x, y);
    }
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

    #[test]
    fn incremental_correction_is_bit_identical() {
        let mut r = rand_iter(4242);
        for case in 0..120 {
            let sigma = [0.75f32, 1.5, 2.25, 3.0, 5.0, 12.0][case % 6];
            // A wobbly walk with zigzags, pauses (repeated points) and speed changes.
            let n = 3 + (r.next().unwrap() * 500.0) as usize;
            let (mut x, mut y) = (100.0 * r.next().unwrap(), 100.0 * r.next().unwrap());
            let mut pts = Vec::with_capacity(n);
            for i in 0..n {
                let v = r.next().unwrap();
                if v > 0.1 {
                    let speed = if case % 4 == 0 { 0.3 } else { 4.0 * v };
                    x += speed + (r.next().unwrap() - 0.5) * if i % 9 < 3 { 6.0 } else { 1.0 };
                    y += (r.next().unwrap() - 0.5) * 3.0;
                }
                let pressure = r.next().unwrap();
                pts.push(ShapeSample { x, y, pressure, tilt_x: v, dt: 0.002 + 0.01 * v as f64, ..Default::default() });
            }
            let (mut want, mut want_scratch) = (Vec::new(), Vec::new());
            let want_shift = correct_path(&pts, sigma, &mut want, &mut want_scratch);

            // Fed in random chunks, as the worker pops them.
            let mut c = Corrector::new(sigma);
            let (mut out, mut scratch) = (Vec::new(), Vec::new());
            let (mut fed, mut done) = (0, 0);
            while fed < n {
                fed = (fed + 1 + (r.next().unwrap() * 8.0) as usize).min(n);
                assert!(c.advance(&pts[..fed], &mut out, &mut scratch));
                assert!(c.final_len(&out) >= done);
                done = c.final_len(&out);
            }
            let total: f32 = pts.windows(2).map(|w| seg_len(&w[0], &w[1])).sum();
            if total < 0.5 {
                continue;
            }
            assert!(c.finish(&pts, &mut out, &mut scratch));
            assert_eq!(out.len(), want.len(), "case {case}");
            for (j, (a, b)) in out.iter().zip(&want).enumerate() {
                let bits = |s: &ShapeSample| {
                    [s.x.to_bits(), s.y.to_bits(), s.pressure.to_bits(), s.tilt_x.to_bits(), s.tilt_y.to_bits()]
                };
                assert_eq!(bits(a), bits(b), "case {case} station {j} of {} (final before pen-up: {done})", out.len());
                assert_eq!(a.dt.to_bits(), b.dt.to_bits(), "case {case} station {j}: dt");
            }
            assert_eq!(c.shift.to_bits(), want_shift.to_bits(), "case {case}: shift");
            if total > 100.0 {
                assert!(done * 2 > out.len(), "case {case}: only {done} of {} stations final before pen-up", out.len());
            }
        }
    }

    #[test]
    fn incremental_correction_gives_up_past_its_step() {
        // So long that the one-shot call stretches h: the running one must stop.
        let pts: Vec<ShapeSample> = (0..2000).map(|i| at(i as f32 * 20.0, 0.0)).collect();
        let (mut out, mut scratch) = (Vec::new(), Vec::new());
        let mut c = Corrector::new(1.5);
        assert!(c.advance(&pts[..1000], &mut out, &mut scratch));
        assert!(!c.advance(&pts, &mut out, &mut scratch));
        assert!(!c.finish(&pts, &mut out, &mut scratch));
        let mut nan = pts[..10].to_vec();
        nan[5].x = f32::NAN;
        assert!(!Corrector::new(1.5).advance(&nan, &mut out, &mut scratch));
    }

    #[test]
    fn stable_prefix_is_bit_identical_across_stroke_growth() {
        // Generate a random polyline with wobble.
        let mut r = rand_iter(1337);
        let mut pts = Vec::new();
        let (mut x, mut y) = (50.0f32, 50.0f32);
        for _ in 0..300 {
            x += 2.0 + r.next().unwrap() * 2.0;
            y += (r.next().unwrap() - 0.5) * 4.0;
            let pressure = 0.3 + 0.6 * r.next().unwrap();
            pts.push(ShapeSample { x, y, pressure, dt: 0.005, ..Default::default() });
        }

        let short_pts = &pts[..120];
        let long_pts = &pts[..250];

        for sigma in [1.5f32, 2.25, 3.0, 5.0] {
            let (mut out_short, mut scr_short) = (Vec::new(), Vec::new());
            let (mut out_long, mut scr_long) = (Vec::new(), Vec::new());

            correct_path(short_pts, sigma, &mut out_short, &mut scr_short);
            correct_path(long_pts, sigma, &mut out_long, &mut scr_long);

            let h = station_step(sigma);
            let taps = correction_taps(sigma, h);
            let corr_reach = taps as f32 * h;
            let short_total: f32 = short_pts.windows(2).map(|w| seg_len(&w[0], &w[1])).sum();
            let cutoff = short_total - corr_reach;

            let mut checked = 0;
            for (j, (s, l)) in out_short.iter().zip(&out_long).enumerate() {
                let station_s = j as f32 * h;
                if station_s > cutoff {
                    break;
                }
                assert_eq!(s.x.to_bits(), l.x.to_bits(), "sigma {sigma} station {j}: x differed");
                assert_eq!(s.y.to_bits(), l.y.to_bits(), "sigma {sigma} station {j}: y differed");
                assert_eq!(s.pressure.to_bits(), l.pressure.to_bits(), "sigma {sigma} station {j}: pressure differed");
                assert_eq!(s.dt.to_bits(), l.dt.to_bits(), "sigma {sigma} station {j}: dt differed");
                checked += 1;
            }
            assert!(checked > 30, "sigma {sigma}: expected > 30 stable stations, checked {checked}");
        }
    }
}

