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

    /// Bitmap indicating which buckets have been modified since the last `copy_from`.
    ///
    /// Bit `i` (for `i < DIRTY_BUCKETS_OVERFLOW_INDEX`) is set when bucket at index `i`
    /// has been incremented by a non-zero observation. The highest bit
    /// (`DIRTY_BUCKETS_OVERFLOW_INDEX`) is a catch-all that is set when any bucket at
    /// that index or higher is modified. `copy_from` consumes (reads and clears) this
    /// bitmap to skip stores for buckets that have not changed since the previous push.
    ///
    /// Observations with `count == 0` short-circuit before reaching the bucket-update
    /// path, so the dirty bit is only set when the corresponding bucket count actually
    /// changes.
    dirty_buckets: Cell<u64>,
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
            dirty_buckets: Cell::new(0),
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

    /// Returns the current count of observations recorded in this bag.
    ///
    /// The count is incremented monotonically by every observation (by the batch
    /// size, which is non-zero for any data-changing observation). `MetricsPusher`
    /// uses this as a dirty indicator to skip pushing pairs that have not received
    /// new observations since the last push.
    pub(crate) fn count(&self) -> u64 {
        self.count.get()
    }

    /// Reads the dirty-bucket bitmap and clears it.
    ///
    /// Used by `ObservationBagSync::copy_from` to determine which buckets need to be
    /// copied to the global bag without iterating over buckets that have not been
    /// modified since the previous copy. See `dirty_buckets` for the bit encoding.
    pub(crate) fn take_dirty_buckets(&self) -> u64 {
        let bits = self.dirty_buckets.get();
        self.dirty_buckets.set(0);
        bits
    }
}

