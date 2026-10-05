//! [`PenMeter`]: pen rate and input age for the latency overlay.

use crate::{PenPhase, PenSample, PenStats};

/// Smoothing factor of the age EMA.
const AGE_ALPHA: f32 = 0.2;
/// `age_max_ms` covers this window (seconds).
const MAX_WINDOW: f64 = 1.0;
/// The rate is measured over windows at least this long (seconds).
const RATE_WINDOW: f64 = 0.5;
/// No contact sample for this long (seconds) → rate 0.
const IDLE: f64 = 1.0;

/// Folds each frame's drained batch into [`PenStats`]. All times are on the
/// [`crate::now_secs`] clock.
#[derive(Debug, Default)]
pub struct PenMeter {
    stats: PenStats,
    /// `age_ms` has a value to smooth from.
    has_age: bool,
    /// Start of the `age_max_ms` window.
    max_since: f64,
    /// Time of the first contact sample in the rate window and how many followed it.
    rate_start: Option<f64>,
    rate_count: u32,
    /// Time of the newest contact sample.
    rate_last: f64,
}

impl PenMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Account for `batch` (this frame's samples, oldest first) consumed at `now`.
    pub fn update(&mut self, batch: &[PenSample], now: f64, dropped: u32, native: bool) -> PenStats {
        let st = &mut self.stats;
        if now - self.max_since >= MAX_WINDOW {
            self.max_since = now;
            st.age_max_ms = 0.0;
        }
        if let Some(newest) = batch.last() {
            let age = ((now - newest.time) * 1000.0).max(0.0) as f32;
            st.age_ms = if self.has_age { st.age_ms + AGE_ALPHA * (age - st.age_ms) } else { age };
            self.has_age = true;
            st.age_max_ms = st.age_max_ms.max(age);
        }
        // Hover is coalesced in the queue, so only contact samples show the device rate.
        for s in batch.iter().filter(|s| matches!(s.phase, PenPhase::Down | PenPhase::Move | PenPhase::Up)) {
            match self.rate_start {
                Some(t0) if s.time > t0 => {
                    self.rate_count += 1;
                    let span = s.time - t0;
                    if span >= RATE_WINDOW {
                        st.rate_hz = (f64::from(self.rate_count) / span) as f32;
                        self.rate_start = Some(s.time);
                        self.rate_count = 0;
                    }
                }
                Some(_) => {}
                None => {
                    self.rate_start = Some(s.time);
                    self.rate_count = 0;
                }
            }
            self.rate_last = s.time;
        }
        if self.rate_start.is_some() && now - self.rate_last >= IDLE {
            self.rate_start = None;
            st.rate_hz = 0.0;
        }
        st.native = native;
        st.dropped = dropped;
        *st
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moves(t0: f64, hz: f64, n: usize, phase: PenPhase) -> Vec<PenSample> {
        (0..n).map(|i| PenSample { phase, time: t0 + i as f64 / hz, ..Default::default() }).collect()
    }

    /// P16
    #[test]
    fn meter_rate_age_and_idle_reset() {
        let mut m = PenMeter::new();
        let st = m.update(&[], 10.0, 0, true);
        assert_eq!((st.rate_hz, st.age_ms, st.native), (0.0, 0.0, true));

        // 200 Hz contact samples, consumed 2 ms after the newest one, in 60 Hz frames.
        let all = moves(10.0, 200.0, 400, PenPhase::Move);
        let mut st = PenStats::default();
        for chunk in all.chunks(3) {
            let now = chunk.last().unwrap().time + 0.002;
            st = m.update(chunk, now, 3, true);
        }
        assert!((st.rate_hz - 200.0).abs() < 1.0, "{st:?}");
        assert!((st.age_ms - 2.0).abs() < 1e-3, "{st:?}");
        assert!((st.age_max_ms - 2.0).abs() < 1e-3);
        assert_eq!(st.dropped, 3);

        // One stale batch raises the max at once and the smoothed age by α.
        let last = all.last().unwrap().time;
        let late = PenSample { phase: PenPhase::Move, time: last + 0.005, ..Default::default() };
        let st = m.update(&[late], late.time + 0.012, 3, true);
        assert!((st.age_max_ms - 12.0).abs() < 1e-3);
        assert!((st.age_ms - (2.0 + 0.2 * 10.0)).abs() < 1e-3, "{st:?}");

        // Hover samples do not count towards the rate.
        let mut h = PenMeter::new();
        let st = h.update(&moves(0.0, 1000.0, 900, PenPhase::Hover), 0.9, 0, true);
        assert_eq!(st.rate_hz, 0.0);

        // Idle for a second: rate drops to 0 and the max window resets.
        let st = m.update(&[], late.time + 1.1, 0, false);
        assert_eq!(st.rate_hz, 0.0);
        assert!(!st.native);
        let st = m.update(&[], late.time + 2.5, 0, false);
        assert_eq!(st.age_max_ms, 0.0);
    }
}
