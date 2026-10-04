//! Test-only helpers shared by ARTY crates.
//!
//! The main export is [`CountingAllocator`]: install it as the
//! `#[global_allocator]` of a test binary and wrap a hot path in
//! [`count_allocs`] to assert it performs zero heap allocations.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;
//!
//! let n = arty_testkit::count_allocs(|| hot_path());
//! assert_eq!(n, 0);
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

/// Counts allocations made on the current thread while tracking is enabled.
pub struct CountingAllocator;

#[inline]
fn note_alloc() {
    // `try_with` so allocations during TLS teardown never panic.
    let _ = TRACKING.try_with(|t| {
        if t.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Runs `f` and returns how many allocations (alloc, alloc_zeroed, realloc)
/// it made on this thread. Only meaningful when [`CountingAllocator`] is the
/// global allocator.
pub fn count_allocs(f: impl FnOnce()) -> usize {
    COUNT.with(|c| c.set(0));
    TRACKING.with(|t| t.set(true));
    f();
    TRACKING.with(|t| t.set(false));
    COUNT.with(|c| c.get())
}
