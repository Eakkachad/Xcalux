//! Pen samples and the stroke stabilizer.

/// One pointer sample in document pixel space.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputSample {
    pub x: f32,
    pub y: f32,
    /// 0..=1
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// Seconds, monotonic.
    pub time: f64,
}

/// Ring size: the widest window (level 15, ≈ 0.38 s) at a 1 kHz pen.
const CAP: usize = 512;

/// Window length in seconds for `level`: `3·level + 1` samples at 120 Hz.
#[inline]
fn window_secs(level: u8) -> f64 {
    (3 * level as u32 + 1) as f64 / 120.0
}

/// SAI-style stabilizer: a weighted moving average over the samples of the
/// last `(3·level + 1) / 120` seconds, newest weighted most (less lag than a
/// box filter at equal smoothness). The window is in time, not samples, so
/// a 240 Hz pen feels like a 60 Hz mouse at the same level. On pen-up,
/// [`Stabilizer::drain_step`] lets the line catch up to where the pen
/// actually lifted.
///
/// Fixed-size ring buffer — never allocates.
pub struct Stabilizer {
    level: u8,
    buf: [InputSample; CAP],
    start: usize,
    len: usize,
}

impl Default for Stabilizer {
    fn default() -> Self {
        Self { level: 0, buf: [InputSample::default(); CAP], start: 0, len: 0 }
    }
}

impl Stabilizer {
    pub const MAX_LEVEL: u8 = 15;

    pub fn set_level(&mut self, level: u8) {
        self.level = level.min(Self::MAX_LEVEL);
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn reset(&mut self) {
        self.start = 0;
        self.len = 0;
    }

    #[inline]
    fn at(&self, i: usize) -> &InputSample {
        &self.buf[(self.start + i) % CAP]
    }

    /// Add a raw sample, returning the smoothed one.
    pub fn push(&mut self, s: InputSample) -> InputSample {
        if self.level == 0 {
            return s;
        }
        if self.len == CAP {
            self.start = (self.start + 1) % CAP;
            self.len -= 1;
        }
        self.buf[(self.start + self.len) % CAP] = s;
        self.len += 1;
        let window = window_secs(self.level);
        while self.len > 1 && s.time - self.at(0).time >= window {
            self.start = (self.start + 1) % CAP;
            self.len -= 1;
        }
        self.average()
    }

    /// Drop the oldest sample and return the new average, until the output
    /// has converged on the last raw sample.
    pub fn drain_step(&mut self) -> Option<InputSample> {
        if self.len <= 1 {
            return None;
        }
        self.start = (self.start + 1) % CAP;
        self.len -= 1;
        Some(self.average())
    }

    fn average(&self) -> InputSample {
        let newest = *self.at(self.len - 1);
        let window = window_secs(self.level);
        // Weight grows linearly with recency across the window; at a constant
        // rate this is the classic (i + 1) ramp.
        let from = newest.time - window;
        let floor = window * 1e-3;
        let (mut x, mut y, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..self.len {
            let s = self.at(i);
            let w = (s.time - from).max(floor);
            x += s.x as f64 * w;
            y += s.y as f64 * w;
            wsum += w;
        }
        // Pressure follows the pen more closely than position: average only
        // the newest third of the window so taper still feels immediate.
        let recent = newest.time - window / 3.0;
        let (mut p, mut pn) = (newest.pressure, 1u32);
        for i in (0..self.len - 1).rev() {
            let s = self.at(i);
            if s.time <= recent {
                break;
            }
            p += s.pressure;
            pn += 1;
        }
        InputSample { x: (x / wsum) as f32, y: (y / wsum) as f32, pressure: p / pn as f32, ..newest }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: f32, y: f32) -> InputSample {
        InputSample { x, y, pressure: 1.0, ..Default::default() }
    }

    #[test]
    fn level_zero_is_passthrough() {
        let mut st = Stabilizer::default();
        assert_eq!(st.push(s(5.0, 6.0)), s(5.0, 6.0));
        assert!(st.drain_step().is_none());
    }

    #[test]
    fn smooths_jitter_and_drains_to_last_point() {
        let mut st = Stabilizer::default();
        st.set_level(5);
        let mut max_dev = 0.0f32;
        for i in 0..100 {
            let jitter = if i % 2 == 0 { 3.0 } else { -3.0 };
            let out = st.push(s(i as f32, jitter));
            if i > 20 {
                max_dev = max_dev.max(out.y.abs());
            }
        }
        assert!(max_dev < 0.5, "jitter not smoothed: {max_dev}");
        let mut last = None;
        while let Some(o) = st.drain_step() {
            last = Some(o);
        }
        let last = last.unwrap();
        assert!((last.x - 99.0).abs() < 1e-3, "drain must end at the pen-up point, got {}", last.x);
    }

    /// Steady-state lag (px) of a 500 px/s straight line sampled at `hz`.
    fn lag_at(level: u8, hz: f64) -> f32 {
        let mut st = Stabilizer::default();
        st.set_level(level);
        let mut out = InputSample::default();
        let n = (hz * 2.0) as usize;
        for i in 0..n {
            let t = i as f64 / hz;
            out = st.push(InputSample { x: (500.0 * t) as f32, time: t, pressure: 1.0, ..Default::default() });
        }
        (500.0 * (n - 1) as f64 / hz) as f32 - out.x
    }

    #[test]
    fn window_is_rate_independent() {
        for level in [5, 10, 15] {
            let reference = lag_at(level, 120.0);
            assert!(reference > 5.0, "level {level} smooths at all: lag {reference}");
            for hz in [60.0, 240.0, 480.0] {
                let lag = lag_at(level, hz);
                let rel = lag / reference;
                assert!((0.9..=1.1).contains(&rel), "level {level} at {hz} Hz: lag {lag} vs {reference} at 120 Hz");
            }
        }
    }

    #[test]
    fn max_level_fits_ring_at_1khz() {
        let mut st = Stabilizer::default();
        st.set_level(Stabilizer::MAX_LEVEL);
        for i in 0..3000 {
            st.push(InputSample { x: i as f32, time: i as f64 / 1000.0, ..Default::default() });
            assert!(st.len < CAP, "the window, not the ring, bounds the history ({} samples)", st.len);
        }
        let window = window_secs(Stabilizer::MAX_LEVEL);
        assert!(st.len >= (window * 1000.0) as usize - 1);
        // Same lag as at 120 Hz.
        let rel = lag_at(Stabilizer::MAX_LEVEL, 1000.0) / lag_at(Stabilizer::MAX_LEVEL, 120.0);
        assert!((0.9..=1.1).contains(&rel), "{rel}");
    }

    #[test]
    fn max_level_fits_ring() {
        let mut st = Stabilizer::default();
        st.set_level(255);
        assert_eq!(st.level(), Stabilizer::MAX_LEVEL);
        for i in 0..500 {
            st.push(s(i as f32, 0.0));
        }
    }
}
