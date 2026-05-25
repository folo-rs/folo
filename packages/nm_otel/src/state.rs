//! State tracking for delta computation between collections.

use std::hash::BuildHasher;

use foldhash::fast::RandomState;
use hashbrown::HashTable;
use hashbrown::hash_table::Entry;
use nm::{EventName, Magnitude};

/// Tracks the previous state of nm metrics for delta computation.
///
/// Counter-type metrics (count, histogram buckets) are cumulative in nm but require
/// delta computation for OpenTelemetry. Gauge-type metrics (sum) are set directly.
#[derive(Debug, Default)]
pub(crate) struct CollectionState {
    // We use `hashbrown::HashTable` (not `HashMap`) so the `entry(hash, eq, hasher)` API can
    // avoid cloning `EventName` on cache hits. The natural `HashMap::entry(name.clone())` shape
    // clones on every call, which is a heap allocation per export for `Cow::Owned` event names.
    // Stable `HashMap` has no equivalent: `raw_entry_mut` is unstable, `entry_ref` needs
    // `&Q: Into<K>` (no such impl exists for `Cow<'static, str>`), and `get_mut`-first
    // early-return fails NLL borrow-check on rustc 1.93. Hashing is still done by
    // `foldhash::fast::RandomState`; only the map shell changed. The hasher must be stored on
    // the struct so the lookup-time hash and the growth-time rehash closure use the same
    // instance and therefore produce the same hash for the same key.
    hasher: RandomState,

    /// Previous state per event name.
    events: HashTable<(EventName, EventState)>,
}

impl CollectionState {
    /// Creates a new empty collection state.
    pub(crate) fn new() -> Self {
        Self {
            hasher: RandomState::default(),
            events: HashTable::new(),
        }
    }

    /// Gets or creates the state for an event.
    pub(crate) fn event_state(&mut self, name: &EventName) -> &mut EventState {
        let hash = self.hasher.hash_one(name);
        let hasher = &self.hasher;
        // The three closures fed to `entry()`:
        // 1. `hash`            - precomputed hash of the lookup key.
        // 2. `|...| ... == name` - tiebreaker on probed slots (collision check).
        // 3. `|...| hash_one`  - rehash closure, called per existing entry on table growth.
        // All three must agree on hashing; we route them through the same `RandomState`.
        match self.events.entry(
            hash,
            |(existing, _)| existing == name,
            |(existing, _)| hasher.hash_one(existing),
        ) {
            Entry::Occupied(occupied) => &mut occupied.into_mut().1,
            Entry::Vacant(vacant) => {
                // The key clone is confined to this branch — we only pay for it on a true
                // cache miss, not on the steady-state hit path.
                &mut vacant
                    .insert((name.clone(), EventState::default()))
                    .into_mut()
                    .1
            }
        }
    }
}

/// Previous state for a single event.
#[derive(Debug, Default)]
pub struct EventState {
    /// Previous cumulative count.
    pub(crate) count: u64,

    /// Previous cumulative histogram bucket counts (already converted to cumulative format).
    /// Indexed by bucket index.
    pub(crate) histogram_buckets: Vec<u64>,
}

impl EventState {
    /// Computes the delta for the count metric.
    ///
    /// Returns the delta and updates internal state.
    pub(crate) fn count_delta(&mut self, current: u64) -> u64 {
        let delta = current.saturating_sub(self.count);
        self.count = current;
        delta
    }

    /// Computes deltas for histogram bucket counts.
    ///
    /// Takes nm's non-cumulative bucket counts, converts to cumulative format,
    /// computes deltas from previous state, and updates internal state. All
    /// computation is streaming so the steady-state path performs no heap
    /// allocations.
    ///
    /// Returns an iterator yielding `(magnitude, cumulative_count, delta)` for each bucket
    /// in input order. Internal state is only updated as the iterator is consumed, so the
    /// returned iterator must be fully driven for the next call to observe the deltas
    /// correctly.
    ///
    /// # Panics
    ///
    /// Panics during iteration if the number of buckets yielded differs from the count
    /// established on the first call. Histogram bucket configuration is expected to be
    /// fixed for the lifetime of an event. The check fires either when an extra bucket
    /// is yielded beyond the established length, or when the source iterator is exhausted
    /// before all established buckets have been visited.
    pub fn histogram_deltas<'a>(
        &'a mut self,
        magnitudes: impl IntoIterator<Item = Magnitude> + 'a,
        non_cumulative_counts: impl IntoIterator<Item = u64> + 'a,
    ) -> impl Iterator<Item = (Magnitude, u64, u64)> + 'a {
        let first_call = self.histogram_buckets.is_empty();
        let source = magnitudes.into_iter().zip(non_cumulative_counts);
        if first_call {
            // Reserve capacity upfront so the per-bucket `push(0)` on the first call only
            // costs a write, not a reallocation. For sized inputs (arrays, slices, Vecs) the
            // upper bound is exact and we get a single allocation; otherwise we fall back to
            // the lower bound and `push` grows on demand.
            let (lower, upper) = source.size_hint();
            let reserve_hint = upper.unwrap_or(lower);
            self.histogram_buckets.reserve_exact(reserve_hint);
        }
        HistogramDeltas {
            source,
            buckets: &mut self.histogram_buckets,
            first_call,
            running_cumulative: 0,
            index: 0,
        }
    }
}

