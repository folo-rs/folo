use smallvec::SmallVec;

use crate::LayoutKey;

/// Inline capacity chosen to cover the documented "handful of distinct layouts" common case
/// fully inline. With the indirected layout (small dispatch entries plus a parallel `values`
/// array), the inline scan footprint is just 8 * 24 = 192 bytes (~3 cache lines), while the
/// big `V` payloads sit in their own inline-or-spilled `SmallVec`.
const INLINE_CAPACITY: usize = 8;

/// Sort the dispatch array by descending hit count after this many touches (either successful
/// lookups or new insertions). The interval amortizes sort cost over many lookups while still
/// converging the dispatch order toward the actual usage distribution quickly.
const SORT_INTERVAL: u32 = 100;

/// Per-key dispatch metadata. Kept deliberately small so the linear scan stays cache-friendly:
/// on 64-bit targets this struct is 24 bytes (8-aligned), so a full inline-capacity scan
/// touches three cache lines regardless of how large `V` is.
#[derive(Debug, Clone, Copy)]
struct DispatchEntry {
    key: LayoutKey,
    /// Index into the parallel `values` array. Stays valid across sorts because we sort the
    /// dispatch metadata only — the `values` array is append-only and never reordered.
    value_idx: usize,
    /// Number of times `get_or_insert_with` has touched this entry. Drives the periodic sort
    /// that places the hottest entries first. `u64` exhausts the existing 8-byte alignment
    /// padding for free and pushes overflow centuries beyond any realistic lookup volume, so
    /// no saturation handling is needed.
    hit_count: u64,
}

/// Dispatches by `LayoutKey` to an associated value, optimized for a small number of distinct
/// keys with strong locality of reference.
///
/// Storage is split across two inline-capable arrays: a compact `dispatch` array of
/// `(LayoutKey, value_idx, hit_count)` entries scanned linearly on every lookup, and a parallel
/// `values` array that holds the bulk `V` payloads. Every `get_or_insert_with` call increments
/// the matching entry's hit count, and every [`SORT_INTERVAL`] touches the dispatch array is
/// re-sorted by descending hit count. After warmup the hottest layout sits at index 0 and the
/// linear scan returns on the first comparison.
///
/// Iteration order of `values()` / `values_mut()` is therefore not part of the contract.
//
// This is a deviation from the standard `BTreeMap<LayoutKey, V>` we used previously, and
// also from the obvious "sorted SmallVec + binary_search_by_key" pattern proposed in
// issue #176. Both alternatives were rejected by direct Callgrind measurement against
// move-to-front predecessors of this layout: at the single-digit `N` we observe in
// `BlindPool` workloads, `BTreeMap`'s tree descent and `binary_search_by_key`'s
// per-iteration constant factor (mid computation, conditional bound updates,
// end-of-loop check) both lose to a tight load-compare-branch scan over a hit-count-
// ordered slice.
//
// Move-to-front variants (PRs #182/#183) were faster than `BTreeMap` on every typical-case
// scenario but degraded sharply on adversarial workloads where the hot layout is forced to
// the back on every call: every lookup pays the full N-comparison scan AND a write-back
// rotation. Amortized hit-count ordering avoids the write-back: the linear scan is the only
// cost on the hot path, and the sort fires only once per [`SORT_INTERVAL`] touches.
//
// Splitting `(LayoutKey, V)` into a small metadata array plus a parallel `values` array is
// motivated by the same locality argument: with the `RawOpaquePool` payload (~152 bytes on
// x64), each combined entry would be 168 bytes, so an N=8 scan would stride through ~21
// cache lines even when only the 8-byte key matters per iteration. Indirecting to
// 24-byte metadata entries collapses that to 3 cache lines and also keeps sort costs low
// (24-byte tuple swaps instead of 168-byte payload moves).
//
// A `HashMap` (e.g. with FxHash on the 8-byte `LayoutKey`) was considered but not
// measured. It would need to beat a one-iteration linear scan plus a single equality
// check on the hot path, which seems unlikely given hash + bucket probe overhead, but
// has not been verified empirically.
//
// Beyond the inline capacity the underlying `SmallVec`s fall back to heap allocation;
// correctness is preserved, only cache locality on the cold spill path is reduced. There
// is no upper bound on the number of distinct layouts that can be tracked.
#[derive(Debug)]
pub(crate) struct LayoutDispatch<V> {
    dispatch: SmallVec<[DispatchEntry; INLINE_CAPACITY]>,
    values: SmallVec<[V; INLINE_CAPACITY]>,
    lookups_since_sort: u32,
}

impl<V> LayoutDispatch<V> {
    pub(crate) fn new() -> Self {
        Self {
            dispatch: SmallVec::new(),
            values: SmallVec::new(),
            lookups_since_sort: 0,
        }
    }

