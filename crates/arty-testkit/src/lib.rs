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
//!
//! It also tracks live heap bytes process-wide, so [`peak_bytes_during`]
//! can bound the memory a (possibly multi-threaded) operation needs.
//!
//! With feature `synthetic`, [`synthetic_manga_page`] builds the reference
//! document of the I/O benchmarks.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "synthetic")]
pub mod synthetic;
#[cfg(feature = "synthetic")]
pub use synthetic::{LayerType, Page, synthetic_manga_page};

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

/// Live heap bytes across all threads, and the high-water mark since the
/// last [`reset_peak`].
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Counts allocations made on the current thread while tracking is enabled,
/// and live bytes on every thread.
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

#[inline]
fn grow(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

#[inline]
fn shrink(bytes: usize) {
    LIVE.fetch_sub(bytes, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            // Count the old block as freed only after the new one exists,
            // since a moving realloc briefly holds both.
            grow(new_size);
            shrink(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        shrink(layout.size());
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

/// Heap bytes currently allocated by the whole process.
pub fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// Highest [`live_bytes`] seen since the last [`reset_peak`].
pub fn peak_bytes() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// Restart peak tracking from the current live bytes.
pub fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Runs `f` and returns its result with the peak heap growth (bytes above
/// the live bytes at the start) it caused on any thread.
///
/// The counters are process-wide: tests running concurrently in the same
/// binary add to the peak, so give peak-bounded tests their own test binary
/// or keep them to one test per binary.
pub fn peak_bytes_during<R>(f: impl FnOnce() -> R) -> (R, usize) {
    reset_peak();
    let base = live_bytes();
    let r = f();
    (r, peak_bytes().saturating_sub(base))
}
