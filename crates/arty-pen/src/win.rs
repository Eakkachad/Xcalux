//! Windows integration: a comctl32 subclass on the root window reads the
//! pen's pointer history before winit's window proc sees the message. This
//! is the only unsafe code in the crate. Every `// SAFETY:` comment names the
//! invariant it relies on:
//!
//! - **I1 Thread affinity.** `install_hwnd`, `pen_proc` and `RemoveWindowSubclass`
//!   run on the thread that created `hwnd` (comctl32 subclassing requires it).
//!   `ProcState` holds `Rc`/`Cell`/`RefCell`, so it is `!Send` and cannot leave the thread.
//! - **I2 Ownership.** The `ProcState` box is owned by the subclass from a successful
//!   `SetWindowSubclass` until the `WM_NCDESTROY` arm frees it, exactly once, after
//!   `RemoveWindowSubclass` (comctl32 never passes `data` again). If installation fails,
//!   `install_hwnd` frees it at once. Nothing else frees it. The app's `Rc<PenQueue>`
//!   keeps the queue alive independently, in either destruction order.
//! - **I3 Aliasing.** Only `&ProcState` is ever made from `data`, and it never lives
//!   across a call that can dispatch messages (`DefSubclassProc`). All mutation goes
//!   through `Cell`/`RefCell::try_borrow_mut`, so a re-entrant call cannot panic or alias
//!   a `&mut`. The `Cell<Vec>` take/put pattern hands a re-entrant call an empty Vec,
//!   which is correct but may allocate (rare).
//! - **I4 FFI buffers.** `buf` holds `n` initialised `POINTER_PEN_INFO` (zeroed is valid)
//!   and `got == n` on input, so the OS writes at most `n` entries; only `got.min(n)`
//!   are read. Out-params are live locals. `msg`/`wp`/`lp` are forwarded unmodified.
//! - **I5 No panic.** No `unwrap`/`expect`/unchecked indexing/overflowing arithmetic in
//!   `pen_proc` or `capture` (nor in the tracker and queue they call).
//! - **I6 Transparency.** Every message is forwarded through `DefSubclassProc` and its
//!   result returned, so winit/egui behave exactly as before.
//! - **I7 No egui or message pumping from the proc.** It never repaints, opens dialogs
//!   or pumps messages; the canvas drains the queue on the next frame.
//! - **I8 Single install per window,** enforced with the thread-local `HOOKED` list.
//!   (`GetWindowSubclass` would do, but the comctl32 5.82 in System32, which an exe
//!   without a common-controls v6 manifest loads, does not export it by name: importing
//!   it makes the process fail to start with STATUS_ENTRYPOINT_NOT_FOUND.)

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::UI::Controls::{
    FEEDBACK_PEN_BARRELVISUALIZATION, FEEDBACK_PEN_DOUBLETAP, FEEDBACK_PEN_PRESSANDHOLD, FEEDBACK_PEN_RIGHTTAP,
    FEEDBACK_PEN_TAP, SetWindowFeedbackSetting,
};
use windows_sys::Win32::UI::Input::Pointer::{
    GetPointerDeviceRects, GetPointerPenInfo, GetPointerPenInfoHistory, GetPointerType, POINTER_PEN_INFO,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    POINTER_INPUT_TYPE, PT_PEN, WM_NCDESTROY, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERLEAVE, WM_POINTERUP,
    WM_POINTERUPDATE,
};
use windows_sys::core::BOOL;

use crate::PenQueue;
use crate::track::{DeviceMap, PenTracker, RawPen};

/// Subclass id ("ARTY").
const SUBCLASS_ID: usize = 0x4152_5459;
/// Most history entries read per message.
const MAX_HISTORY: u32 = 256;
/// About 5 s of samples at 200 Hz.
const QUEUE_CAP: usize = 1024;
/// Initial capacity of the per-message scratch buffers.
const SCRATCH_CAP: usize = 64;