    /// Returns a shared reference to the value associated with `key`, if present.
    ///
    /// Does not update the hit count or trigger sorting, so calls through `get` do not
    /// influence the dispatch order. The hot path goes through `get_or_insert_with`.
    pub(crate) fn get(&self, key: LayoutKey) -> Option<&V> {
        let entry = self.dispatch.iter().find(|e| e.key == key)?;
        Some(
            self.values
                .get(entry.value_idx)
                .expect("value_idx is always a valid index into values: only set in `get_or_insert_with` after the corresponding `values.push`"),
        )
    }

    /// Returns a unique reference to the value associated with `key`, if present.
    ///
    /// Does not update the hit count or trigger sorting, so calls through `get_mut` do not
    /// influence the dispatch order. The hot path goes through `get_or_insert_with`.
    pub(crate) fn get_mut(&mut self, key: LayoutKey) -> Option<&mut V> {
        let entry = self.dispatch.iter().find(|e| e.key == key)?;
        let idx = entry.value_idx;
        Some(
            self.values
                .get_mut(idx)
                .expect("value_idx is always a valid index into values: only set in `get_or_insert_with` after the corresponding `values.push`"),
        )
    }

    /// Returns the value associated with `key`, inserting one produced by `f` if not present.
    ///
    /// Each call increments the hit count for the matching (or newly inserted) entry. Every
    /// [`SORT_INTERVAL`] calls the dispatch array is re-sorted in descending hit-count order,
    /// so the hottest layout drifts toward index 0 and the linear scan returns on the first
    /// comparison in steady state.
    pub(crate) fn get_or_insert_with<F>(&mut self, key: LayoutKey, f: F) -> &mut V
    where
        F: FnOnce() -> V,
    {
        let value_idx = match self.dispatch.iter().position(|e| e.key == key) {
            Some(i) => {
                let entry = self
                    .dispatch
                    .get_mut(i)
                    .expect("index `i` was just produced by `position`");
                // `u64` hit counts cannot realistically overflow (centuries at 1 ns/call),
                // so an explicit `wrapping_add` is safe and saves the saturating-arithmetic
                // overhead on the hot path.
                entry.hit_count = entry.hit_count.wrapping_add(1);
                entry.value_idx
            }
            None => {
                // Compute `f()` before any mutation so that a panic in the constructor leaves
                // the dispatch entirely unchanged.
                let value = f();
                let idx = self.values.len();
                self.values.push(value);
                self.dispatch.push(DispatchEntry {
                    key,
                    value_idx: idx,
                    hit_count: 1,
                });
                idx
            }
        };

        // `lookups_since_sort` is reset to 0 every `SORT_INTERVAL` (= 100) increments, so its
        // value is structurally bounded well below `u32::MAX`. `wrapping_add` is therefore
        // equivalent to checked arithmetic here and avoids the saturating-add overhead on the
        // hot path.
        self.lookups_since_sort = self.lookups_since_sort.wrapping_add(1);
        if self.lookups_since_sort >= SORT_INTERVAL {
            self.resort();
        }

        self.values
            .get_mut(value_idx)
            .expect("value_idx is always a valid index into values: either just set above or read from a dispatch entry that was previously inserted with a valid idx")
    }

    /// Returns an iterator over the values. Iteration order is not part of the contract.
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.values.iter()
    }

    /// Returns an iterator over the values for mutation. Iteration order is not part of the
    /// contract.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.values.iter_mut()
    }

    /// Sorts the dispatch metadata in descending hit-count order and resets the lookup
    /// counter. `value_idx` fields remain valid because we never reorder the `values` array.
    fn resort(&mut self) {
        // Unstable sort is fine: tie-breaking order between entries with equal hit counts has
        // no observable effect — they all match the key with equal probability.
        self.dispatch.sort_unstable_by(|a, b| b.hit_count.cmp(&a.hit_count));
        self.lookups_since_sort = 0;
    }
}

