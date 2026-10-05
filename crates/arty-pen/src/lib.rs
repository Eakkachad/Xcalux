//! Native pen input. On Windows a comctl32 subclass reads the full Windows Ink pointer
//! history (per-sample pressure, tilt, eraser, QPC time) into a [`PenQueue`] that the
//! canvas drains once per frame. Everything except `win.rs` is portable and unit-tested.

use std::rc::Rc;

mod meter;
mod queue;
pub mod track;
#[cfg(windows)]
mod win;

pub use meter::PenMeter;
pub use queue::PenQueue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PenEnd {
    #[default]
    Tip = 0,
    Eraser = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PenPhase {
    #[default]
    Hover,
    Down,
    Move,
    Up,
    Cancel,
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PenSample {
    /// Windows pointer id; equals egui `TouchId.0` for the same contact.
    pub pointer: u32,
    pub phase: PenPhase,
    pub end: PenEnd,
    pub barrel: bool,
    /// Client-area physical pixels (= egui points × pixels_per_point).
    pub pos: [f32; 2],
    /// 0..=1; `None` = the device reports no pressure axis.
    pub pressure: Option<f32>,
    /// MyPaint convention in screen axes: ±1 = 60°; [0, 0] = no tilt.
    pub tilt: [f32; 2],
    /// Seconds on the [`now_secs`] clock (QPC), strictly increasing per tracker.
    pub time: f64,
}

/// Read-only figures for the latency overlay. Public fields only (other crates build it in tests).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PenStats {
    /// The native queue is installed and enabled.
    pub native: bool,
    /// Pen samples per second over the last ~0.5 s of pen activity; 0 when idle.
    pub rate_hz: f32,
    /// Newest sample's age when the canvas consumed it (OS timestamp → frame), ms, smoothed.
    pub age_ms: f32,
    /// Largest such age during the last second, ms.
    pub age_max_ms: f32,
    /// Samples dropped because the UI stalled (ring full).
    pub dropped: u32,
}

/// Install the pen hook on `window`'s root HWND. Call once, on the window's thread.
/// `None` on non-Windows platforms or when installation fails (the canvas then uses egui Touch).
pub fn install(window: &impl raw_window_handle::HasWindowHandle) -> Option<Rc<PenQueue>> {
    #[cfg(windows)]
    {
        let queue = win::install(window);
        match queue {
            Some(_) => log::info!("native pen hook installed (Windows Ink pointer history)"),
            None => log::warn!("native pen hook not installed; pen input uses egui touch events"),
        }
        queue
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// Seconds on the clock `PenSample::time` uses (QPC on Windows).
pub fn now_secs() -> f64 {
    #[cfg(windows)]
    {
        win::now_secs()
    }
    #[cfg(not(windows))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64()
    }
}

/// Rotate/flip a screen-space tilt into document space with the view's linear part, keeping its magnitude.
///
/// `lin` is `[a, b, c, d]` of the screen→document matrix. `[0, 0]` stays
/// exactly `[0, 0]`, which hokusai reads as "no tilt".
pub fn view_tilt(lin: [f32; 4], t: [f32; 2]) -> [f32; 2] {
    let mag = t[0].hypot(t[1]);
    if mag == 0.0 {
        return [0.0, 0.0];
    }
    let v = [lin[0] * t[0] + lin[1] * t[1], lin[2] * t[0] + lin[3] * t[1]];
    let n = v[0].hypot(v[1]);
    if !(n.is_finite() && n > 0.0) {
        return t;
    }
    [v[0] * mag / n, v[1] * mag / n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    /// P14
    #[test]
    fn view_tilt_rotation_flip_and_zero() {
        let t = [0.6, 0.0];
        // Zoom alone changes nothing (magnitude is kept).
        assert!(close(view_tilt([0.25, 0.0, 0.0, 0.25], t), t));
        // A 90° rotation (scaled) turns +x into +y.
        let (s, c) = 90f32.to_radians().sin_cos();
        let z = 0.5;
        assert!(close(view_tilt([c * z, -s * z, s * z, c * z], t), [0.0, 0.6]));
        // A horizontal flip mirrors x.
        assert!(close(view_tilt([-1.0, 0.0, 0.0, 1.0], [0.3, 0.4]), [-0.3, 0.4]));
        // No tilt stays exactly zero, sign included.
        let zero = view_tilt([0.0, -1.0, 1.0, 0.0], [0.0, 0.0]);
        assert_eq!(zero.map(f32::to_bits), [0, 0]);
        let neg = view_tilt([-1.0, 0.0, 0.0, 1.0], [-0.0, 0.0]);
        assert_eq!(neg.map(f32::to_bits), [0, 0]);
        // A degenerate matrix passes the tilt through.
        assert_eq!(view_tilt([0.0; 4], [0.1, 0.2]), [0.1, 0.2]);
        assert_eq!(view_tilt([f32::NAN, 0.0, 0.0, 1.0], [0.1, 0.2]), [0.1, 0.2]);
    }

    #[test]
    fn clock_advances() {
        let a = now_secs();
        let b = now_secs();
        assert!(a.is_finite() && b >= a);
    }
}