thread_local! {
    /// Windows subclassed by this thread (I1, I8), removed at WM_NCDESTROY.
    static HOOKED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

struct ProcState {
    queue: Rc<PenQueue>,
    tracker: RefCell<PenTracker>,
    scratch: Cell<Vec<POINTER_PEN_INFO>>,
    raws: Cell<Vec<RawPen>>,
}

pub(crate) fn install(window: &impl HasWindowHandle) -> Option<Rc<PenQueue>> {
    let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: I1 — eframe's root window, created by this (event-loop) thread, alive
    // during App creation, which is when this is called.
    unsafe { install_hwnd(h.hwnd.get() as HWND) }
}

/// Subclass `hwnd` and return the queue its pen samples go to; `None` when
/// already installed (I8) or when comctl32 refuses.
///
/// # Safety
/// `hwnd` is a live window created by the calling thread.
pub(crate) unsafe fn install_hwnd(hwnd: HWND) -> Option<Rc<PenQueue>> {
    let key = hwnd as usize;
    if HOOKED.with_borrow(|h| h.contains(&key)) {
        return None; // I8
    }
    let queue = Rc::new(PenQueue::new(QUEUE_CAP));
    let state = Box::into_raw(Box::new(ProcState {
        queue: queue.clone(),
        tracker: RefCell::new(PenTracker::new(qpc_hz())),
        scratch: Cell::new(Vec::with_capacity(SCRATCH_CAP)),
        raws: Cell::new(Vec::with_capacity(SCRATCH_CAP)),
    }));
    // SAFETY: I1 — same thread as `hwnd`'s creator. I2 — on success the subclass owns
    // `state` until WM_NCDESTROY.
    if unsafe { SetWindowSubclass(hwnd, Some(pen_proc), SUBCLASS_ID, state as usize) } == 0 {
        // SAFETY: I2 — not installed, so nothing else holds `state`; freed exactly once here.
        drop(unsafe { Box::from_raw(state) });
        return None;
    }
    HOOKED.with_borrow_mut(|h| h.push(key));
    disable_pen_feedback(hwnd);
    Some(queue)
}

/// Turn off the Windows Ink tap ripple, press-and-hold and barrel feedback,
/// which would lag the pen and fire right clicks. Failures are only logged.
fn disable_pen_feedback(hwnd: HWND) {
    let off: BOOL = 0;
    for f in [
        FEEDBACK_PEN_TAP,
        FEEDBACK_PEN_DOUBLETAP,
        FEEDBACK_PEN_PRESSANDHOLD,
        FEEDBACK_PEN_RIGHTTAP,
        FEEDBACK_PEN_BARRELVISUALIZATION,
    ] {
        // SAFETY: I1 — `hwnd` is the window just subclassed on this thread (an invalid
        // handle only makes the call fail). The configuration points at a live BOOL of
        // the size passed.
        let ok = unsafe { SetWindowFeedbackSetting(hwnd, f, 0, size_of::<BOOL>() as u32, (&raw const off).cast()) };
        if ok == 0 {
            log::debug!("SetWindowFeedbackSetting({f}) failed");
        }
    }
}

/// QPC ticks per second (0 if unavailable, which makes the tracker use `dwTime`).
fn qpc_hz() -> f64 {
    static HZ: OnceLock<f64> = OnceLock::new();
    *HZ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: I4 — `f` is a live out-param.
        let ok = unsafe { QueryPerformanceFrequency(&mut f) };
        if ok != 0 && f > 0 { f as f64 } else { 0.0 }
    })
}

pub(crate) fn now_secs() -> f64 {
    let hz = qpc_hz();
    let mut c = 0i64;
    // SAFETY: I4 — `c` is a live out-param.
    let ok = unsafe { QueryPerformanceCounter(&mut c) };
    if ok != 0 && hz > 0.0 { c as f64 / hz } else { 0.0 }
}

