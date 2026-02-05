use std::sync::atomic::{self, AtomicU64};

/// The number of allocation size buckets for detailed tracking.
pub(crate) const BUCKET_COUNT: usize = 9;

/// Bucket boundaries for allocation size tracking (in bytes).
/// Buckets are: [0, 64), [64, 256), [256, 1KB), [1KB, 4KB), [4KB, 16KB),
/// [16KB, 64KB), [64KB, 256KB), [256KB, 1MB), [1MB, ∞)
pub(crate) const BUCKET_BOUNDARIES: [u64; BUCKET_COUNT] = [
    64,          // < 64 bytes (tiny)
    256,         // 64 - 256 bytes (small)
    1024,        // 256 bytes - 1KB (medium-small)
    4 * 1024,    // 1KB - 4KB (medium)
    16 * 1024,   // 4KB - 16KB (medium-large)
    64 * 1024,   // 16KB - 64KB (large)
    256 * 1024,  // 64KB - 256KB (very large)
    1024 * 1024, // 256KB - 1MB (huge)
    u64::MAX,    // >= 1MB (massive)
];

/// Human-readable labels for allocation buckets.
pub(crate) const BUCKET_LABELS: [&str; BUCKET_COUNT] = [
    "< 64B",
    "64B - 256B",
    "256B - 1KB",
    "1KB - 4KB",
    "4KB - 16KB",
    "16KB - 64KB",
    "64KB - 256KB",
    "256KB - 1MB",
    ">= 1MB",
];

/// Returns the bucket index for a given allocation size.
#[inline]
pub(crate) fn bucket_index(size: u64) -> usize {
    for (i, &boundary) in BUCKET_BOUNDARIES.iter().enumerate() {
        if size < boundary {
            return i;
        }
    }
    // This should never happen since the last boundary is u64::MAX
    BUCKET_COUNT - 1
}

/// Snapshot of allocation counts per size bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BucketCounts {
    counts: [u64; BUCKET_COUNT],
}

impl BucketCounts {
    /// Creates a new zeroed bucket counts.
    #[inline]
    pub(crate) const fn zero() -> Self {
        Self {
            counts: [0; BUCKET_COUNT],
        }
    }

    /// Creates bucket counts from a raw array.
    #[inline]
    pub(crate) const fn from_array(counts: [u64; BUCKET_COUNT]) -> Self {
        Self { counts }
    }

    /// Merges another bucket counts into this one by adding counts.
    #[inline]
    #[expect(
        clippy::indexing_slicing,
        reason = "index i from enumerate is always valid for same-sized arrays"
    )]
    pub(crate) fn merge(&mut self, other: &Self) {
        for (i, &count) in other.counts.iter().enumerate() {
            self.counts[i] = self.counts[i]
                .checked_add(count)
                .expect("bucket count overflow");
        }
    }

    /// Computes the delta between this snapshot and an end snapshot,
    /// dividing by iterations if greater than 1.
    #[inline]
    #[expect(
        clippy::indexing_slicing,
        reason = "index i from enumerate is always valid for same-sized arrays"
    )]
    pub(crate) fn delta_from(&self, end: &Self, iterations: u64) -> Self {
        let mut result = [0_u64; BUCKET_COUNT];
        for (i, (&start, &end)) in self.counts.iter().zip(end.counts.iter()).enumerate() {
            let total = end
                .checked_sub(start)
                .expect("bucket count could not possibly decrease");
            result[i] = if iterations > 1 {
                total.checked_div(iterations).expect("iterations != 0")
            } else {
                total
            };
        }
        Self { counts: result }
    }

    /// Adds scaled bucket counts (multiplied by iterations) to this instance.
    #[inline]
    #[expect(
        clippy::indexing_slicing,
        reason = "index i from enumerate is always valid for same-sized arrays"
    )]
    pub(crate) fn add_scaled(&mut self, other: &Self, iterations: u64) {
        for (i, &delta) in other.counts.iter().enumerate() {
            let total = delta
                .checked_mul(iterations)
                .expect("bucket delta * iterations overflow");
            self.counts[i] = self.counts[i]
                .checked_add(total)
                .expect("bucket count overflow");
        }
    }

    /// Returns an iterator over allocation buckets from smallest to largest.
    #[inline]
    #[expect(
        clippy::indexing_slicing,
        reason = "BUCKET_LABELS has same length as counts array"
    )]
    pub(crate) fn iter(&self) -> impl Iterator<Item = AllocationBucket> + '_ {
        self.counts
            .iter()
            .enumerate()
            .map(|(i, &count)| AllocationBucket::new(BUCKET_LABELS[i], count))
    }
}

