//! Ported from katgpt-rs `crates/katgpt-core/src/conformal/ring.rs` (MIT, see
//! `NOTICE`). Changes: added `ResidualRingBuffer::clear` and
//! `RingView::is_empty`; the sorted ring documents its no-realloc contract.
//!
//! Ring buffers for the conformal residual pool and the seasonal forecaster.

/// A single-channel ring storing `(value, tick)` pairs, kept sorted
/// ascending by `value`.
///
/// Insertion is O(n) (binary search + shift); the buffer is small. Both
/// arrays are reserved to `capacity` up front, so `push` never reallocates.
/// When `len == capacity`, the oldest entry (lowest tick) is evicted.
pub struct SortedRing {
    values: Vec<f32>,
    ticks: Vec<u64>,
    len: usize,
    capacity: usize,
}

impl SortedRing {
    /// Construct an empty ring with the given `capacity`.
    pub fn with_capacity(capacity: usize) -> Self {
        debug_assert!(capacity >= 1);
        Self {
            values: Vec::with_capacity(capacity),
            ticks: Vec::with_capacity(capacity),
            len: 0,
            capacity,
        }
    }

    /// Current number of stored entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` iff no entries are stored.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Sorted read of the `i`-th entry (ascending by value): `(value, tick)`.
    #[inline]
    pub fn get_sorted(&self, i: usize) -> (f32, u64) {
        debug_assert!(i < self.len, "index {} out of bounds (len {})", i, self.len);
        (self.values[i], self.ticks[i])
    }

    /// Push `(value, tick)`, keeping `values` sorted ascending. At capacity,
    /// evicts the entry with the smallest tick (deterministic).
    pub fn push(&mut self, value: f32, tick: u64) {
        let pos = match self.values.binary_search_by(|v| v.total_cmp(&value)) {
            Ok(p) | Err(p) => p,
        };
        if self.len < self.capacity {
            // Within the reserved capacity: no reallocation.
            self.values.insert(pos, value);
            self.ticks.insert(pos, tick);
            self.len += 1;
        } else if let Some(evict_idx) = self.oldest_tick_index() {
            // Combined evict + insert: one shift instead of two.
            if evict_idx < pos {
                // Shift left: [evict_idx+1..pos] → [evict_idx..pos-1]
                self.values.copy_within(evict_idx + 1..pos, evict_idx);
                self.ticks.copy_within(evict_idx + 1..pos, evict_idx);
                self.values[pos - 1] = value;
                self.ticks[pos - 1] = tick;
            } else if evict_idx > pos {
                // Shift right: [pos..evict_idx] → [pos+1..evict_idx+1]
                self.values.copy_within(pos..evict_idx, pos + 1);
                self.ticks.copy_within(pos..evict_idx, pos + 1);
                self.values[pos] = value;
                self.ticks[pos] = tick;
            } else {
                self.values[pos] = value;
                self.ticks[pos] = tick;
            }
        }
    }

    /// Index of the entry with the smallest tick (oldest). `None` if empty.
    fn oldest_tick_index(&self) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let mut best = 0usize;
        let mut best_tick = self.ticks[0];
        for i in 1..self.len {
            if self.ticks[i] < best_tick {
                best_tick = self.ticks[i];
                best = i;
            }
        }
        Some(best)
    }

    /// Clear all entries (keeps capacity).
    pub fn clear(&mut self) {
        self.values.clear();
        self.ticks.clear();
        self.len = 0;
    }
}

/// Per-channel × per-horizon-bucket pool of sorted residual rings.
pub struct ResidualRingBuffer {
    /// Flat array of rings: `rings[channel * n_buckets + bucket]`.
    rings: Vec<SortedRing>,
    /// Number of channels (dim 0).
    pub n_channels: usize,
    /// Number of horizon buckets (dim 1).
    pub n_buckets: usize,
}

impl ResidualRingBuffer {
    /// Construct a new pool with the given shape.
    pub fn new(n_channels: usize, n_buckets: usize, capacity: usize) -> Self {
        debug_assert!(n_channels >= 1);
        debug_assert!(n_buckets >= 1);
        debug_assert!(capacity >= 1);
        let total = n_channels
            .checked_mul(n_buckets)
            .expect("n_channels * n_buckets overflow");
        let rings = (0..total)
            .map(|_| SortedRing::with_capacity(capacity))
            .collect();
        Self {
            rings,
            n_channels,
            n_buckets,
        }
    }

    #[inline]
    fn flat(&self, channel: usize, bucket: usize) -> usize {
        debug_assert!(channel < self.n_channels, "channel {channel} oob");
        debug_assert!(bucket < self.n_buckets, "bucket {bucket} oob");
        channel * self.n_buckets + bucket
    }

    /// Push `(residual, tick)` into `(channel, bucket)`.
    #[inline]
    pub fn push(&mut self, residual: f32, channel: usize, bucket: usize, tick: u64) {
        let i = self.flat(channel, bucket);
        self.rings[i].push(residual, tick);
    }

    /// Read-only view of the `(channel, bucket)` ring.
    #[inline]
    pub fn channel_bucket(&self, channel: usize, bucket: usize) -> RingView<'_> {
        let i = self.flat(channel, bucket);
        RingView {
            inner: &self.rings[i],
        }
    }

    /// Empty every ring (keeps capacity).
    pub fn clear(&mut self) {
        for r in &mut self.rings {
            r.clear();
        }
    }
}

/// Read-only view into one sorted ring.
pub struct RingView<'a> {
    inner: &'a SortedRing,
}