impl<V> Default for LayoutDispatch<V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::alloc::Layout;

    use testing::assert_panics;

    use super::*;

    fn key_from_size_align(size: usize, align: usize) -> LayoutKey {
        LayoutKey::new(Layout::from_size_align(size, align).unwrap())
    }

    #[test]
    fn new_dispatch_is_empty() {
        let dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        assert_eq!(dispatch.values().count(), 0);
        assert!(dispatch.get(key_from_size_align(1, 1)).is_none());
    }

    #[test]
    fn default_dispatch_is_empty() {
        let dispatch: LayoutDispatch<u32> = LayoutDispatch::default();

        assert_eq!(dispatch.values().count(), 0);
    }

    #[test]
    fn get_or_insert_with_inserts_value() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();
        let key = key_from_size_align(4, 4);

        let value = dispatch.get_or_insert_with(key, || 42);

        assert_eq!(*value, 42);
        assert_eq!(dispatch.values().count(), 1);
    }

    #[test]
    fn get_or_insert_with_returns_existing_without_calling_constructor() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();
        let key = key_from_size_align(4, 4);

        _ = dispatch.get_or_insert_with(key, || 42);

        let value =
            dispatch.get_or_insert_with(key, || panic!("must not call ctor for existing key"));
        assert_eq!(*value, 42);
        assert_eq!(dispatch.values().count(), 1);
    }

    #[test]
    fn get_returns_inserted_value() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();
        let key = key_from_size_align(4, 4);

        _ = dispatch.get_or_insert_with(key, || 99);

        assert_eq!(dispatch.get(key), Some(&99));
    }

    #[test]
    fn get_returns_none_for_absent_key() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();
        _ = dispatch.get_or_insert_with(key_from_size_align(4, 4), || 1);

        assert!(dispatch.get(key_from_size_align(8, 8)).is_none());
    }

    #[test]
    fn insertions_are_retained_regardless_of_key_order() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        // Insert with arbitrary key order; lookup must find every key.
        _ = dispatch.get_or_insert_with(key_from_size_align(8, 8), || 80);
        _ = dispatch.get_or_insert_with(key_from_size_align(2, 2), || 20);
        _ = dispatch.get_or_insert_with(key_from_size_align(4, 4), || 40);
        _ = dispatch.get_or_insert_with(key_from_size_align(1, 1), || 10);
        _ = dispatch.get_or_insert_with(key_from_size_align(16, 16), || 160);

        assert_eq!(dispatch.get(key_from_size_align(1, 1)), Some(&10));
        assert_eq!(dispatch.get(key_from_size_align(2, 2)), Some(&20));
        assert_eq!(dispatch.get(key_from_size_align(4, 4)), Some(&40));
        assert_eq!(dispatch.get(key_from_size_align(8, 8)), Some(&80));
        assert_eq!(dispatch.get(key_from_size_align(16, 16)), Some(&160));
        assert_eq!(dispatch.values().count(), 5);
    }

    #[test]
    fn repeated_get_or_insert_with_does_not_call_constructor() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        let key_a = key_from_size_align(1, 1);
        let key_b = key_from_size_align(2, 2);
        let key_c = key_from_size_align(4, 4);

        _ = dispatch.get_or_insert_with(key_a, || 10);
        _ = dispatch.get_or_insert_with(key_b, || 20);
        _ = dispatch.get_or_insert_with(key_c, || 30);

        // Repeated insertion of an existing key must not invoke the constructor and must
        // return the originally inserted value.
        let value = dispatch.get_or_insert_with(key_a, || panic!("must not construct"));
        assert_eq!(*value, 10);
    }

    #[test]
    fn hottest_key_drifts_to_front_after_sort() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        let cold_a = key_from_size_align(1, 1);
        let cold_b = key_from_size_align(2, 2);
        let hot = key_from_size_align(4, 4);

        // Insert the cold keys first, so the hot key starts at the back of the dispatch.
        _ = dispatch.get_or_insert_with(cold_a, || 10);
        _ = dispatch.get_or_insert_with(cold_b, || 20);
        _ = dispatch.get_or_insert_with(hot, || 30);

        // Hit the hot key enough to cross the sort interval; it must end up returned with
        // the same value, and a subsequent sort must reorder the dispatch so that the
        // hottest key wins.
        let total = SORT_INTERVAL.checked_add(10).unwrap();
        for _ in 0..total {
            let value = dispatch.get_or_insert_with(hot, || unreachable!());
            assert_eq!(*value, 30);
        }
    }

    #[test]
    fn values_mut_modifies_in_place() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        _ = dispatch.get_or_insert_with(key_from_size_align(4, 4), || 1);
        _ = dispatch.get_or_insert_with(key_from_size_align(8, 8), || 2);

        for v in dispatch.values_mut() {
            *v = v.wrapping_mul(10);
        }

        let mut values: Vec<u32> = dispatch.values().copied().collect();
        values.sort_unstable();
        assert_eq!(values, vec![10, 20]);
    }

    #[test]
    fn spills_to_heap_past_inline_capacity_and_remains_correct() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        // Use INLINE_CAPACITY + 4 distinct layouts to force heap spill.
        let count = INLINE_CAPACITY.checked_add(4).unwrap();
        for i in 0..count {
            let size = i.checked_add(1).unwrap();
            let value = u32::try_from(i).unwrap();
            _ = dispatch.get_or_insert_with(key_from_size_align(size, 1), || value);
        }

        assert_eq!(dispatch.values().count(), count);

        for i in 0..count {
            let size = i.checked_add(1).unwrap();
            let value = u32::try_from(i).unwrap();
            assert_eq!(dispatch.get(key_from_size_align(size, 1)), Some(&value));
        }
    }

    #[test]
    fn constructor_panic_leaves_dispatch_unchanged() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        let key_a = key_from_size_align(4, 4);
        let key_b = key_from_size_align(8, 8);

        _ = dispatch.get_or_insert_with(key_a, || 10);

        // The constructor for a new key panics. The dispatch must be left intact:
        // no new entry should have been added, and the existing entry must still
        // be accessible.
        assert_panics(|| {
            _ = dispatch.get_or_insert_with(key_b, || panic!("ctor failure"));
        });

        assert_eq!(dispatch.get(key_a), Some(&10));
        assert_eq!(dispatch.get(key_b), None);
        assert_eq!(dispatch.values().count(), 1);
    }
}