/// Streaming iterator returned by [`EventState::histogram_deltas`].
///
/// On the first call `buckets` starts empty and is grown lazily as items are yielded.
/// On subsequent calls `buckets` already has the established length and we index into
/// it in lockstep with the source iterator, panicking if the yielded count drifts.
struct HistogramDeltas<'a, I> {
    source: I,
    buckets: &'a mut Vec<u64>,
    first_call: bool,
    running_cumulative: u64,
    index: usize,
}

impl<I> Iterator for HistogramDeltas<'_, I>
where
    I: Iterator<Item = (Magnitude, u64)>,
{
    type Item = (Magnitude, u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        let Some((magnitude, non_cumulative)) = self.source.next() else {
            // Source exhausted: verify we visited every previously-established bucket.
            // On the first call there is no established length yet, so anything goes.
            assert!(
                self.first_call || self.index == self.buckets.len(),
                "histogram bucket count changed unexpectedly: source yielded {} buckets, \
                 but {} were established on the first call",
                self.index,
                self.buckets.len()
            );
            return None;
        };

        self.running_cumulative = self.running_cumulative.saturating_add(non_cumulative);

        let previous = if self.first_call {
            self.push_initial_bucket();
            0
        } else {
            assert!(
                self.index < self.buckets.len(),
                "histogram bucket count changed unexpectedly: source yielded at least {} \
                 buckets, but only {} were established on the first call",
                self.index.saturating_add(1),
                self.buckets.len()
            );
            #[expect(
                clippy::indexing_slicing,
                reason = "index is bounds-checked by the assertion above"
            )]
            let previous = self.buckets[self.index];
            previous
        };

        let delta = self.running_cumulative.saturating_sub(previous);

        #[expect(
            clippy::indexing_slicing,
            reason = "on first call we just pushed; otherwise the assertion above \
                      verified the index is in bounds"
        )]
        {
            self.buckets[self.index] = self.running_cumulative;
        }
        self.index = self.index.saturating_add(1);

        Some((magnitude, self.running_cumulative, delta))
    }
}