/// GET_POINTERID_WPARAM.
fn pointer_id(wp: WPARAM) -> u32 {
    (wp & 0xFFFF) as u32
}

unsafe extern "system" fn pen_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
    if msg == WM_NCDESTROY {
        // SAFETY: I1 — comctl32 calls the proc on `hwnd`'s thread. I6 — forwarded unmodified.
        let r = unsafe {
            RemoveWindowSubclass(hwnd, Some(pen_proc), SUBCLASS_ID);
            DefSubclassProc(hwnd, msg, wp, lp)
        };
        // I5: `try_*` so TLS teardown or a held borrow cannot panic here.
        let _ = HOOKED.try_with(|h| {
            if let Ok(mut h) = h.try_borrow_mut() {
                h.retain(|&w| w != hwnd as usize);
            }
        });
        if data != 0 {
            // SAFETY: I2 — `data` is the box handed to SetWindowSubclass; the subclass is
            // removed, so comctl32 never passes it again and this is the only free.
            // I3 — no `&ProcState` is alive here (outer frames drop theirs before
            // calling DefSubclassProc).
            drop(unsafe { Box::from_raw(data as *mut ProcState) });
        }
        return r;
    }
    if data != 0 {
        // SAFETY: I2 — `data` points at the live ProcState until WM_NCDESTROY, handled
        // above. I3 — shared reference only, dropped before DefSubclassProc below.
        let state = unsafe { &*(data as *const ProcState) };
        state.on_message(hwnd, msg, wp);
    }
    // SAFETY: I1, I6 — every message is forwarded unmodified and its result returned.
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

impl ProcState {
    fn on_message(&self, hwnd: HWND, msg: u32, wp: WPARAM) {
        let id = pointer_id(wp);
        match msg {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
                if self.queue.enabled() {
                    self.capture(hwnd, id);
                } else if let Ok(mut t) = self.tracker.try_borrow_mut() {
                    t.reset();
                }
            }
            WM_POINTERCAPTURECHANGED => {
                if let Ok(mut t) = self.tracker.try_borrow_mut() {
                    t.cancel(id, &self.queue);
                }
            }
            WM_POINTERLEAVE => {
                if let Ok(mut t) = self.tracker.try_borrow_mut() {
                    t.leave(id, &self.queue);
                }
            }
            _ => {}
        }
    }

    /// Read pointer `id`'s pen history and queue it. Mouse never gets here
    /// (`EnableMouseInPointer` is never called) and touch arrives as WM_TOUCH.
    fn capture(&self, hwnd: HWND, id: u32) {
        let mut ty: POINTER_INPUT_TYPE = 0;
        // SAFETY: I4 — `ty` is a live out-param; an unknown id only makes the call fail.
        if unsafe { GetPointerType(id, &mut ty) } == 0 || ty != PT_PEN {
            return;
        }
        let mut cur = POINTER_PEN_INFO::default();
        // SAFETY: I4 — `cur` is a live out-param.
        if unsafe { GetPointerPenInfo(id, &mut cur) } == 0 {
            return;
        }
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: I1 — `hwnd` is the live window whose proc this is; I4 — `origin` is a
        // live in/out-param.
        if unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
            return; // positions would be wrong; the canvas's Touch safety net ends a stroke
        }
        let mut map = DeviceMap { client_origin: [origin.x, origin.y], ..DeviceMap::default() };
        let (mut dev, mut disp) = (RECT::default(), RECT::default());
        // SAFETY: I4 — `dev`/`disp` are live out-params; a stale device handle only fails.
        if unsafe { GetPointerDeviceRects(cur.pointerInfo.sourceDevice, &mut dev, &mut disp) } != 0 {
            map.device = [dev.left, dev.top, dev.right, dev.bottom];
            map.display = [disp.left, disp.top, disp.right, disp.bottom];
        }

        let n = cur.pointerInfo.historyCount.clamp(1, MAX_HISTORY);
        let mut buf = self.scratch.take();
        buf.clear();
        buf.resize(n as usize, POINTER_PEN_INFO::default());
        let mut got = n;
        // SAFETY: I4 — `buf` holds `n` initialised entries and `got == n`, so the OS
        // writes at most `n`; only `got.min(n)` are read below.
        let ok = unsafe { GetPointerPenInfoHistory(id, &mut got, buf.as_mut_ptr()) } != 0;
        let entries = if ok { buf.get(..got.min(n) as usize).unwrap_or_default() } else { std::slice::from_ref(&cur) };

        let mut raws = self.raws.take();
        raws.clear();
        raws.extend(entries.iter().map(raw_pen));
        if let Ok(mut t) = self.tracker.try_borrow_mut() {
            t.process(&raws, &map, now_secs(), &self.queue);
        }
        self.raws.set(raws);
        self.scratch.set(buf);
    }
}

