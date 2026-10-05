//! Native pen input. On Windows a comctl32 subclass reads the full Windows Ink pointer
//! history (per-sample pressure, tilt, eraser, QPC time) into a [`PenQueue`] that the
//! canvas drains once per frame. Everything except `win.rs` is portable and unit-tested.

use std::rc::Rc;

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

/// Fixed-capacity sample ring shared by the window proc and the canvas (same thread).
pub struct PenQueue {/* TRACK PEN */}

impl PenQueue {
    pub fn new(cap: usize) -> Self {
        let _ = cap;
        todo!("TRACK PEN")
    }
    pub fn push(&self, s: PenSample) {
        let _ = s;
        todo!("TRACK PEN")
    }
    pub fn drain_into(&self, out: &mut Vec<PenSample>) {
        let _ = out;
        todo!("TRACK PEN")
    }
    pub fn set_enabled(&self, on: bool) {
        let _ = on;
        todo!("TRACK PEN")
    }
    pub fn enabled(&self) -> bool {
        todo!("TRACK PEN")
    }
    pub fn dropped(&self) -> u32 {
        todo!("TRACK PEN")
    }
}

/// Install the pen hook on `window`'s root HWND. Call once, on the window's thread.
/// `None` on non-Windows platforms or when installation fails (the canvas then uses egui Touch).
pub fn install(window: &impl raw_window_handle::HasWindowHandle) -> Option<Rc<PenQueue>> {
    let _ = window;
    None
}

/// Seconds on the clock `PenSample::time` uses (QPC on Windows).
pub fn now_secs() -> f64 {
    0.0
}