/// Maximum bucket index that gets its own bit in the per-bag dirty bitmap. Bucket
/// indices at or above this value are coalesced into the highest bit, which acts as
/// a catch-all that causes `copy_from` to scan all buckets at or above the threshold.
/// 64-bucket histograms are not anticipated in practice, so this is "good enough" for
/// realistic workloads.
const DIRTY_BUCKETS_OVERFLOW_INDEX: usize = 63;

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
        debug_assert_eq!(self.bucket_magnitudes, other.bucket_magnitudes);

        // Both bags derive `bucket_counts` length from `bucket_magnitudes` length at
        // construction. We enforce length match in release builds because the unsafe
        // iteration below depends on this invariant for soundness.
        assert!(
            self.bucket_counts.len() == other.bucket_counts.len(),
            "bucket_counts length invariant must hold"
        );

        for (i, other_bucket_count) in other.bucket_counts.iter().enumerate() {
            // SAFETY: The `assert!` above guarantees
            // `self.bucket_counts.len() == other.bucket_counts.len()`, and `i` is produced
            // by `enumerate()` over `other.bucket_counts`, so `i < other.bucket_counts.len()
            // == self.bucket_counts.len()`. The index is therefore in bounds.
            let target = unsafe { self.bucket_counts.get_unchecked(i) };

            target.fetch_add(
                other_bucket_count.load(SYNC_BAG_ACCESS_ORDERING),
                SYNC_BAG_ACCESS_ORDERING,
            );
        }
    }

    /// Replaces the data in the bag with the data from the local observation bag.
    ///
    /// Only buckets that have been modified in `data` since the previous `copy_from`
    /// are stored; the rest are left as they were. Reads (and clears) `data`'s dirty
    /// bitmap as part of the copy.
    pub(crate) fn copy_from(&self, data: &ObservationBag) {
        // We cannot replace with a snapshot with different bucket magnitudes.
        debug_assert_eq!(self.bucket_magnitudes, data.bucket_magnitudes);

        // Both bags derive `bucket_counts` length from `bucket_magnitudes` length at
        // construction. We enforce length match in release builds because the unsafe
        // iteration below depends on this invariant for soundness.
        assert!(
            self.bucket_counts.len() == data.bucket_counts.len(),
            "bucket_counts length invariant must hold"
        );

        self.count.store(data.count.get(), SYNC_BAG_ACCESS_ORDERING);
        self.sum.store(data.sum.get(), SYNC_BAG_ACCESS_ORDERING);

        let dirty = data.take_dirty_buckets();
        let mut dirty = self.drain_overflow_buckets(data, dirty);

        // Iterate the remaining set bits one at a time.
        while dirty != 0 {
            let i = dirty.trailing_zeros() as usize;
            dirty = clear_lowest_set_bit(dirty);

            // SAFETY: Bit `i` (with `i < DIRTY_BUCKETS_OVERFLOW_INDEX`) is only set by
            // `ObservationBag::insert` when a bucket at exactly index `i` was modified,
            // which requires `data.bucket_counts.len() > i`. The access is therefore
            // in bounds for `data.bucket_counts`.
            let source = unsafe { data.bucket_counts.get_unchecked(i) };
            // SAFETY: The `assert!` above guarantees
            // `self.bucket_counts.len() == data.bucket_counts.len()`, so `i` is also
            // in bounds for `self.bucket_counts`.
            let target = unsafe { self.bucket_counts.get_unchecked(i) };
            target.store(source.get(), SYNC_BAG_ACCESS_ORDERING);
        }
    }

    /// Copies buckets at indices `>= DIRTY_BUCKETS_OVERFLOW_INDEX` from `data` into
    /// `self` when the overflow bit is set in `dirty`. Returns `dirty` with the
    /// overflow bit cleared so the caller can iterate the remaining per-bucket bits.
    ///
    /// Mutation testing on this helper is suppressed because the mutations the
    /// tool generates here (`&` -> `|`, `&` -> `^`, `&=` -> `|=` on the overflow
    /// mask handling) all degrade to over-iteration of bucket stores. The
    /// redundant stores copy `source` buckets that already match the destination,
    /// leaving observable behavior unchanged in any state reachable through the
    /// public API. Catching them would require reaching into private fields to
    /// construct a state where `source.bucket_counts` and `self.bucket_counts`
    /// disagree in buckets that were not marked dirty - a condition no normal
    /// caller can produce. The function body is small and trivially reviewable.
    #[cfg_attr(test, mutants::skip)]
    fn drain_overflow_buckets(&self, data: &ObservationBag, dirty: u64) -> u64 {
        // Caller (`copy_from`) asserts the length invariant we rely on below.
        debug_assert_eq!(self.bucket_counts.len(), data.bucket_counts.len());

        let overflow_mask = 1_u64 << DIRTY_BUCKETS_OVERFLOW_INDEX;
        if dirty & overflow_mask == 0 {
            return dirty;
        }

        for i in DIRTY_BUCKETS_OVERFLOW_INDEX..data.bucket_counts.len() {
            // SAFETY: The loop bound is `data.bucket_counts.len()`, so `i` is in
            // bounds for `data.bucket_counts`.
            let source = unsafe { data.bucket_counts.get_unchecked(i) };
            // SAFETY: The caller's `assert!` guarantees
            // `self.bucket_counts.len() == data.bucket_counts.len()`, so `i` is
            // also in bounds for `self.bucket_counts`.
            let target = unsafe { self.bucket_counts.get_unchecked(i) };
            target.store(source.get(), SYNC_BAG_ACCESS_ORDERING);
        }

        dirty & !overflow_mask
    }
}

/// Clears the lowest set bit of `value` (Brian Kernighan's bit-clear trick).
///
/// Extracted into a dedicated function so we can suppress mutation testing on it.
/// Mutating the `&` to `|` produces a no-op (the bit-iteration loop never makes
/// progress), which leads to an infinite loop in `copy_from`. Mutation testing
/// runs with the watchdog disabled, so the hang manifests as a timeout rather
/// than a normal test failure. The function is trivially correct.
#[cfg_attr(test, mutants::skip)]
const fn clear_lowest_set_bit(value: u64) -> u64 {
    value & value.wrapping_sub(1)
}

