use smallvec::SmallVec;

use crate::LayoutKey;

/// Inline capacity chosen to cover the documented "handful of distinct layouts" common case
/// fully inline.
///
/// Beyond this capacity, the underlying `SmallVec`s fall back to heap allocation. Lookup
/// remains correct; only the cache locality on the cold spill path is reduced.
const INLINE_CAPACITY: usize = 8;

/// Dispatches by `LayoutKey` to an associated value, optimized for a small number of distinct
/// keys with strong locality of reference.
///
/// Storage is split into two parallel `SmallVec`s — one for keys, one for values — kept in
/// lockstep. Lookup scans the dense 8-byte `keys` slice without paying for value padding, and
/// every successful lookup moves the matching entry to position 0 ("move-to-front"). Newly
/// inserted entries are also placed at position 0. The combination keeps the most-recently
/// used key at the front, so repeated inserts of the same type — the common case in
/// `BlindPool` workloads — find their entry on the first comparison.
///
/// Iteration order of `values()` / `values_mut()` is not part of the contract.
//
// This is a deviation from the standard `BTreeMap<LayoutKey, V>` we used previously, and
// also from the obvious "sorted SmallVec + binary_search_by_key" pattern proposed in
// issue #176. Both alternatives were rejected by direct Callgrind measurement against
// this implementation: at the single-digit `N` we observe in `BlindPool` workloads,
// `BTreeMap`'s tree descent and `binary_search_by_key`'s per-iteration constant factor
// (mid computation, conditional bound updates, end-of-loop check) both lose to a tight
// load-compare-branch scan over an MRU-ordered slice.
//
// A `HashMap` (e.g. with FxHash on the 8-byte `LayoutKey`) was considered but not
// measured. It would need to beat a one-iteration linear scan plus a single equality
// check on the hot path, which seems unlikely given hash + bucket probe overhead, but
// has not been verified empirically.
//
// The keys and values live in two parallel `SmallVec`s, kept in lockstep. This is a
// deviation from the more obvious unified `SmallVec<[(LayoutKey, V); N]>` layout. The
// rationale is twofold:
//
// * The scan inside `position(|k| *k == key)` walks only 8-byte `LayoutKey`s rather than
//   `(LayoutKey, V)` pairs (~160 bytes each), so the working set scanned is ~20x smaller.
// * Sidestepping a borrow-checker (Polonius) limitation: indexing into a single tuple
//   `SmallVec` for the second access forces a fresh `get_mut(0)` with its own bounds check;
//   indexing into the parallel `values` vector after the `keys` check sees no borrow conflict.
//
// Beyond the inline capacity, both `SmallVec`s fall back to heap allocation; correctness
// is preserved, only cache locality on the cold spill path is reduced.
//
// Moving entries via `SmallVec::insert` or `SmallVec::swap` is safe: the value type
// stores its bulk data in separately allocated buffers (e.g. slab vectors) and is not
// self-referential to the enclosing struct. The two parallel vectors are always
// transformed together to preserve the lockstep invariant.
#[derive(Debug)]
pub(crate) struct LayoutDispatch<V> {
    keys: SmallVec<[LayoutKey; INLINE_CAPACITY]>,
    values: SmallVec<[V; INLINE_CAPACITY]>,
}

impl<V> LayoutDispatch<V> {
    pub(crate) fn new() -> Self {
        Self {
            keys: SmallVec::new(),
            values: SmallVec::new(),
        }
    }

    /// Returns a shared reference to the value associated with `key`, if present.
    pub(crate) fn get(&self, key: LayoutKey) -> Option<&V> {
        let idx = self.keys.iter().position(|k| *k == key)?;
        self.values.get(idx)
    }

    /// Returns a unique reference to the value associated with `key`, if present.
    pub(crate) fn get_mut(&mut self, key: LayoutKey) -> Option<&mut V> {
        let idx = self.keys.iter().position(|k| *k == key)?;
        self.values.get_mut(idx)
    }

    /// Returns the value associated with `key`, inserting one produced by `f` if not present.
    ///
    /// The matching (or newly inserted) entry is positioned at index 0, so the next call
    /// with the same key returns on the first comparison.
    pub(crate) fn get_or_insert_with<F>(&mut self, key: LayoutKey, f: F) -> &mut V
    where
        F: FnOnce() -> V,
    {
        // Fast path: the most-recently-used key is at position 0 by construction of this
        // dispatch. Copy the key out of `keys.first()` so the immutable borrow on `keys`
        // ends before the mutable borrow on `values` begins (sidesteps a Polonius
        // limitation in the current borrow-checker).
        if self.keys.first().copied() == Some(key) {
            return self
                .values
                .get_mut(0)
                .expect("keys and values are kept in lockstep; first key matched");
        }

        if let Some(idx) = self.keys.iter().position(|k| *k == key) {
            self.keys.swap(0, idx);
            self.values.swap(0, idx);
        } else {
            // Construct the value FIRST so that if `f()` panics, neither vector is
            // mutated and the lockstep invariant `keys.len() == values.len()` is
            // preserved.
            let value = f();
            self.keys.insert(0, key);
            self.values.insert(0, value);
        }

        self.values
            .get_mut(0)
            .expect("either swapped to position 0 or inserted at position 0")
    }

    /// Returns an iterator over the values in current MRU order.
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.values.iter()
    }

    /// Returns an iterator over the values for mutation, in current MRU order.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.values.iter_mut()
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
    fn repeated_get_or_insert_with_keeps_key_at_front() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        let key_a = key_from_size_align(1, 1);
        let key_b = key_from_size_align(2, 2);
        let key_c = key_from_size_align(4, 4);

        _ = dispatch.get_or_insert_with(key_a, || 10);
        _ = dispatch.get_or_insert_with(key_b, || 20);
        _ = dispatch.get_or_insert_with(key_c, || 30);

        // Calling get_or_insert_with with an existing key should move it to the front,
        // so the next call with the same key finds it without scanning the others.
        _ = dispatch.get_or_insert_with(key_a, || panic!("must not construct"));

        // Inspect MRU order via values() — key_a was last accessed, must be at front.
        let values: Vec<u32> = dispatch.values().copied().collect();
        assert_eq!(values.first().copied(), Some(10));
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
