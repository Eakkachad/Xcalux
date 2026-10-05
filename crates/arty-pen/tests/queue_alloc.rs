//! The pen queue runs inside the window proc and once per frame: no heap
//! allocation after `PenQueue::new` (and none in `drain_into` while the
//! canvas's reused Vec has room).

use arty_pen::{PenPhase, PenQueue, PenSample};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

/// P13
#[test]
fn queue_push_and_drain_do_not_allocate() {
    let q = PenQueue::new(1024);
    let mut out = Vec::with_capacity(1024);
    let n = arty_testkit::count_allocs(|| {
        for frame in 0..10 {
            for i in 0..300 {
                let phase = if i % 3 == 0 { PenPhase::Hover } else { PenPhase::Move };
                q.push(PenSample { pointer: 1, phase, time: f64::from(frame * 300 + i), ..Default::default() });
            }
            out.clear();
            q.drain_into(&mut out);
        }
        // Overflow: drops the oldest without allocating.
        for i in 0..3000 {
            q.push(PenSample { phase: PenPhase::Move, time: f64::from(i), ..Default::default() });
        }
        out.clear();
        q.drain_into(&mut out);
    });
    assert_eq!(n, 0, "PenQueue allocated {n} times");
    assert_eq!(out.len(), 1024);
    assert_eq!(q.dropped(), 3000 - 1024);
}
