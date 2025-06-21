use std::cell::Cell;
use std::iter;
use std::sync::atomic::{self, AtomicI64, AtomicU64};

use crate::Magnitude;

/// Records the observations of an event.
///
/// This variant is intended for single-threaded use, though may be shared on that
/// thread via `Rc` or similar mechanisms as it uses interior mutability.
#[derive(Debug)]
pub(crate) struct ObservationBag {
    count: Cell<u64>,
    sum: Cell<i64>,

    bucket_counts: Box<[Cell<u64>]>,
    bucket_magnitudes: &'static [Magnitude],
}

/// Records the observations of an event in a thread-safe manner.
///
/// While this variant is intended to be written to from a single thread, the data within
/// may be read from other threads for the purpose of generating metrics reports.
///
/// As reading is lock-free, logically torn reads (of different fields) are entirely possible.
/// Do not assume internal consistency between reading different fields.
#[derive(Debug)]
pub(crate) struct ObservationBagSync {
    count: AtomicU64,
    sum: AtomicI64,

    bucket_counts: Box<[AtomicU64]>,
    bucket_magnitudes: &'static [Magnitude],
}

/// Abstraction over the different types of observation bags.
pub(crate) trait Observations {
    /// Record `count` observations of the given `magnitude`.
    fn insert(&self, magnitude: Magnitude, count: usize);

    /// Takes a snapshot of the current state.
    ///
    /// No synchronization is assumed - different fields of the snapshot are
    /// not guaranteed to be consistent with each other. The only guarantee we provide
    /// is that each field has a value that was extant at some recent point in time.
    fn snapshot(&self) -> ObservationBagSnapshot;

    /// The bucket magnitudes used by this bag to generate histograms.
    ///
    /// Buckets with different magnitudes are incompatible, so this is used to verify
    /// that two ostensibly similar bags can be merged or compared.
    fn bucket_magnitudes(&self) -> &'static [Magnitude];
}

