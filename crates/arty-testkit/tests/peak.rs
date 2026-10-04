//! The allocator's live/peak byte tracking. One test, so nothing else in
//! this binary disturbs the process-wide counters.

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

#[test]
fn peak_tracks_largest_live_set() {
    const MIB: usize = 1 << 20;
    let (_, peak) = arty_testkit::peak_bytes_during(|| {
        let a = vec![1u8; 4 * MIB];
        let b = vec![2u8; 2 * MIB];
        drop(a);
        // Other threads count too: 2 + 3 MiB live, below the 6 MiB peak.
        std::thread::spawn(|| std::hint::black_box(vec![3u8; 3 * MIB]).len()).join().unwrap();
        drop(b);
    });
    assert!((6 * MIB..7 * MIB).contains(&peak), "peak {peak}");

    let before = arty_testkit::live_bytes();
    let mut v = vec![0u8; MIB];
    v.reserve_exact(3 * MIB);
    assert!(arty_testkit::live_bytes() >= before + 4 * MIB, "realloc counts the new size");
    drop(v);
    assert!(arty_testkit::live_bytes() < before + MIB, "dealloc is subtracted");
}
