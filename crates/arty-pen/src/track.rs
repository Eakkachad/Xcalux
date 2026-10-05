//! Windows Ink pointer history → [`PenSample`]s, without touching the OS.
//!
//! `win.rs` copies the `POINTER_PEN_INFO` fields it needs into [`RawPen`]s
//! and hands them to a [`PenTracker`], which orders them, drops repeats,
//! derives the contact phase and converts units. Everything here runs
//! inside the window proc, so it never panics: no indexing, no unchecked
//! arithmetic.

use crate::{PenEnd, PenPhase, PenQueue, PenSample};

/// `POINTER_INFO::pointerFlags`: the pointer touches the digitizer.
pub const POINTER_FLAG_INCONTACT: u32 = 0x4;
/// `POINTER_INFO::pointerFlags`: the pointer departs abnormally.
pub const POINTER_FLAG_CANCELED: u32 = 0x8000;
/// `POINTER_PEN_INFO::penFlags`: barrel button pressed.
pub const PEN_FLAG_BARREL: u32 = 1;
/// `POINTER_PEN_INFO::penFlags`: the pen is upside down (eraser end), set while hovering too.
pub const PEN_FLAG_INVERTED: u32 = 2;
/// `POINTER_PEN_INFO::penFlags`: the eraser end touches the digitizer.
pub const PEN_FLAG_ERASER: u32 = 4;
/// `POINTER_PEN_INFO::penMask`: `pressure` is valid.
pub const PEN_MASK_PRESSURE: u32 = 1;
/// `POINTER_PEN_INFO::penMask`: `tiltX` is valid.
pub const PEN_MASK_TILT_X: u32 = 4;
/// `POINTER_PEN_INFO::penMask`: `tiltY` is valid.
pub const PEN_MASK_TILT_Y: u32 = 8;

#[cfg(windows)]
const _: () = {
    use windows_sys::Win32::UI::Input::Pointer as p;
    use windows_sys::Win32::UI::WindowsAndMessaging as w;
    assert!(POINTER_FLAG_INCONTACT == p::POINTER_FLAG_INCONTACT);
    assert!(POINTER_FLAG_CANCELED == p::POINTER_FLAG_CANCELED);
    assert!(PEN_FLAG_BARREL == w::PEN_FLAG_BARREL);
    assert!(PEN_FLAG_INVERTED == w::PEN_FLAG_INVERTED);
    assert!(PEN_FLAG_ERASER == w::PEN_FLAG_ERASER);
    assert!(PEN_MASK_PRESSURE == w::PEN_MASK_PRESSURE);
    assert!(PEN_MASK_TILT_X == w::PEN_MASK_TILT_X);
    assert!(PEN_MASK_TILT_Y == w::PEN_MASK_TILT_Y);
};

/// Windows Ink reports pressure as 0..=1024.
const PRESSURE_MAX: u32 = 1024;
/// Tilt in degrees that maps to ±1 (MyPaint convention).
const TILT_FULL_DEG: f32 = 60.0;
/// Step the monotonic guard adds when a timestamp does not advance.
const MIN_STEP_SECS: f64 = 0.0005;
/// Mapped and OS pixel positions may disagree by this much before the mapping is distrusted.
const MAX_MAP_ERROR_PX: f64 = 2.0;

/// OS-free copy of the POINTER_PEN_INFO fields used (tests build these by hand).
#[derive(Debug, Clone, Copy, Default)]
pub struct RawPen {
    pub pointer: u32,
    pub frame: u32,
    pub flags: u32,
    pub pen_flags: u32,
    pub pen_mask: u32,
    pub pressure: u32,
    pub tilt: [i32; 2],
    pub pixel: [i32; 2],
    pub himetric: [i32; 2],
    pub perf_count: u64,
    pub time_ms: u32,
}

/// Rects as [left, top, right, bottom]; device rect in himetric, display rect in screen px.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceMap {
    pub device: [i32; 4],
    pub display: [i32; 4],
    pub client_origin: [i32; 2],
}