impl ObservationBag {
    pub(crate) fn new(bucket_magnitudes: &'static [Magnitude]) -> Self {
        let bag = Self {
            count: Cell::new(0),
            sum: Cell::new(0),
            bucket_counts: iter::repeat_with(|| Cell::new(0))
                .take(bucket_magnitudes.len())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            bucket_magnitudes,
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
}

/// We use `Relaxed` ordering for all atomic operations to allow field access to be as
/// fast as possible because we want to avoid any penalties on write accesses. This should be
/// approximately equivalent to non-atomic access on 64-bit platforms, avoiding performance loss.
/// We accept the potential for delayed writes and similar effects on platforms with weak memory.
const SYNC_BAG_ACCESS_ORDERING: atomic::Ordering = atomic::Ordering::Relaxed;

impl ObservationBagSync {
    pub(crate) fn new(bucket_magnitudes: &'static [Magnitude]) -> Self {
        let bag = Self {
            count: AtomicU64::new(0),
            sum: AtomicI64::new(0),
            bucket_counts: iter::repeat_with(|| AtomicU64::new(0))
                .take(bucket_magnitudes.len())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            bucket_magnitudes,
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

    /// Merges another observation bag into this one, combining their data set.
    ///
    /// This is typically used when archiving data from unregistered threads,
    /// at which point it gets merged into a single archive bag.
    pub(crate) fn merge_from(&self, other: &Self) {
        self.count.fetch_add(
            other.count.load(SYNC_BAG_ACCESS_ORDERING),
            SYNC_BAG_ACCESS_ORDERING,
        );
        self.sum.fetch_add(
            other.sum.load(SYNC_BAG_ACCESS_ORDERING),
            SYNC_BAG_ACCESS_ORDERING,
        );

        // We cannot merge bags with different bucket magnitudes.
        assert_eq!(self.bucket_magnitudes, other.bucket_magnitudes);

        // Extra sanity check for maximum paranoia.
        assert!(self.bucket_counts.len() == other.bucket_counts.len());

        for (i, other_bucket_count) in other.bucket_counts.iter().enumerate() {
            let target = self
                .bucket_counts
                .get(i)
                .expect("guarded by assertion above");

            target.fetch_add(
                other_bucket_count.load(SYNC_BAG_ACCESS_ORDERING),
                SYNC_BAG_ACCESS_ORDERING,
            );
        }
    }

    /// Replaces the data in the bag with the data from the given snapshot.
    pub(crate) fn replace(&self, data: &ObservationBagSnapshot) {
        // We cannot replace with a snapshot with different bucket magnitudes.
        assert_eq!(self.bucket_magnitudes, data.bucket_magnitudes);

        // Extra sanity check for maximum paranoia.
        assert!(self.bucket_counts.len() == data.bucket_counts.len());

        self.count.store(data.count, SYNC_BAG_ACCESS_ORDERING);
        self.sum.store(data.sum, SYNC_BAG_ACCESS_ORDERING);

        for (i, &bucket_count) in data.bucket_counts.iter().enumerate() {
            let target = self
                .bucket_counts
                .get(i)
                .expect("guarded by assertion above");

            target.store(bucket_count, SYNC_BAG_ACCESS_ORDERING);
        }
    }
}

impl Observations for ObservationBag {
    fn insert(&self, magnitude: Magnitude, count: usize) {
        // Crate policy is to not panic but instead to mangle data upon mathematical
        // challenges and edge cases that cannot be correctly handled. We apply this here
        // by defaulting to 0 in case of out-of-range values and by using wrapping arithmetic.

        let count_u64 = u64::try_from(count).unwrap_or_default();
        let count_i64 = i64::try_from(count_u64).unwrap_or_default();

        let sum_increment = magnitude.wrapping_mul(count_i64);

        // We use wrapping arithmetic because it is the fastest (and we are allowed to mangle).
        self.count.set(self.count.get().wrapping_add(count_u64));
        self.sum.set(self.sum.get().wrapping_add(sum_increment));

        // This may be none if we have no buckets (i.e. the event is a bare counter, no histogram).
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
            // We use wrapping arithmetic because it is the fastest (and we are allowed to mangle).
            //
            // We do this unsafely because we need minimal overhead in the hot path from
            // collecting observations and this will be a very hot path.
            //
            // SAFETY: Type invariant: there are always the same number of bucket counts
            // as there are bucket magnitudes.
            let bucket_count = unsafe { self.bucket_counts.get_unchecked(bucket_index) };
            bucket_count.set(bucket_count.get().wrapping_add(count_u64));
        }
    }

    fn snapshot(&self) -> ObservationBagSnapshot {
        ObservationBagSnapshot {
            count: self.count.get(),
            sum: self.sum.get(),
            bucket_counts: self
                .bucket_counts
                .iter()
                .map(Cell::get)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            bucket_magnitudes: self.bucket_magnitudes,
        }
    }

    #[cfg_attr(test, mutants::skip)] // Would violate counts.len() == magnitudes.len() invariant.
    fn bucket_magnitudes(&self) -> &'static [Magnitude] {
        self.bucket_magnitudes
    }
}

impl Observations for ObservationBagSync {
    fn insert(&self, magnitude: Magnitude, count: usize) {
        // Crate policy is to not panic but instead to mangle data upon mathematical
        // challenges and edge cases that cannot be correctly handled. We apply this here
        // by defaulting to 0 in case of out-of-range values and by using wrapping arithmetic.

        let count_u64 = u64::try_from(count).unwrap_or_default();
        let count_i64 = i64::try_from(count_u64).unwrap_or_default();

        let sum_increment = magnitude.wrapping_mul(count_i64);

        // These operations always use wrapping arithmetic.
        self.count.fetch_add(count_u64, SYNC_BAG_ACCESS_ORDERING);
        self.sum.fetch_add(sum_increment, SYNC_BAG_ACCESS_ORDERING);

        // This may be none if we have no buckets (i.e. the event is a bare counter, no histogram).
        // TODO: Explore optimizing this lookup - SIMD can potentially help us here if not
        // automatically applied already by the compiler.
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
            // We use wrapping arithmetic because it is the fastest (and we are allowed to mangle).
            //
            // We do this unsafely because we need minimal overhead in the hot path from
            // collecting observations and this will be a very hot path.
            //
            // SAFETY: Type invariant: there are always the same number of bucket counts
            // as there are bucket magnitudes.
            unsafe { self.bucket_counts.get_unchecked(bucket_index) }
                .fetch_add(count_u64, SYNC_BAG_ACCESS_ORDERING);
        }
    }

    fn snapshot(&self) -> ObservationBagSnapshot {
        ObservationBagSnapshot {
            count: self.count.load(SYNC_BAG_ACCESS_ORDERING),
            sum: self.sum.load(SYNC_BAG_ACCESS_ORDERING),
            bucket_counts: self
                .bucket_counts
                .iter()
                .map(|x| x.load(SYNC_BAG_ACCESS_ORDERING))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            bucket_magnitudes: self.bucket_magnitudes,
        }
    }

    #[cfg_attr(test, mutants::skip)] // Would violate counts.len() == magnitudes.len() invariant.
    fn bucket_magnitudes(&self) -> &'static [Magnitude] {
        self.bucket_magnitudes
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

    /// Ascending order, not including the final `Magnitude::MAX` bucket.
    pub(crate) bucket_magnitudes: &'static [Magnitude],

    /// Not including the final `Magnitude::MAX` bucket.
    pub(crate) bucket_counts: Box<[u64]>,
}

impl ObservationBagSnapshot {
    /// Merges another snapshot into this one, combining their data set.
    ///
    /// This is typically used to combine the data from multiple threads for reporting.
    pub(crate) fn merge_from(&mut self, other: &Self) {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;

    #[test]
    fn observations_are_recorded() {
        let observations = ObservationBag::new(&[]);

        // A quick sanity check first.
        observations.insert(7, 2);

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 14);

        // Zero is a perfectly fine magnitude.
        observations.insert(0, 3);

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 5);
        assert_eq!(snapshot.sum, 14);