impl<I> HistogramDeltas<'_, I> {
    /// Appends a zero entry to grow `buckets` during the first collection.
    ///
    /// Marked `#[cold]` because the first-call path is only ever taken during the
    /// very first `histogram_deltas` invocation for an event; every subsequent call
    /// takes the steady-state validation branch. Biasing branch layout this way keeps
    /// the steady-state path as the straight-line fall-through.
    #[cold]
    #[inline(never)]
    fn push_initial_bucket(&mut self) {
        self.buckets.push(0);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Caps iterator consumption in these tests. If `HistogramDeltas::next` ever fails to
    /// terminate (for example, due to a mutation-testing substitution that replaces the body
    /// with `Some(Default::default())`), bounded consumption surfaces the defect as a
    /// length-mismatch failure instead of letting the test hang.
    const HISTOGRAM_ITER_SAFETY_BOUND: usize = 8;

    #[test]
    fn event_state_count_delta_first_collection() {
        let mut state = EventState::default();
        let delta = state.count_delta(100);
        assert_eq!(delta, 100);
        assert_eq!(state.count, 100);
    }

    #[test]
    fn event_state_count_delta_subsequent_collections() {
        let mut state = EventState::default();

        let delta1 = state.count_delta(100);
        assert_eq!(delta1, 100);

        let delta2 = state.count_delta(150);
        assert_eq!(delta2, 50);

        let delta3 = state.count_delta(150);
        assert_eq!(delta3, 0);

        let delta4 = state.count_delta(200);
        assert_eq!(delta4, 50);
    }

    #[test]
    fn event_state_histogram_deltas_first_collection() {
        let mut state = EventState::default();

        let magnitudes = [10, 50, 100, Magnitude::MAX];
        let non_cumulative = [5, 12, 8, 2];

        let result: Vec<_> = state
            .histogram_deltas(magnitudes, non_cumulative)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .collect();

        // First collection: deltas equal cumulative values.
        assert_eq!(result.len(), 4);
        assert_eq!(
            result,
            vec![
                (10, 5, 5),
                (50, 17, 17),
                (100, 25, 25),
                (Magnitude::MAX, 27, 27),
            ]
        );

        // First call must reserve enough capacity to hold every bucket without growth, so
        // the steady-state path (and the alloc-tracker integration test) sees zero allocs.
        assert_eq!(state.histogram_buckets.len(), 4);
        assert!(state.histogram_buckets.capacity() >= 4);
    }

    #[test]
    fn event_state_histogram_deltas_subsequent_collections() {
        let mut state = EventState::default();

        let magnitudes = [10, 50, 100, Magnitude::MAX];

        // First collection.
        let non_cumulative1 = [5, 12, 8, 2];
        let count1 = state
            .histogram_deltas(magnitudes, non_cumulative1)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .count();
        assert_eq!(count1, 4);

        // Second collection with more observations.
        let non_cumulative2 = [7, 15, 10, 3];
        let result: Vec<_> = state
            .histogram_deltas(magnitudes, non_cumulative2)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .collect();

        // Cumulative: [7, 22, 32, 35].
        // Previous:   [5, 17, 25, 27].
        // Deltas:     [2, 5, 7, 8].
        assert_eq!(result.len(), 4);
        assert_eq!(
            result,
            vec![
                (10, 7, 2),
                (50, 22, 5),
                (100, 32, 7),
                (Magnitude::MAX, 35, 8),
            ]
        );
    }

    #[test]
    fn collection_state_creates_event_state_on_demand() {
        let mut state = CollectionState::new();

        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 0);

        event_state.count = 100;

        let event_state_again = state.event_state(&"test_event".into());
        assert_eq!(event_state_again.count, 100);
    }

    #[test]
    fn collection_state_tracks_multiple_events() {
        let mut state = CollectionState::new();

        state.event_state(&"event_a".into()).count = 10;
        state.event_state(&"event_b".into()).count = 20;

        assert_eq!(state.event_state(&"event_a".into()).count, 10);
        assert_eq!(state.event_state(&"event_b".into()).count, 20);
    }

    #[test]
    fn collection_state_preserves_entries_across_table_growth() {
        // The underlying `HashTable` grows when capacity is exceeded, which calls the
        // rehash closure passed to `entry()` on every existing entry. This test inserts
        // enough events to force multiple grows and then reads every entry back to
        // verify the rehash closure produces hashes consistent with the lookup-time
        // hashing — otherwise entries would land in the wrong buckets after a grow
        // and the reads would return fresh `EventState::default()` values instead of
        // the values we wrote.
        const NUM_EVENTS: u64 = 64;

        let mut state = CollectionState::new();

        for i in 0..NUM_EVENTS {
            // Use distinguishable non-zero values so a re-defaulted `EventState` (count = 0)
            // would be detectable.
            state.event_state(&format!("growth_event_{i}").into()).count =
                i.saturating_mul(7).saturating_add(1);
        }

        for i in 0..NUM_EVENTS {
            let expected = i.saturating_mul(7).saturating_add(1);
            assert_eq!(
                state.event_state(&format!("growth_event_{i}").into()).count,
                expected,
            );
        }
    }

    #[test]
    fn event_state_histogram_deltas_same_bucket_count_works() {
        let mut state = EventState::default();

        let magnitudes = [10, 50, 100];
        let non_cumulative1 = [5, 10, 3];

        // First call - initializes to 3 buckets.
        let count1 = state
            .histogram_deltas(magnitudes, non_cumulative1)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .count();
        assert_eq!(count1, 3);
        assert_eq!(state.histogram_buckets.len(), 3);

        // Second call with same bucket count - should work fine.
        let non_cumulative2 = [7, 12, 5];
        let result2: Vec<_> = state
            .histogram_deltas(magnitudes, non_cumulative2)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .collect();
        assert_eq!(state.histogram_buckets.len(), 3);

        // Verify deltas are computed correctly.
        // Cumulative1: [5, 15, 18], Cumulative2: [7, 19, 24].
        // Deltas: [2, 4, 6].
        assert_eq!(result2, vec![(10, 7, 2), (50, 19, 4), (100, 24, 6)]);
    }

    #[test]
    #[should_panic]
    fn event_state_histogram_deltas_bucket_count_mismatch_panics() {
        let mut state = EventState::default();

        // First call with 3 buckets.
        let magnitudes3 = [10, 50, 100];
        let non_cumulative3 = [5, 10, 3];
        state
            .histogram_deltas(magnitudes3, non_cumulative3)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .for_each(drop);

        // Second call with 4 buckets - should panic when the iterator is consumed past the
        // established bucket count.
        let magnitudes4 = [10, 50, 100, 500];
        let non_cumulative4 = [5, 10, 3, 2];
        state
            .histogram_deltas(magnitudes4, non_cumulative4)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .for_each(drop);
    }

    #[test]
    #[should_panic]
    fn event_state_histogram_deltas_fewer_buckets_panics() {
        let mut state = EventState::default();

        // First call establishes 3 buckets.
        let magnitudes3 = [10, 50, 100];
        let non_cumulative3 = [5, 10, 3];
        state
            .histogram_deltas(magnitudes3, non_cumulative3)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .for_each(drop);

        // Second call yields only 2 buckets - should panic when the source iterator
        // is exhausted before all established buckets have been visited.
        let magnitudes2 = [10, 50];
        let non_cumulative2 = [7, 12];
        state
            .histogram_deltas(magnitudes2, non_cumulative2)
            .take(HISTOGRAM_ITER_SAFETY_BOUND)
            .for_each(drop);
    }
}
