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

const CAP: usize = 64;

/// SAI-style stabilizer: a weighted moving average over the last
/// `3·level + 1` samples, newest weighted most (less lag than a box filter
/// at equal smoothness). On pen-up, [`Stabilizer::drain_step`] lets the
/// line catch up to where the pen actually lifted.
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

    fn window(&self) -> usize {
        self.level as usize * 3 + 1
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
        let window = self.window();
        if window <= 1 {
            return s;
        }
        while self.len >= window {
            self.start = (self.start + 1) % CAP;
            self.len -= 1;
        }
        self.buf[(self.start + self.len) % CAP] = s;
        self.len += 1;
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
        let (mut x, mut y, mut wsum) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..self.len {
            let s = self.at(i);
            let w = (i + 1) as f32;
            x += s.x * w;
            y += s.y * w;
            wsum += w;
        }
        // Pressure follows the pen more closely than position: average only
        // the newest third so taper still feels immediate.
        let pn = (self.len / 3).max(1);
        let mut p = 0.0;
        for i in self.len - pn..self.len {
            p += self.at(i).pressure;
        }
        let newest = *self.at(self.len - 1);
        InputSample { x: x / wsum, y: y / wsum, pressure: p / pn as f32, ..newest }
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