        // Negative magnitudes are also fine.
        observations.insert(-30, 4);

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 9);
        assert_eq!(snapshot.sum, -106);
    }

    #[test]
    fn observations_are_recorded_sync() {
        let observations = ObservationBagSync::new(&[]);

        // A quick sanity check first.
        observations.insert(7, 2);

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 14);

        // Zero is a perfectly fine magnitude.
        observations.insert(0, 3);

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 5);
        assert_eq!(snapshot.sum, 14);

        // Negative magnitudes are also fine.
        observations.insert(-30, 4);

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 9);
        assert_eq!(snapshot.sum, -106);
    }

    #[test]
    fn observations_are_recorded_in_histogram() {
        let observations = ObservationBag::new(&[-100, -10, 0, 10, 100]);

        observations.insert(-1000, 1);
        observations.insert(0, 2);
        observations.insert(11, 3);
        observations.insert(1111, 4);

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 10);
        assert_eq!(snapshot.sum, 1111 * 4 + 11 * 3 - 1000);

        assert_eq!(snapshot.bucket_counts.len(), 5);
        assert_eq!(snapshot.bucket_counts[0], 1); // -1000
        assert_eq!(snapshot.bucket_counts[1], 0); // nothing
        assert_eq!(snapshot.bucket_counts[2], 2); // 0
        assert_eq!(snapshot.bucket_counts[3], 0); // nothing
        assert_eq!(snapshot.bucket_counts[4], 3); // 11

        // 1111 is outside any bucket ranges, so only present in the totals.
    }

    #[test]
    fn observations_are_recorded_in_histogram_sync() {
        let observations = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

        observations.insert(-1000, 1);
        observations.insert(0, 2);
        observations.insert(11, 3);
        observations.insert(1111, 4);

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 10);
        assert_eq!(snapshot.sum, 1111 * 4 + 11 * 3 - 1000);

        assert_eq!(snapshot.bucket_counts.len(), 5);
        assert_eq!(snapshot.bucket_counts[0], 1); // -1000
        assert_eq!(snapshot.bucket_counts[1], 0); // nothing
        assert_eq!(snapshot.bucket_counts[2], 2); // 0
        assert_eq!(snapshot.bucket_counts[3], 0); // nothing
        assert_eq!(snapshot.bucket_counts[4], 3); // 11

        // 1111 is outside any bucket ranges, so only present in the totals.
    }

    #[test]
    fn existing_snapshots_do_not_change() {
        let observations = ObservationBag::new(&[]);
        observations.insert(7, 2);

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 14);

        observations.insert(123, 123);

        // The existing snapshot should not have changed.
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 14);
    }

    #[test]
    fn existing_snapshots_do_not_change_sync() {
        let observations = ObservationBagSync::new(&[]);
        observations.insert(7, 2);

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 14);

        observations.insert(123, 123);

        // The existing snapshot should not have changed.
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 14);
    }

    #[test]
    fn snapshot_merge_merges_data() {
        let observations = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

        observations.insert(-1000, 1);
        observations.insert(0, 2);
        observations.insert(11, 3);
        observations.insert(1111, 4);

        // We just merge the same snapshot into itself to test the merge logic.
        let mut snapshot1 = observations.snapshot();
        let snapshot2 = observations.snapshot();

        snapshot1.merge_from(&snapshot2);

        assert_eq!(snapshot1.count, 2 * 10);
        assert_eq!(snapshot1.sum, 2 * (1111 * 4 + 11 * 3 - 1000));

        assert_eq!(snapshot1.bucket_counts.len(), 5);
        assert_eq!(snapshot1.bucket_counts[0], 2); // -1000
        assert_eq!(snapshot1.bucket_counts[1], 0); // nothing
        assert_eq!(snapshot1.bucket_counts[2], 4); // 0
        assert_eq!(snapshot1.bucket_counts[3], 0); // nothing
        assert_eq!(snapshot1.bucket_counts[4], 6); // 11
    }

    #[test]
    fn snapshot_merge_from_sync_and_nonsync_merges_data() {
        let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

        observations1.insert(-1000, 1);
        observations1.insert(0, 2);
        observations1.insert(11, 3);
        observations1.insert(1111, 4);

        let observations2 = ObservationBag::new(&[-100, -10, 0, 10, 100]);

        observations2.insert(-1000, 1);
        observations2.insert(0, 2);
        observations2.insert(11, 3);
        observations2.insert(1111, 4);

        let mut snapshot1 = observations1.snapshot();
        let snapshot2 = observations2.snapshot();

        snapshot1.merge_from(&snapshot2);

        assert_eq!(snapshot1.count, 2 * 10);
        assert_eq!(snapshot1.sum, 2 * (1111 * 4 + 11 * 3 - 1000));

        assert_eq!(snapshot1.bucket_counts.len(), 5);
        assert_eq!(snapshot1.bucket_counts[0], 2); // -1000
        assert_eq!(snapshot1.bucket_counts[1], 0); // nothing
        assert_eq!(snapshot1.bucket_counts[2], 4); // 0
        assert_eq!(snapshot1.bucket_counts[3], 0); // nothing
        assert_eq!(snapshot1.bucket_counts[4], 6); // 11
    }

    #[test]
    fn bag_merge_merges_data_sync() {
        // Note: merge functionality is only present on the Sync variant.
        // This is not a design limitation, we just do not need it on the other.
        let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

        observations1.insert(-1000, 1);
        observations1.insert(0, 2);
        observations1.insert(11, 3);
        observations1.insert(1111, 4);

        let observations2 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

        observations2.insert(-1000, 10);
        observations2.insert(0, 10);
        observations2.insert(11, 10);
        observations2.insert(1111, 10);

        observations1.merge_from(&observations2);

        let snapshot = observations1.snapshot();

        assert_eq!(snapshot.count, 10 + 40);
        assert_eq!(
            snapshot.sum,
            (1111 * 4 + 11 * 3 - 1000) + 10 * (1111 + 11 - 1000)
        );

        assert_eq!(snapshot.bucket_counts.len(), 5);
        assert_eq!(snapshot.bucket_counts[0], 11); // -1000
        assert_eq!(snapshot.bucket_counts[1], 0); // nothing
        assert_eq!(snapshot.bucket_counts[2], 12); // 0
        assert_eq!(snapshot.bucket_counts[3], 0); // nothing
        assert_eq!(snapshot.bucket_counts[4], 13); // 11
    }

    #[test]
    #[should_panic]
    fn snapshot_merge_with_mismatched_bucket_counts_panics() {
        let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
        let observations2 = ObservationBagSync::new(&[-100, -10, 0]);

        let mut snapshot1 = observations1.snapshot();
        let snapshot2 = observations2.snapshot();

        // This should panic because the bucket counts do not match.
        snapshot1.merge_from(&snapshot2);
    }

    #[test]
    #[should_panic]
    fn snapshot_merge_with_mismatched_bucket_magnitudes_panics() {
        let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
        let observations2 = ObservationBagSync::new(&[-100, -10, 0, 20, 100]);

        let mut snapshot1 = observations1.snapshot();
        let snapshot2 = observations2.snapshot();

        // This should panic because the bucket magnitudes do not match.
        snapshot1.merge_from(&snapshot2);
    }

    #[test]
    #[should_panic]
    fn bag_merge_with_mismatched_bucket_counts_panics() {
        // Note: merge functionality is only present on the Sync variant.
        // This is not a design limitation, we just do not need it on the other.
        let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
        let observations2 = ObservationBagSync::new(&[-100, -10, 0]);

        // This should panic because the bucket counts do not match.
        observations1.merge_from(&observations2);
    }

    #[test]
    #[should_panic]
    fn bag_merge_with_mismatched_bucket_magnitudes_panics() {
        // Note: merge functionality is only present on the Sync variant.
        // This is not a design limitation, we just do not need it on the other.
        let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
        let observations2 = ObservationBagSync::new(&[-100, -10, 0, 20, 100]);

        // This should panic because the bucket magnitudes do not match.
        observations1.merge_from(&observations2);
    }
}