/// Copy the fields the tracker uses (himetric, not the untransformed `Raw` locations).
fn raw_pen(p: &POINTER_PEN_INFO) -> RawPen {
    let i = &p.pointerInfo;
    RawPen {
        pointer: i.pointerId,
        frame: i.frameId,
        flags: i.pointerFlags,
        pen_flags: p.penFlags,
        pen_mask: p.penMask,
        pressure: p.pressure,
        tilt: [p.tiltX, p.tiltY],
        pixel: [i.ptPixelLocation.x, i.ptPixelLocation.y],
        himetric: [i.ptHimetricLocation.x, i.ptHimetricLocation.y],
        perf_count: i.PerformanceCount,
        time_ms: i.dwTime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SendMessageW, WM_GETTEXTLENGTH};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// P15: sends window messages to a private, never-shown window only; the
    /// cursor and the OS input queue are never touched.
    #[test]
    fn subclass_forwards_and_frees_state() {
        let (class, title) = (wide("STATIC"), wide("arty"));
        // SAFETY: I1 — the window is created and used on this test thread only; the
        // strings are NUL-terminated and outlive the call.
        let hwnd = unsafe {
            CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0, 0, 0, 16, 16, null_mut(), null_mut(), null_mut(), null())
        };
        assert!(!hwnd.is_null());
        // SAFETY: I1 — live window created by this thread.
        let q = unsafe { install_hwnd(hwnd) }.expect("subclass installs");
        // SAFETY: as above.
        assert!(unsafe { install_hwnd(hwnd) }.is_none(), "second install is refused (I8)");
        assert_eq!(Rc::strong_count(&q), 2);

        // SAFETY: I1 — synchronous messages to this thread's own window.
        unsafe {
            // A bogus pointer id: GetPointerType fails, the message is forwarded, nothing is queued.
            SendMessageW(hwnd, WM_POINTERUPDATE, 0xFFFE, 0);
            SendMessageW(hwnd, WM_POINTERDOWN, 0xFFFE, 0);
            SendMessageW(hwnd, WM_POINTERCAPTURECHANGED, 0xFFFE, 0);
            SendMessageW(hwnd, WM_POINTERLEAVE, 0xFFFE, 0);
            q.set_enabled(false);
            SendMessageW(hwnd, WM_POINTERUP, 0xFFFE, 0);
        }
        assert!(q.is_empty());
        // Other messages reach the original window proc and its result comes back (I6).
        // SAFETY: as above.
        assert_eq!(unsafe { SendMessageW(hwnd, WM_GETTEXTLENGTH, 0, 0) }, 4);

        // SAFETY: as above; WM_NCDESTROY runs synchronously inside DestroyWindow.
        assert_ne!(unsafe { DestroyWindow(hwnd) }, 0);
        assert_eq!(Rc::strong_count(&q), 1, "WM_NCDESTROY freed the ProcState (I2)");
    }
}