/// Client-area pixel position of `r`, sub-pixel when the device rects allow.
///
/// `ptPixelLocation` is rounded to whole pixels; the himetric position maps
/// through the device rects to sub-pixel screen coordinates (subtracting the
/// device rect's origin, which winit forgets). When the rects are missing or
/// the mapped point strays over 2 px from the OS pixel position (bad driver
/// rects, rotated screens), the pixel position is used.
pub fn to_client(r: &RawPen, m: &DeviceMap) -> [f32; 2] {
    let span = |a: i32, b: i32| f64::from(b) - f64::from(a);
    let (dw, dh) = (span(m.device[0], m.device[2]), span(m.device[1], m.device[3]));
    let (pw, ph) = (span(m.display[0], m.display[2]), span(m.display[1], m.display[3]));
    let (px, py) = (f64::from(r.pixel[0]), f64::from(r.pixel[1]));
    let (mut sx, mut sy) = (px, py);
    if dw > 0.0 && dh > 0.0 && pw > 0.0 && ph > 0.0 {
        let mx = f64::from(m.display[0]) + span(m.device[0], r.himetric[0]) * pw / dw;
        let my = f64::from(m.display[1]) + span(m.device[1], r.himetric[1]) * ph / dh;
        if (mx - px).abs() <= MAX_MAP_ERROR_PX && (my - py).abs() <= MAX_MAP_ERROR_PX {
            (sx, sy) = (mx, my);
        }
    }
    [(sx - f64::from(m.client_origin[0])) as f32, (sy - f64::from(m.client_origin[1])) as f32]
}

/// Turns pointer history into phased, timestamped [`PenSample`]s.
///
/// The phase comes only from `POINTER_FLAG_INCONTACT` transitions between
/// consecutive entries, never from message ids or the DOWN/UP flags, which
/// coalesced history entries do not carry.
pub struct PenTracker {
    /// Pointer currently touching the digitizer.
    contact: Option<u32>,
    /// Newest `(pointer, frame)` processed, for dropping repeated history.
    last_frame: Option<(u32, u32)>,
    /// Last sample pushed.
    last: Option<PenSample>,
    /// Last sample pushed for the contact pointer (what a Cancel repeats).
    contact_last: Option<PenSample>,
    /// Newest timestamp handed out (seconds).
    last_time: f64,
    /// QPC ticks per second.
    qpc_hz: f64,
    /// `(dwTime, now)` anchoring the millisecond clock when QPC counts are missing.
    ms_base: Option<(u32, f64)>,
}

impl PenTracker {
    pub fn new(qpc_hz: f64) -> Self {
        Self {
            contact: None,
            last_frame: None,
            last: None,
            contact_last: None,
            last_time: f64::NEG_INFINITY,
            qpc_hz,
            ms_base: None,
        }
    }

    /// `newest_first` exactly as GetPointerPenInfoHistory returns it; returns samples pushed.
    /// `now` is [`crate::now_secs`], used only to anchor the `dwTime` fallback clock.
    pub fn process(&mut self, newest_first: &[RawPen], map: &DeviceMap, now: f64, q: &PenQueue) -> usize {
        let mut pushed = 0;
        for r in newest_first.iter().rev() {
            // 1. Dedupe: overlapping history repeats frames already seen.
            if let Some((p, f)) = self.last_frame
                && p == r.pointer
                && (r.frame.wrapping_sub(f) as i32) <= 0
            {
                continue;
            }
            self.last_frame = Some((r.pointer, r.frame));

            // 2. Phase from contact transitions.
            let in_c = r.flags & POINTER_FLAG_INCONTACT != 0;
            let canceled = r.flags & POINTER_FLAG_CANCELED != 0;
            let was = self.contact == Some(r.pointer);
            let phase = if canceled && was {
                self.contact = None;
                PenPhase::Cancel
            } else if in_c && !was && !canceled {
                if let Some(other) = self.contact {
                    pushed += self.cancel(other, q);
                }
                self.contact = Some(r.pointer);
                PenPhase::Down
            } else if in_c && was {
                PenPhase::Move
            } else if was {
                self.contact = None;
                PenPhase::Up
            } else {
                PenPhase::Hover
            };

            let s = PenSample {
                pointer: r.pointer,
                phase,
                // 3. End and barrel.
                end: if r.pen_flags & (PEN_FLAG_INVERTED | PEN_FLAG_ERASER) != 0 { PenEnd::Eraser } else { PenEnd::Tip },
                barrel: r.pen_flags & PEN_FLAG_BARREL != 0,
                // 7. Position.
                pos: to_client(r, map),
                // 4. Pressure: a light contact stays light (never "unknown = full").
                pressure: (r.pen_mask & PEN_MASK_PRESSURE != 0)
                    .then(|| r.pressure.min(PRESSURE_MAX) as f32 / PRESSURE_MAX as f32),
                // 5. Tilt.
                tilt: [tilt_axis(r, PEN_MASK_TILT_X, r.tilt[0]), tilt_axis(r, PEN_MASK_TILT_Y, r.tilt[1])],
                // 6. Time.
                time: self.time_of(r, now),
            };
            // 8. Push.
            q.push(s);
            self.last = Some(s);
            if self.contact == Some(r.pointer) {
                self.contact_last = Some(s);
            } else if self.contact.is_none() {
                self.contact_last = None;
            }
            pushed += 1;
        }
        pushed
    }