impl RingView<'_> {
    /// Number of stored entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` iff no entries are stored.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Sorted read of the `i`-th entry.
    #[inline]
    pub fn get_sorted(&self, i: usize) -> (f32, u64) {
        self.inner.get_sorted(i)
    }
}

/// A simple FIFO ring buffer of `f32` (the seasonal forecaster's history).
pub struct RingBuffer {
    buf: Vec<f32>,
    head: usize,
    len: usize,
    capacity: usize,
}

impl RingBuffer {
    /// Construct an empty ring with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        debug_assert!(capacity >= 1);
        Self {
            buf: vec![0.0; capacity],
            head: 0,
            len: 0,
            capacity,
        }
    }

    /// Current number of stored entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` iff no entries are stored.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `true` iff at capacity.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len == self.capacity
    }

    /// Push a value, evicting the oldest if at capacity.
    #[inline]
    pub fn push(&mut self, value: f32) {
        if self.len < self.capacity {
            let idx = (self.head + self.len) % self.capacity;
            self.buf[idx] = value;
            self.len += 1;
        } else {
            self.buf[self.head] = value;
            self.head = (self.head + 1) % self.capacity;
        }
    }

    /// Value at logical index `i` (0 = oldest), or `None` if `i >= len`.
    #[inline]
    pub fn get(&self, i: usize) -> Option<f32> {
        if i >= self.len {
            return None;
        }
        Some(self.buf[(self.head + i) % self.capacity])
    }

    /// Value at offset `back` from the newest (`0` = newest).
    #[inline]
    pub fn back(&self, back: usize) -> Option<f32> {
        if back >= self.len {
            return None;
        }
        self.get(self.len - 1 - back)
    }

    /// Clear all entries (keeps capacity).
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_ring_keeps_sorted_on_push() {
        let mut r = SortedRing::with_capacity(8);
        for &v in &[3.0_f32, 1.0, 4.0, 1.5, 2.0, 1.0, 3.5, 0.5] {
            r.push(v, 0);
        }
        let got: Vec<f32> = (0..r.len()).map(|i| r.get_sorted(i).0).collect();
        assert_eq!(got, vec![0.5, 1.0, 1.0, 1.5, 2.0, 3.0, 3.5, 4.0]);
    }

    #[test]
    fn sorted_ring_evicts_oldest_tick_at_capacity() {
        let mut r = SortedRing::with_capacity(3);
        r.push(10.0, 1); // oldest
        r.push(20.0, 2);
        r.push(30.0, 3);
        r.push(40.0, 4);
        let got: Vec<(f32, u64)> = (0..r.len()).map(|i| r.get_sorted(i)).collect();
        assert_eq!(got, vec![(20.0, 2), (30.0, 3), (40.0, 4)]);
    }

    #[test]
    fn sorted_ring_eviction_all_shift_directions() {
        // Evict left of the insert point, right of it, and at it.
        let mut r = SortedRing::with_capacity(3);
        r.push(1.0, 1);
        r.push(2.0, 2);
        r.push(3.0, 3);
        r.push(2.5, 4); // evicts 1.0 (left of pos)
        assert_eq!(
            (0..3).map(|i| r.get_sorted(i)).collect::<Vec<_>>(),
            vec![(2.0, 2), (2.5, 4), (3.0, 3)]
        );
        let mut r = SortedRing::with_capacity(3);
        r.push(3.0, 1);
        r.push(1.0, 2);
        r.push(2.0, 3);
        r.push(0.5, 4); // evicts 3.0 (right of pos)
        assert_eq!(
            (0..3).map(|i| r.get_sorted(i)).collect::<Vec<_>>(),
            vec![(0.5, 4), (1.0, 2), (2.0, 3)]
        );
        let mut r = SortedRing::with_capacity(2);
        r.push(1.0, 1);
        r.push(5.0, 2);
        r.push(1.0, 3); // evicts the 1.0 at the insert point
        assert_eq!(
            (0..2).map(|i| r.get_sorted(i)).collect::<Vec<_>>(),
            vec![(1.0, 3), (5.0, 2)]
        );
    }

    #[test]
    fn ring_buffer_fifo() {
        let mut rb = RingBuffer::with_capacity(3);
        for &v in &[1.0_f32, 2.0, 3.0] {
            rb.push(v);
        }
        assert!(rb.is_full());
        assert_eq!(rb.get(0), Some(1.0));
        assert_eq!(rb.get(2), Some(3.0));
        assert_eq!(rb.back(0), Some(3.0));
        assert_eq!(rb.back(1), Some(2.0));
        rb.push(4.0);
        assert_eq!(rb.get(0), Some(2.0));
        assert_eq!(rb.get(2), Some(4.0));
        assert_eq!(rb.back(0), Some(4.0));
        rb.clear();
        assert!(rb.is_empty());
    }

    #[test]
    fn residual_pool_channel_bucket_isolation() {
        let mut pool = ResidualRingBuffer::new(2, 2, 4);
        pool.push(1.0, 0, 0, 0);
        pool.push(2.0, 0, 0, 0);
        pool.push(10.0, 1, 1, 0);
        pool.push(20.0, 1, 1, 0);
        let v00 = pool.channel_bucket(0, 0);
        assert_eq!(v00.len(), 2);
        assert_eq!(v00.get_sorted(0).0, 1.0);
        assert_eq!(v00.get_sorted(1).0, 2.0);
        assert!(pool.channel_bucket(1, 0).is_empty());
        let v11 = pool.channel_bucket(1, 1);
        assert_eq!(v11.len(), 2);
        assert_eq!(v11.get_sorted(0).0, 10.0);
        pool.clear();
        assert!(pool.channel_bucket(1, 1).is_empty());
    }
}
