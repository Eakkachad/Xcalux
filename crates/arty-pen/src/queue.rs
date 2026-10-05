//! [`PenQueue`]: the fixed ring between the window proc (producer) and the
//! canvas (consumer). Both run on the UI thread, so plain `Cell`s suffice:
//! no locks, no unsafe, and no allocation after [`PenQueue::new`].

use std::cell::Cell;

use crate::{PenPhase, PenSample};

/// Fixed-capacity sample ring shared by the window proc and the canvas (same thread).
pub struct PenQueue {
    buf: Box<[Cell<PenSample>]>,
    /// Index of the oldest sample.
    head: Cell<usize>,
    len: Cell<usize>,
    dropped: Cell<u32>,
    enabled: Cell<bool>,
}

impl PenQueue {
    /// A ring holding up to `cap` samples (at least one), enabled.
    pub fn new(cap: usize) -> Self {
        Self {
            buf: (0..cap.max(1)).map(|_| Cell::new(PenSample::default())).collect(),
            head: Cell::new(0),
            len: Cell::new(0),
            dropped: Cell::new(0),
            enabled: Cell::new(true),
        }
    }

    /// Append `s`. A hover sample replaces a newest queued hover sample of
    /// the same pointer (only the latest hover position matters); when the
    /// ring is full the oldest sample is dropped and counted.
    ///
    /// Runs inside the window proc, so it never panics: slots are read with
    /// `get` (`head < cap` and `len <= cap` always hold anyway).
    pub fn push(&self, s: PenSample) {
        let cap = self.buf.len();
        let (head, len) = (self.head.get(), self.len.get());
        if s.phase == PenPhase::Hover
            && let Some(newest) = len.checked_sub(1).and_then(|i| self.slot(head + i))
        {
            let n = newest.get();
            if n.phase == PenPhase::Hover && n.pointer == s.pointer {
                newest.set(s);
                return;
            }
        }
        if len >= cap {
            self.head.set((head + 1) % cap);
            self.len.set(cap - 1);
            self.dropped.set(self.dropped.get().saturating_add(1));
        }
        let (head, len) = (self.head.get(), self.len.get());
        if let Some(slot) = self.slot(head + len) {
            slot.set(s);
            self.len.set(len + 1);
        }
    }

    /// Ring slot `i` (taken modulo the capacity).
    fn slot(&self, i: usize) -> Option<&Cell<PenSample>> {
        self.buf.get(i % self.buf.len())
    }

    /// Move every queued sample, oldest first, to the end of `out`.
    /// Allocates only when `out` lacks capacity.
    pub fn drain_into(&self, out: &mut Vec<PenSample>) {
        let (head, len) = (self.head.get(), self.len.get());
        out.reserve(len);
        for i in 0..len {
            if let Some(slot) = self.slot(head + i) {
                out.push(slot.get());
            }
        }
        self.head.set(0);
        self.len.set(0);
    }

    /// The window proc reads samples only while enabled (`InputSettings::native_pen`).
    pub fn set_enabled(&self, on: bool) {
        self.enabled.set(on);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }

    /// Samples dropped so far because the ring was full (saturating).
    pub fn dropped(&self) -> u32 {
        self.dropped.get()
    }

    /// Samples waiting to be drained.
    pub fn len(&self) -> usize {
        self.len.get()
    }

    pub fn is_empty(&self) -> bool {
        self.len.get() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PenEnd;

    fn s(pointer: u32, phase: PenPhase, x: f32) -> PenSample {
        PenSample { pointer, phase, pos: [x, 0.0], time: x as f64, ..Default::default() }
    }

    fn drain(q: &PenQueue) -> Vec<PenSample> {
        let mut v = Vec::new();
        q.drain_into(&mut v);
        v
    }

    /// P12
    #[test]
    fn queue_drops_oldest_counts_and_coalesces_hover() {
        let q = PenQueue::new(4);
        assert!(q.enabled());
        for i in 0..6 {
            q.push(s(1, PenPhase::Move, i as f32));
        }
        assert_eq!(q.dropped(), 2);
        let xs: Vec<f32> = drain(&q).iter().map(|s| s.pos[0]).collect();
        assert_eq!(xs, [2.0, 3.0, 4.0, 5.0], "oldest dropped, FIFO order kept");
        assert!(q.is_empty());

        // Hover coalesces onto the newest hover of the same pointer only.
        q.push(s(1, PenPhase::Hover, 1.0));
        q.push(s(1, PenPhase::Hover, 2.0));
        assert_eq!(q.len(), 1);
        q.push(s(2, PenPhase::Hover, 3.0));
        assert_eq!(q.len(), 2, "hover of another pointer is not coalesced");
        q.push(s(2, PenPhase::Up, 4.0));
        q.push(s(2, PenPhase::Hover, 5.0));
        assert_eq!(q.len(), 4, "hover never replaces an Up");
        q.push(s(2, PenPhase::Up, 6.0));
        q.push(s(2, PenPhase::Up, 7.0));
        let got = drain(&q);
        assert_eq!(got.len(), 4);
        assert_eq!(q.dropped(), 4);
        assert_eq!(got.iter().filter(|s| s.phase == PenPhase::Up).count(), 3, "Up is never coalesced");
        assert_eq!(got[0].pos[0], 4.0);

        // A coalesced hover keeps the newest fields.
        let mut e = s(1, PenPhase::Hover, 8.0);
        e.end = PenEnd::Eraser;
        q.push(s(1, PenPhase::Hover, 7.5));
        q.push(e);
        assert_eq!(drain(&q), [e]);

        q.set_enabled(false);
        assert!(!q.enabled());
        assert_eq!(PenQueue::new(0).buf.len(), 1);
    }

    #[test]
    fn drain_appends_and_wraps() {
        let q = PenQueue::new(3);
        q.push(s(1, PenPhase::Move, 0.0));
        q.push(s(1, PenPhase::Move, 1.0));
        drain(&q);
        // head is reset by drain; fill across the end of the buffer again.
        for i in 0..5 {
            q.push(s(1, PenPhase::Move, i as f32));
        }
        let mut out = vec![s(9, PenPhase::Leave, -1.0)];
        q.drain_into(&mut out);
        let xs: Vec<f32> = out.iter().map(|s| s.pos[0]).collect();
        assert_eq!(xs, [-1.0, 2.0, 3.0, 4.0]);
    }
}