    /// The pointer lost capture (WM_POINTERCAPTURECHANGED): end its contact
    /// with a Cancel that repeats its last sample (position and time).
    /// Returns the samples pushed.
    pub fn cancel(&mut self, pointer: u32, q: &PenQueue) -> usize {
        if self.contact != Some(pointer) {
            return 0;
        }
        self.contact = None;
        match self.contact_last.take() {
            Some(s) => {
                let s = PenSample { phase: PenPhase::Cancel, ..s };
                q.push(s);
                self.last = Some(s);
                1
            }
            None => 0,
        }
    }

    /// The pointer left the window (WM_POINTERLEAVE): cancel a contact, then
    /// push a Leave that repeats the pointer's last sample. Returns the samples pushed.
    pub fn leave(&mut self, pointer: u32, q: &PenQueue) -> usize {
        let mut pushed = self.cancel(pointer, q);
        if let Some(s) = self.last.filter(|s| s.pointer == pointer) {
            let s = PenSample { phase: PenPhase::Leave, ..s };
            q.push(s);
            self.last = Some(s);
            pushed += 1;
        }
        pushed
    }

    /// Forget contact and history (the queue was disabled). The clock stays monotonic.
    pub fn reset(&mut self) {
        self.contact = None;
        self.last_frame = None;
        self.last = None;
        self.contact_last = None;
    }

    /// Seconds on the QPC clock, strictly increasing.
    fn time_of(&mut self, r: &RawPen, now: f64) -> f64 {
        let mut t = if r.perf_count != 0 && self.qpc_hz > 0.0 {
            r.perf_count as f64 / self.qpc_hz
        } else {
            // dwTime fallback: milliseconds since boot, wrapping every ~49.7 days.
            let (base_ms, base_s) = *self.ms_base.get_or_insert((r.time_ms, now));
            // Signed difference: wraps correctly and an entry older than the
            // anchor goes backwards (then the guard below) instead of ~49 days forward.
            base_s + f64::from(r.time_ms.wrapping_sub(base_ms) as i32) / 1000.0
        };
        if t.is_nan() || t <= self.last_time {
            t = self.last_time + MIN_STEP_SECS;
        }
        self.last_time = t;
        t
    }
}