/// Atomic counters for tracking allocations by size bucket.
#[derive(Debug)]
pub(crate) struct BucketCounters {
    counts: [AtomicU64; BUCKET_COUNT],
}

impl BucketCounters {
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            counts: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// Records an allocation of the given size into the appropriate bucket.
    #[inline]
    #[expect(
        clippy::indexing_slicing,
        reason = "bucket_index always returns a valid index in 0..BUCKET_COUNT"
    )]
    pub(crate) fn record(&self, size: u64) {
        let idx = bucket_index(size);
        self.counts[idx].fetch_add(1, atomic::Ordering::Relaxed);
    }

    /// Returns a snapshot of current bucket counts.
    #[inline]
    #[expect(
        clippy::indexing_slicing,
        reason = "std::array::from_fn generates valid indices for the array size"
    )]
    pub(crate) fn counts(&self) -> BucketCounts {
        BucketCounts::from_array(std::array::from_fn(|i| {
            self.counts[i].load(atomic::Ordering::Relaxed)
        }))
    }
}

/// Information about allocations in a specific size bucket.
#[derive(Clone, Debug)]
pub struct AllocationBucket {
    label: &'static str,
    allocations: u64,
}

impl AllocationBucket {
    /// Creates a new allocation bucket with the given label and count.
    pub(crate) fn new(label: &'static str, allocations: u64) -> Self {
        Self { label, allocations }
    }

    /// Returns the human-readable label for this bucket (e.g., "< 64B", "1KB - 4KB").
    #[must_use]
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// Returns the number of allocations in this bucket.
    #[must_use]
    pub fn allocations(&self) -> u64 {
        self.allocations
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn bucket_index_boundaries() {
        let cases = [
            (0, 0),
            (63, 0), // < 64B
            (64, 1),
            (255, 1), // 64B - 256B
            (256, 2),
            (1023, 2), // 256B - 1KB
            (1024, 3),
            (4095, 3), // 1KB - 4KB
            (4096, 4),
            (16383, 4), // 4KB - 16KB
            (16384, 5),
            (65535, 5), // 16KB - 64KB
            (65536, 6),
            (262143, 6), // 64KB - 256KB
            (262144, 7),
            (1048575, 7), // 256KB - 1MB
            (1048576, 8),
            (u64::MAX - 1, 8), // >= 1MB
        ];
        for (size, expected) in cases {
            assert_eq!(bucket_index(size), expected, "size {size}");
        }
    }

    #[test]
    fn allocation_bucket_getters() {
        let bucket = AllocationBucket::new("< 64B", 42);
        assert_eq!(bucket.label(), "< 64B");
        assert_eq!(bucket.allocations(), 42);
    }

    #[test]
    fn bucket_counts_zero() {
        let counts = BucketCounts::zero();
        assert_eq!(counts.counts, [0; BUCKET_COUNT]);
    }

    #[test]
    fn bucket_counts_merge() {
        let mut a = BucketCounts::from_array([1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let b = BucketCounts::from_array([10, 20, 30, 40, 50, 60, 70, 80, 90]);
        a.merge(&b);
        assert_eq!(a.counts, [11, 22, 33, 44, 55, 66, 77, 88, 99]);
    }

    #[test]
    fn bucket_counts_delta_from() {
        let start = BucketCounts::from_array([0, 10, 20, 30, 40, 50, 60, 70, 80]);
        let end = BucketCounts::from_array([5, 15, 30, 40, 50, 60, 70, 80, 90]);
        let delta = start.delta_from(&end, 1);
        assert_eq!(delta.counts, [5, 5, 10, 10, 10, 10, 10, 10, 10]);
    }

    #[test]
    fn bucket_counts_delta_from_with_iterations() {
        let start = BucketCounts::from_array([0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let end = BucketCounts::from_array([10, 20, 30, 40, 50, 60, 70, 80, 90]);
        let delta = start.delta_from(&end, 10);
        assert_eq!(delta.counts, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn bucket_counts_add_scaled() {
        let mut counts = BucketCounts::from_array([1, 1, 1, 1, 1, 1, 1, 1, 1]);
        let delta = BucketCounts::from_array([1, 2, 3, 4, 5, 6, 7, 8, 9]);
        counts.add_scaled(&delta, 3);
        assert_eq!(counts.counts, [4, 7, 10, 13, 16, 19, 22, 25, 28]);
    }

    #[test]
    fn bucket_counts_iter() {
        let counts = BucketCounts::from_array([1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let buckets: Vec<_> = counts.iter().collect();
        assert_eq!(buckets.len(), 9);
        assert_eq!(buckets[0].label(), "< 64B");
        assert_eq!(buckets[0].allocations(), 1);
        assert_eq!(buckets[8].label(), ">= 1MB");
        assert_eq!(buckets[8].allocations(), 9);
    }
}
