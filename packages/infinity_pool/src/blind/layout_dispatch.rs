use smallvec::SmallVec;

use crate::LayoutKey;

/// Inline capacity for the secondary `rest` `SmallVec` — sized so that, together with the
/// hoisted `front` slot, total inline capacity matches the documented "handful of distinct
/// layouts" common case fully inline.
///
/// Beyond this capacity the `SmallVec` falls back to heap allocation. Lookup remains correct;
/// only the cache locality on the cold spill path is reduced.
const REST_INLINE_CAPACITY: usize = 7;

/// Total inline capacity including the hoisted `front` slot.
#[cfg(test)]
const INLINE_CAPACITY: usize = REST_INLINE_CAPACITY + 1;

/// Dispatches by `LayoutKey` to an associated value, optimized for a small number of distinct
/// keys with strong locality of reference.
///
/// Storage is split into a hoisted `front: Option<(LayoutKey, V)>` slot that holds the
/// most-recently-used entry, plus a `SmallVec` of `(LayoutKey, V)` entries for the rest.
/// On every successful lookup the matching entry is promoted into `front` (the previous
/// `front` is demoted into the `SmallVec`). The combination keeps the most-recently used
/// key in a dedicated field, so repeated inserts of the same type — the common case in
/// `BlindPool` workloads — find their entry without touching the `SmallVec` at all.
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
// Beyond the inline capacity, the underlying `SmallVec` falls back to heap allocation;
// correctness is preserved, only cache locality on the cold spill path is reduced.
//
// Moving entries via `SmallVec::push` or replacing the `front` slot is safe: the value
// type stores its bulk data in separately allocated buffers (e.g. slab vectors) and is
// not self-referential to the enclosing struct.
#[derive(Debug)]
pub(crate) struct LayoutDispatch<V> {
    front: Option<(LayoutKey, V)>,
    rest: SmallVec<[(LayoutKey, V); REST_INLINE_CAPACITY]>,
}

impl<V> LayoutDispatch<V> {
    pub(crate) fn new() -> Self {
        Self {
            front: None,
            rest: SmallVec::new(),
        }
    }

    /// Returns a shared reference to the value associated with `key`, if present.
    pub(crate) fn get(&self, key: LayoutKey) -> Option<&V> {
        if let Some((k, v)) = &self.front
            && *k == key
        {
            return Some(v);
        }
        self.rest.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// Returns a unique reference to the value associated with `key`, if present.
    pub(crate) fn get_mut(&mut self, key: LayoutKey) -> Option<&mut V> {
        // Copy out the front key first so the immutable borrow does not extend into the
        // mutable-access branch (Polonius limitation in current borrow-checker).
        if self.front.as_ref().is_some_and(|(k, _)| *k == key) {
            return self.front.as_mut().map(|(_, v)| v);
        }
        self.rest
            .iter_mut()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    /// Returns the value associated with `key`, inserting one produced by `f` if not present.
    ///
    /// The matching (or newly inserted) entry is promoted into `front`, so the next call
    /// with the same key returns on the first comparison.
    pub(crate) fn get_or_insert_with<F>(&mut self, key: LayoutKey, f: F) -> &mut V
    where
        F: FnOnce() -> V,
    {
        // Fast path: the most-recently-used entry sits in the hoisted `front` slot, so a
        // single Option discriminant + key compare resolves the hit without touching the
        // SmallVec at all on the hot repeated-lookup path.
        if self.front.as_ref().is_some_and(|(k, _)| *k == key) {
            return &mut self
                .front
                .as_mut()
                .expect("front matched in the immediately preceding check")
                .1;
        }

        // Either find the entry in `rest` and promote it, or construct a new one. Either
        // way, the previous `front` (if any) is demoted into `rest`.
        let new_front = if let Some(idx) = self.rest.iter().position(|(k, _)| *k == key) {
            self.rest.swap_remove(idx)
        } else {
            (key, f())
        };
        let demoted = self.front.replace(new_front);
        if let Some(old) = demoted {
            self.rest.push(old);
        }

        &mut self
            .front
            .as_mut()
            .expect("front was just assigned via `replace`")
            .1
    }

    /// Returns an iterator over the values in current MRU order.
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.front
            .iter()
            .map(|(_, v)| v)
            .chain(self.rest.iter().map(|(_, v)| v))
    }

    /// Returns an iterator over the values for mutation, in current MRU order.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.front
            .iter_mut()
            .map(|(_, v)| v)
            .chain(self.rest.iter_mut().map(|(_, v)| v))
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