impl Observations for ObservationBag {
    fn insert(&self, magnitude: Magnitude, count: usize) {
        // No-op observations would not change any field anyway, but exiting early also
        // ensures the dirty-bucket bitmap is not polluted with bits whose buckets did
        // not actually change. That would force the next `copy_from` to perform stores
        // for buckets that hold the same value as before.
        if count == 0 {
            return;
        }

        // Crate policy is to not panic but instead to mangle data upon mathematical
        // challenges and edge cases that cannot be correctly handled. We apply this here
        // by using "as" yolo-casting. If it works, great. If not, too bad.
        let count_u64 = count as u64;
        #[expect(
            clippy::cast_possible_wrap,
            reason = "wrapping is intentional - see above comment"
        )]
        let count_i64 = count as i64;

        // For arithmetic, we use wrapping because it is the fastest and we are allowed to mangle.
        let sum_increment = magnitude.wrapping_mul(count_i64);

        self.count.set(self.count.get().wrapping_add(count_u64));
        self.sum.set(self.sum.get().wrapping_add(sum_increment));

        // This may be none if we have no buckets (i.e. the event is a bare counter,
        // no histogram).
        //
        // We benchmarked a manual SIMD (AVX2/SSE4.2) branchless "count less-than"
        // approach against this scalar linear scan. The scalar version wins across
        // all scenarios because branch prediction is highly effective for sorted
        // bucket lookups and SIMD setup overhead (broadcast, compare, mask, popcnt)
        // exceeds the cost of a well-predicted scalar loop:
        //
        //   Scenario                  SIMD     Scalar
        //   small_5_hit_first         6.1 ns   1.2 ns
        //   small_5_hit_last          5.9 ns   3.3 ns
        //   large_32_hit_first       17.3 ns   1.3 ns
        //   large_32_hit_last        17.6 ns   9.2 ns
        //   large_32_miss            17.4 ns  10.6 ns
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
            let bucket_count = unsafe { self.bucket_counts.get_unchecked(bucket_index) };
            bucket_count.set(bucket_count.get().wrapping_add(count_u64));

            // Mark this bucket as dirty so that the next `copy_from` knows to copy it.
            // Bucket indices at or above the overflow threshold share the highest bit.
            let dirty_bit = bucket_index.min(DIRTY_BUCKETS_OVERFLOW_INDEX);
            self.dirty_buckets
                .set(self.dirty_buckets.get() | (1_u64 << dirty_bit));
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
        // by using "as" yolo-casting. If it works, great. If not, too bad.
        let count_u64 = count as u64;
        #[expect(
            clippy::cast_possible_wrap,
            reason = "wrapping is intentional - see above comment"
        )]
        let count_i64 = count as i64;

        // For arithmetic, we use wrapping because it is the fastest and we are allowed to mangle.
        let sum_increment = magnitude.wrapping_mul(count_i64);

        // These operations always use wrapping arithmetic.
        self.count.fetch_add(count_u64, SYNC_BAG_ACCESS_ORDERING);
        self.sum.fetch_add(sum_increment, SYNC_BAG_ACCESS_ORDERING);

        // This may be none if we have no buckets (i.e. the event is a bare counter,
        // no histogram).
        //
        // We benchmarked a manual SIMD (AVX2/SSE4.2) branchless "count less-than"
        // approach against this scalar linear scan. The scalar version wins across
        // all scenarios because branch prediction is highly effective for sorted
        // bucket lookups and SIMD setup overhead (broadcast, compare, mask, popcnt)
        // exceeds the cost of a well-predicted scalar loop:
        //
        //   Scenario                  SIMD     Scalar
        //   small_5_hit_first         6.1 ns   1.2 ns
        //   small_5_hit_last          5.9 ns   3.3 ns
        //   large_32_hit_first       17.3 ns   1.3 ns
        //   large_32_hit_last        17.6 ns   9.2 ns
        //   large_32_miss            17.4 ns  10.6 ns
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::sync::Arc;
    use std::{iter, thread};

    use super::*;

    static_assertions::assert_impl_all!(ObservationBagSync: Send, Sync);

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

    #[test]
    fn copy_from_transfers_non_empty_bucket_counts() {
        let source = ObservationBag::new(&[-100, -10, 0, 10, 100]);

        // Insert observations into various buckets.
        source.insert(-1000, 1); // Goes into bucket 0 (le -100)
        source.insert(-50, 2); // Goes into bucket 1 (le -10)
        source.insert(0, 3); // Goes into bucket 2 (le 0)
        source.insert(5, 4); // Goes into bucket 3 (le 10)
        source.insert(50, 5); // Goes into bucket 4 (le 100)
        source.insert(1000, 6); // Goes outside any bucket (>100)

        let target = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

        // Verify target starts empty.
        let snapshot_before = target.snapshot();
        assert_eq!(snapshot_before.count, 0);
        assert_eq!(snapshot_before.sum, 0);
        for &count in &snapshot_before.bucket_counts {
            assert_eq!(count, 0);
        }

        // Copy data from source to target.
        target.copy_from(&source);

        // Verify all data was transferred correctly.
        let snapshot_after = target.snapshot();

        // Total count: 1+2+3+4+5+6 = 21
        assert_eq!(snapshot_after.count, 21);

        // Total sum: -1000*1 + -50*2 + 0*3 + 5*4 + 50*5 + 1000*6 = 5170
        assert_eq!(snapshot_after.sum, 5170);

        // Verify bucket counts.
        assert_eq!(snapshot_after.bucket_counts.len(), 5);
        assert_eq!(snapshot_after.bucket_counts[0], 1); // le -100
        assert_eq!(snapshot_after.bucket_counts[1], 2); // le -10
        assert_eq!(snapshot_after.bucket_counts[2], 3); // le 0
        assert_eq!(snapshot_after.bucket_counts[3], 4); // le 10
        assert_eq!(snapshot_after.bucket_counts[4], 5); // le 100
        // Note: observations with magnitude 1000 do not go into any bucket.
    }

    #[test]
    fn copy_from_handles_repeated_observations_on_same_bucket() {
        // Observing the same bucket multiple times must leave that bucket dirty
        // exactly once - the dirty bit is set via `|=`, so subsequent observations
        // are idempotent with respect to the bitmap. A mutation that replaces `|`
        // with `^` would XOR the bit off on the second observation, causing
        // `copy_from` to skip the bucket even though its accumulated value changed.
        let source = ObservationBag::new(&[10]);
        let target = ObservationBagSync::new(&[10]);

        source.insert(5, 1);
        source.insert(5, 1);

        target.copy_from(&source);

        let snapshot = target.snapshot();
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 10);
        assert_eq!(snapshot.bucket_counts[0], 2);
    }

    #[test]
    fn copy_from_handles_overflow_bucket_indices() {
        // Build a histogram with more than `DIRTY_BUCKETS_OVERFLOW_INDEX` buckets so
        // that observations targeting the overflow region exercise the overflow
        // catch-all branch in `copy_from`. We use 70 buckets at strictly increasing
        // magnitudes so each insert lands in a distinct, known bucket.
        const BUCKET_COUNT: usize = 70;
        static MAGNITUDES: [Magnitude; BUCKET_COUNT] = {
            let mut arr = [0_i64; BUCKET_COUNT];
            let mut i = 0;
            while i < BUCKET_COUNT {
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "small bucket index, wrapping is not possible"
                )]
                let m = i as Magnitude;
                arr[i] = m;
                i += 1;
            }
            arr
        };

        let source = ObservationBag::new(&MAGNITUDES);
        let target = ObservationBagSync::new(&MAGNITUDES);

        // Observe one value below the overflow threshold and two at or above it,
        // so that both the non-overflow loop and the overflow scan must run.
        source.insert(MAGNITUDES[10], 1); // bucket 10 (below overflow)
        source.insert(MAGNITUDES[63], 2); // bucket 63 (overflow boundary)
        source.insert(MAGNITUDES[BUCKET_COUNT - 1], 3); // last bucket (above overflow)

        target.copy_from(&source);

        let snapshot = target.snapshot();
        assert_eq!(snapshot.count, 6);
        assert_eq!(snapshot.bucket_counts[10], 1);
        assert_eq!(snapshot.bucket_counts[63], 2);
        assert_eq!(snapshot.bucket_counts[BUCKET_COUNT - 1], 3);

        // All other buckets must remain untouched - the overflow path must not
        // bleed writes into buckets that were never observed.
        for (i, &count) in snapshot.bucket_counts.iter().enumerate() {
            let expected = match i {
                10 => 1,
                63 => 2,
                i if i == BUCKET_COUNT - 1 => 3,
                _ => 0,
            };
            assert_eq!(count, expected, "bucket {i} mismatch");
        }
    }

    #[test]
    fn insert_with_zero_count_does_not_mark_dirty() {
        // A no-op observation must not pollute the dirty-bucket bitmap. Otherwise
        // a later `copy_from` would perform a redundant atomic store for a bucket
        // whose value never changed.
        let bag = ObservationBag::new(&[10]);

        bag.insert(5, 0);

        assert_eq!(bag.count(), 0);
        assert_eq!(bag.take_dirty_buckets(), 0);
        let snapshot = bag.snapshot();
        assert_eq!(snapshot.bucket_counts[0], 0);
    }

    // Multithreaded tests exercising concurrent access patterns on ObservationBagSync.
    // These are designed to run under Miri (via miri-harder) to detect data races.

    #[test]
    fn sync_concurrent_inserts_accumulate_correctly() {
        const THREADS: usize = 4;
        const INSERTS_PER_THREAD: usize = 10;
        const MAGNITUDE: Magnitude = 7;

        let bag = Arc::new(ObservationBagSync::new(&[]));

        let handles: Vec<_> = iter::repeat_with(|| {
            let bag = Arc::clone(&bag);
            thread::spawn(move || {
                for _ in 0..INSERTS_PER_THREAD {
                    bag.insert(MAGNITUDE, 1);
                }
            })
        })
        .take(THREADS)
        .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = bag.snapshot();
        assert_eq!(snapshot.count, (THREADS * INSERTS_PER_THREAD) as u64);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "small test value, wrapping is not possible"
        )]
        let expected_sum = (THREADS * INSERTS_PER_THREAD) as i64 * MAGNITUDE;
        assert_eq!(snapshot.sum, expected_sum);
    }

    #[test]
    fn sync_concurrent_inserts_with_histogram_accumulate_correctly() {
        const THREADS: usize = 4;
        const INSERTS_PER_THREAD: usize = 10;

        let bag = Arc::new(ObservationBagSync::new(&[-100, -10, 0, 10, 100]));

        let handles: Vec<_> = iter::repeat_with(|| {
            let bag = Arc::clone(&bag);
            thread::spawn(move || {
                for _ in 0..INSERTS_PER_THREAD {
                    // Magnitude 5 should land in the "le 10" bucket (index 3).
                    bag.insert(5, 1);
                }
            })
        })
        .take(THREADS)
        .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = bag.snapshot();
        let total = (THREADS * INSERTS_PER_THREAD) as u64;
        assert_eq!(snapshot.count, total);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "small test value, wrapping is not possible"
        )]
        let total_i64 = total as i64;
        assert_eq!(snapshot.sum, total_i64 * 5);
        assert_eq!(snapshot.bucket_counts[3], total);
    }

    #[test]
    fn sync_concurrent_insert_and_snapshot() {
        // One thread inserts observations while another takes snapshots.
        // This exercises the concurrent read/write paths that
        // ObservationBagSync is designed for. Miri will detect any
        // data races on the atomic operations.
        let bag = Arc::new(ObservationBagSync::new(&[-100, -10, 0, 10, 100]));

        let bag_writer = Arc::clone(&bag);
        let bag_reader = Arc::clone(&bag);

        let writer = thread::spawn(move || {
            for _ in 0..20 {
                bag_writer.insert(5, 1);
            }
        });

        let reader = thread::spawn(move || {
            for _ in 0..20 {
                let _snapshot = bag_reader.snapshot();
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();

        // After both threads complete, the final snapshot must be fully consistent.
        let snapshot = bag.snapshot();
        assert_eq!(snapshot.count, 20);
        assert_eq!(snapshot.sum, 100);
    }

    #[test]
    fn sync_concurrent_merge_from_while_inserting() {
        // One thread inserts into a source bag while another merges
        // from the source into a target. This exercises concurrent
        // reads on the source bag via merge_from.
        let source = Arc::new(ObservationBagSync::new(&[-100, 0, 100]));
        let target = Arc::new(ObservationBagSync::new(&[-100, 0, 100]));

        let source_writer = Arc::clone(&source);
        let source_reader = Arc::clone(&source);
        let target_merger = Arc::clone(&target);

        let writer = thread::spawn(move || {
            for _ in 0..20 {
                source_writer.insert(5, 1);
            }
        });

        let merger = thread::spawn(move || {
            for _ in 0..5 {
                target_merger.merge_from(&source_reader);
            }
        });

        writer.join().unwrap();
        merger.join().unwrap();
    }
}