fn tilt_axis(r: &RawPen, mask: u32, deg: i32) -> f32 {
    if r.pen_mask & mask != 0 { (deg as f32 / TILT_FULL_DEG).clamp(-1.0, 1.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IN: u32 = POINTER_FLAG_INCONTACT;
    const ALL: u32 = PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_TILT_Y;
    const HZ: f64 = 10_000_000.0;

    /// Pointer 1 at frame `frame`, x = frame, QPC time = frame ms.
    fn raw(frame: u32, flags: u32) -> RawPen {
        RawPen {
            pointer: 1,
            frame,
            flags,
            pen_mask: ALL,
            pressure: 512,
            pixel: [frame as i32, 0],
            perf_count: 1_000_000 + u64::from(frame) * 10_000,
            ..Default::default()
        }
    }

    /// Runs one message's history (given oldest first, as people read it).
    fn run(t: &mut PenTracker, oldest_first: &[RawPen]) -> Vec<PenSample> {
        let q = PenQueue::new(64);
        let newest_first: Vec<RawPen> = oldest_first.iter().rev().copied().collect();
        let n = t.process(&newest_first, &DeviceMap::default(), 0.0, &q);
        let mut out = Vec::new();
        q.drain_into(&mut out);
        assert_eq!(n, out.len());
        out
    }

    fn phases(v: &[PenSample]) -> Vec<PenPhase> {
        v.iter().map(|s| s.phase).collect()
    }

    use PenPhase::*;

    /// P1
    #[test]
    fn history_is_processed_oldest_first() {
        let mut t = PenTracker::new(HZ);
        let out = run(&mut t, &[raw(1, IN), raw(2, IN), raw(3, IN)]);
        let xs: Vec<f32> = out.iter().map(|s| s.pos[0]).collect();
        assert_eq!(xs, [1.0, 2.0, 3.0]);
        assert_eq!(phases(&out), [Down, Move, Move]);
        assert!(out.windows(2).all(|w| w[0].time < w[1].time));
    }

    /// P2
    #[test]
    fn phase_from_incontact_transitions() {
        let mut t = PenTracker::new(HZ);
        // Coalesced history carries no DOWN/UP flags, only INCONTACT.
        let out = run(&mut t, &[raw(1, 0), raw(2, IN), raw(3, IN), raw(4, 0), raw(5, 0), raw(6, IN)]);
        assert_eq!(phases(&out), [Hover, Down, Move, Up, Hover, Down]);
        // The contact carries over to the next message.
        let out = run(&mut t, &[raw(7, IN), raw(8, 0)]);
        assert_eq!(phases(&out), [Move, Up]);
        // DOWN/UP flags (0x10000 / 0x40000) are ignored.
        let out = run(&mut t, &[raw(9, 0x10000), raw(10, 0x40000 | IN)]);
        assert_eq!(phases(&out), [Hover, Down]);
    }

    /// P3
    #[test]
    fn canceled_in_contact_gives_cancel() {
        let mut t = PenTracker::new(HZ);
        let out = run(&mut t, &[raw(1, IN), raw(2, IN | POINTER_FLAG_CANCELED), raw(3, POINTER_FLAG_CANCELED)]);
        assert_eq!(phases(&out), [Down, Cancel, Hover]);
        // A canceled entry never starts a contact.
        let out = run(&mut t, &[raw(4, IN | POINTER_FLAG_CANCELED)]);
        assert_eq!(phases(&out), [Hover]);

        // cancel()/leave() emit only while in contact, repeating the last sample.
        let q = PenQueue::new(16);
        let mut out = Vec::new();
        let mut t = PenTracker::new(HZ);
        assert_eq!(t.cancel(1, &q), 0);
        assert_eq!(t.leave(1, &q), 0, "nothing known about pointer 1 yet");
        t.process(&[raw(2, IN), raw(1, IN)], &DeviceMap::default(), 0.0, &q);
        q.drain_into(&mut out);
        let last = *out.last().unwrap();
        out.clear();
        assert_eq!(t.cancel(7, &q), 0, "another pointer");
        assert_eq!(t.cancel(1, &q), 1);
        assert_eq!(t.cancel(1, &q), 0, "contact already over");
        q.drain_into(&mut out);
        assert_eq!(out, [PenSample { phase: Cancel, ..last }]);
        out.clear();

        // leave() while in contact: Cancel then Leave.
        t.process(&[raw(3, IN)], &DeviceMap::default(), 0.0, &q);
        q.drain_into(&mut out);
        assert_eq!(phases(&out), [Down]);
        let down = out[0];
        out.clear();
        assert_eq!(t.leave(1, &q), 2);
        q.drain_into(&mut out);
        assert_eq!(out, [PenSample { phase: Cancel, ..down }, PenSample { phase: Leave, ..down }]);
        out.clear();
        // leave() while hovering: Leave only.
        t.process(&[raw(4, 0)], &DeviceMap::default(), 0.0, &q);
        q.drain_into(&mut out);
        out.clear();
        assert_eq!(t.leave(1, &q), 1);
        q.drain_into(&mut out);
        assert_eq!(phases(&out), [Leave]);
    }

    /// P4
    #[test]
    fn second_pointer_down_cancels_first() {
        let mut t = PenTracker::new(HZ);
        let first = run(&mut t, &[raw(1, IN), raw(2, IN)]);
        let mut b = raw(3, 0);
        b.pointer = 2;
        let mut b_down = raw(4, IN);
        b_down.pointer = 2;
        let out = run(&mut t, &[b, b_down]);
        assert_eq!(phases(&out), [Hover, Cancel, Down]);
        assert_eq!(out[1], PenSample { phase: Cancel, ..first[1] }, "repeats pointer 1's last sample");
        assert_eq!(out[2].pointer, 2);
        // Pointer 1's later entries no longer count as its contact.
        let out = run(&mut t, &[raw(5, IN)]);
        assert_eq!(phases(&out), [Cancel, Down], "pointer 1 back down takes over again");
    }

    /// P5
    #[test]
    fn frame_dedupe_with_u32_wraparound() {
        let mut t = PenTracker::new(HZ);
        let near = u32::MAX - 1;
        let out = run(&mut t, &[raw(near, IN), raw(u32::MAX, IN)]);
        assert_eq!(out.len(), 2);
        // Overlapping history: the two already-seen frames are dropped, the wrapped ones kept.
        let out = run(&mut t, &[raw(near, IN), raw(u32::MAX, IN), raw(0, IN), raw(1, IN)]);
        assert_eq!(out.iter().map(|s| s.pos[0]).collect::<Vec<_>>(), [0.0, 1.0]);
        assert_eq!(phases(&out), [Move, Move]);
        // A repeated message is fully skipped.
        assert!(run(&mut t, &[raw(1, IN)]).is_empty());
        // Another pointer's frames are not compared with pointer 1's.
        let mut other = raw(0, 0);
        other.pointer = 2;
        assert_eq!(run(&mut t, &[other]).len(), 1);
    }

    /// P6
    #[test]
    fn pressure_mask_zero_and_clamp() {
        let mut t = PenTracker::new(HZ);
        let p = |t: &mut PenTracker, frame: u32, mask: u32, pressure: u32| {
            let mut r = raw(frame, IN);
            r.pen_mask = mask;
            r.pressure = pressure;
            run(t, &[r])[0].pressure
        };
        assert_eq!(p(&mut t, 1, 0, 700), None);
        assert_eq!(p(&mut t, 2, PEN_MASK_PRESSURE, 0), Some(0.0), "a zero-pressure contact is not full pressure");
        assert_eq!(p(&mut t, 3, PEN_MASK_PRESSURE, 5000), Some(1.0));
        assert_eq!(p(&mut t, 4, PEN_MASK_PRESSURE, 256), Some(0.25));
    }

    /// P7
    #[test]
    fn tilt_scale_clamp_and_mask() {
        let mut t = PenTracker::new(HZ);
        let tilt = |t: &mut PenTracker, frame: u32, mask: u32, deg: [i32; 2]| {
            let mut r = raw(frame, 0);
            r.pen_mask = mask;
            r.tilt = deg;
            run(t, &[r])[0].tilt
        };
        assert_eq!(tilt(&mut t, 1, ALL, [30, -30]), [0.5, -0.5]);
        assert_eq!(tilt(&mut t, 2, ALL, [90, -90]), [1.0, -1.0]);
        assert_eq!(tilt(&mut t, 3, PEN_MASK_TILT_Y, [30, 60]), [0.0, 1.0]);
        assert_eq!(tilt(&mut t, 4, PEN_MASK_PRESSURE, [30, 60]), [0.0, 0.0]);
    }

    /// P8
    #[test]
    fn qpc_time_and_dwtime_fallback_wraparound() {
        let mut t = PenTracker::new(HZ);
        let mut r = raw(1, 0);
        r.perf_count = 25_000_000;
        assert_eq!(run(&mut t, &[r])[0].time, 2.5);

        // dwTime: anchored to `now` on first use, then wrapping milliseconds.
        let q = PenQueue::new(8);
        let mut t = PenTracker::new(HZ);
        // In contact, so the queue does not coalesce them as hover.
        let ms = |frame: u32, time_ms: u32| RawPen { pointer: 1, frame, flags: IN, time_ms, ..Default::default() };
        let newest_first = [ms(3, 5), ms(2, u32::MAX - 4), ms(1, u32::MAX - 14)];
        t.process(&newest_first, &DeviceMap::default(), 100.0, &q);
        // An older (pre-anchor) entry in a later message goes through the guard, not 49 days ahead.
        t.process(&[ms(4, u32::MAX - 100)], &DeviceMap::default(), 999.0, &q);
        let mut out = Vec::new();
        q.drain_into(&mut out);
        let times: Vec<f64> = out.iter().map(|s| s.time).collect();
        let want = [100.0, 100.010, 100.020, 100.0205];
        for (got, want) in times.iter().zip(want) {
            assert!((got - want).abs() < 1e-9, "{times:?}");
        }
        // No QPC frequency: the millisecond clock is used even with counts.
        let mut t = PenTracker::new(0.0);
        let r = RawPen { pointer: 1, frame: 1, perf_count: 123, time_ms: 7, ..Default::default() };
        t.process(&[r], &DeviceMap::default(), 3.0, &q);
        q.drain_into(&mut out);
        assert_eq!(out.last().unwrap().time, 3.0);
    }

    /// P9
    #[test]
    fn time_is_strictly_monotonic() {
        let mut t = PenTracker::new(HZ);
        let mut a = raw(1, IN);
        let mut b = raw(2, IN);
        let mut c = raw(3, IN);
        a.perf_count = 30_000;
        b.perf_count = 30_000; // same tick
        c.perf_count = 10_000; // backwards
        let mut d = raw(4, IN);
        d.perf_count = 0;
        d.time_ms = 0;
        let out = run(&mut t, &[a, b, c, d]);
        assert!(out.windows(2).all(|w| w[0].time < w[1].time), "{out:?}");
        assert!((out[1].time - out[0].time - MIN_STEP_SECS).abs() < 1e-12);
        // reset() keeps the clock monotonic.
        t.reset();
        let mut e = raw(5, 0);
        e.perf_count = 1;
        assert!(run(&mut t, &[e])[0].time > out[3].time);
    }

    /// P10
    #[test]
    fn to_client_subtracts_device_offset_and_client_origin() {
        // 2:1 himetric:px tablet mapped to a monitor left of the primary (negative origin).
        let map = DeviceMap { device: [1000, 2000, 5000, 4000], display: [-1920, 100, 0, 1180], client_origin: [-1800, 150] };
        // himetric (3000, 3000) → display px (-1920 + 2000*1920/4000, 100 + 1000*1080/2000) = (-960, 640).
        let mut r = RawPen { himetric: [3000, 3000], pixel: [-960, 640], ..Default::default() };
        assert_eq!(to_client(&r, &map), [840.0, 490.0]);
        // Sub-pixel: himetric one unit right = +0.48 px.
        r.himetric[0] += 1;
        let [x, _] = to_client(&r, &map);
        assert!((x - 840.48).abs() < 1e-3, "{x}");
        // Mapped point more than 2 px from the OS pixel position: the pixel position wins.
        r.pixel = [-900, 640];
        assert_eq!(to_client(&r, &map), [900.0, 490.0]);
        // Within 2 px: the mapping wins.
        r.pixel = [-958, 642];
        let [x, y] = to_client(&r, &map);
        assert!((x - 840.48).abs() < 1e-3 && y == 490.0);
        // Missing or degenerate rects fall back to the pixel position.
        let zero = DeviceMap { client_origin: [10, 20], ..Default::default() };
        assert_eq!(to_client(&r, &zero), [-968.0, 622.0]);
        let flat = DeviceMap { device: [0, 0, 0, 100], ..map };
        assert_eq!(to_client(&r, &flat), [-958.0 + 1800.0, 492.0]);
    }

    /// P11
    #[test]
    fn inverted_while_hovering_is_eraser() {
        let mut t = PenTracker::new(HZ);
        let mut r = raw(1, 0);
        r.pen_flags = PEN_FLAG_INVERTED;
        let s = run(&mut t, &[r])[0];
        assert_eq!((s.phase, s.end), (Hover, PenEnd::Eraser));
        let mut r = raw(2, IN);
        r.pen_flags = PEN_FLAG_ERASER;
        assert_eq!(run(&mut t, &[r])[0].end, PenEnd::Eraser);
        assert_eq!(run(&mut t, &[raw(3, IN)])[0].end, PenEnd::Tip);
    }

    /// P11
    #[test]
    fn barrel_flag() {
        let mut t = PenTracker::new(HZ);
        let mut r = raw(1, 0);
        r.pen_flags = PEN_FLAG_BARREL;
        let s = run(&mut t, &[r])[0];
        assert!(s.barrel);
        assert_eq!(s.end, PenEnd::Tip);
        assert!(!run(&mut t, &[raw(2, 0)])[0].barrel);
    }
}
