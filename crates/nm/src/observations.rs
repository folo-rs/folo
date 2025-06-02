use std::{
    iter,
    sync::atomic::{self, AtomicI64, AtomicU64},
};

use crate::Magnitude;

/// Collects the observations of a single event on a single thread.
///
/// While intended to be written to from a single thread, the data within
/// may be read from other threads for the purpose of generating metrics reports.
///
/// As reading is lock-free, logically torn reads (of different fields) are entirely possible.
/// Do not assume internal consistency between reading different fields.
#[derive(Debug)]
pub(crate) struct ObservationBag {
    count: AtomicU64,
    sum: AtomicI64,

    bucket_counts: Box<[AtomicU64]>,
    bucket_magnitudes: &'static [Magnitude],
}

/// We use `Relaxed` ordering for all atomic operations to allow field access to be as
/// fast as possible because we want to avoid any penalties on write accesses. This should be
/// approximately equivalent to non-atomic access on 64-bit platforms, avoiding performance loss.
const BAG_ACCESS_ORDERING: atomic::Ordering = atomic::Ordering::Relaxed;

impl ObservationBag {
    pub(crate) fn new(buckets: &'static [Magnitude]) -> Self {
        let bag = Self {
            count: AtomicU64::new(0),
            sum: AtomicI64::new(0),
            bucket_counts: iter::repeat_with(|| AtomicU64::new(0))
                .take(buckets.len())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            bucket_magnitudes: buckets,
        };

        // Important type invariant used to ensure safety - the lengths of these two
        // must always match. We assert this just to make it super obvious.
        debug_assert_eq!(
            bag.bucket_counts.len(),
            bag.bucket_magnitudes.len(),
            "we derive count length from magnitudes length, so they must match",
        );

        bag
    }

    pub(crate) fn insert(&self, magnitude: Magnitude, count: usize) {
        // Crate policy is to not panic but instead to mangle data upon mathematical
        // challenges and edge cases that cannot be correctly handled. We apply this here
        // by defaulting to 0 in case of out-of-range values and by using wrapping arithmetic.

        let count_u64 = u64::try_from(count).unwrap_or_default();
        let count_i64 = i64::try_from(count).unwrap_or_default();

        let sum_increment = magnitude.wrapping_mul(count_i64);

        // These operations always use wrapping arithmetic.
        self.count.fetch_add(count_u64, BAG_ACCESS_ORDERING);
        self.sum.fetch_add(sum_increment, BAG_ACCESS_ORDERING);

        // This may be none if we have no buckets (i.e. the event is a bare counter, no histogram).
        // TODO: Explore optimizing this lookup - SIMD can potentially help us here.
        if let Some(bucket_index) =
            self.bucket_magnitudes
                .iter()
                .enumerate()
                .find_map(|(i, &bucket_magnitude)| {
                    if magnitude <= bucket_magnitude {
                        Some(i)
                    } else {
                        None
                    }
                })
        {
            // We do this unsafely because we need minimal overhead in the hot path from
            // collecting observations and this will be a very hot path.
            //
            // SAFETY: Type invariant: there are always the same number of bucket counts
            // as there are bucket magnitudes.
            unsafe { self.bucket_counts.get_unchecked(bucket_index) }
                .fetch_add(1, BAG_ACCESS_ORDERING);
        }
    }

    /// Takes a snapshot of the current state.
    ///
    /// No synchronization is used - different fields of the snapshot are
    /// not guaranteed to be consistent with each other. The only guarantee we provide
    /// is that each field has a value that was extant at some recent point in time.
    pub(crate) fn snapshot(&self) -> ObservationBagSnapshot {
        ObservationBagSnapshot {
            count: self.count.load(BAG_ACCESS_ORDERING),
            sum: self.sum.load(BAG_ACCESS_ORDERING),
            bucket_counts: self
                .bucket_counts
                .iter()
                .map(|x| x.load(BAG_ACCESS_ORDERING))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            bucket_magnitudes: self.bucket_magnitudes,
        }
    }
}

/// A point in time snapshot of a single event's observations.
///
/// May represent the observations of a single thread or a merged set of observations
/// from multiple threads, depending on how it is obtained.
#[derive(Debug)]
pub(crate) struct ObservationBagSnapshot {
    pub(crate) count: u64,
    pub(crate) sum: Magnitude,
    pub(crate) bucket_counts: Box<[u64]>,
    pub(crate) bucket_magnitudes: &'static [Magnitude],
}

impl ObservationBagSnapshot {
    /// Merges another snapshot into this one, combining their data set.
    ///
    /// This is typically used to combine the data from multiple threads for reporting.
    pub(crate) fn merge(&mut self, other: &Self) {
        self.count = self.count.wrapping_add(other.count);
        self.sum = self.sum.wrapping_add(other.sum);

        // We cannot merge snapshots with different bucket magnitudes.
        assert_eq!(self.bucket_magnitudes, other.bucket_magnitudes);

        // Extra sanity check for maximum paranoia.
        assert!(self.bucket_counts.len() == other.bucket_counts.len());

        for (i, &other_bucket_count) in other.bucket_counts.iter().enumerate() {
            let target = self
                .bucket_counts
                .get_mut(i)
                .expect("guarded by assertion above");

            *target = target.wrapping_add(other_bucket_count);
        }
    }
}
