use std::collections::BTreeMap;

use crate::LayoutKey;

/// Dispatches by `LayoutKey` to an associated value.
///
/// This is the BTreeMap-backed baseline used for benchmarking purposes — it exposes the same
/// API as the production MTF SmallVec implementation so the same Criterion benches can be
/// run against both to compare wall-clock numbers.
#[derive(Debug)]
pub(crate) struct LayoutDispatch<V> {
    entries: BTreeMap<LayoutKey, V>,
}

impl<V> LayoutDispatch<V> {
    pub(crate) fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Returns a shared reference to the value associated with `key`, if present.
    pub(crate) fn get(&self, key: LayoutKey) -> Option<&V> {
        self.entries.get(&key)
    }

    /// Returns a unique reference to the value associated with `key`, if present.
    pub(crate) fn get_mut(&mut self, key: LayoutKey) -> Option<&mut V> {
        self.entries.get_mut(&key)
    }

    /// Returns the value associated with `key`, inserting one produced by `f` if not present.
    pub(crate) fn get_or_insert_with<F>(&mut self, key: LayoutKey, f: F) -> &mut V
    where
        F: FnOnce() -> V,
    {
        self.entries.entry(key).or_insert_with(f)
    }

    /// Returns an iterator over the values.
    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values()
    }

    /// Returns an iterator over the values for mutation.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.values_mut()
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
    fn get_returns_inserted_value() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();
        let key = key_from_size_align(4, 4);

        _ = dispatch.get_or_insert_with(key, || 42);

        assert_eq!(dispatch.get(key), Some(&42));
    }

    #[test]
    fn get_returns_none_for_absent_key() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        _ = dispatch.get_or_insert_with(key_from_size_align(4, 4), || 42);

        assert!(dispatch.get(key_from_size_align(8, 8)).is_none());
    }

    #[test]
    fn get_or_insert_with_returns_existing_without_calling_constructor() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();
        let key = key_from_size_align(4, 4);

        _ = dispatch.get_or_insert_with(key, || 42);
        let value = dispatch.get_or_insert_with(key, || panic!("must not be called"));

        assert_eq!(*value, 42);
    }

    #[test]
    fn insertions_are_retained_regardless_of_key_order() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        _ = dispatch.get_or_insert_with(key_from_size_align(8, 8), || 80);
        _ = dispatch.get_or_insert_with(key_from_size_align(1, 1), || 10);
        _ = dispatch.get_or_insert_with(key_from_size_align(4, 4), || 40);
        _ = dispatch.get_or_insert_with(key_from_size_align(2, 2), || 20);
        _ = dispatch.get_or_insert_with(key_from_size_align(16, 16), || 160);

        assert_eq!(dispatch.get(key_from_size_align(1, 1)), Some(&10));
        assert_eq!(dispatch.get(key_from_size_align(2, 2)), Some(&20));
        assert_eq!(dispatch.get(key_from_size_align(4, 4)), Some(&40));
        assert_eq!(dispatch.get(key_from_size_align(8, 8)), Some(&80));
        assert_eq!(dispatch.get(key_from_size_align(16, 16)), Some(&160));
        assert_eq!(dispatch.values().count(), 5);
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
    fn constructor_panic_leaves_dispatch_unchanged() {
        let mut dispatch: LayoutDispatch<u32> = LayoutDispatch::new();

        let key_a = key_from_size_align(4, 4);
        let key_b = key_from_size_align(8, 8);

        _ = dispatch.get_or_insert_with(key_a, || 10);

        assert_panics(|| {
            _ = dispatch.get_or_insert_with(key_b, || panic!("ctor failure"));
        });

        assert_eq!(dispatch.get(key_a), Some(&10));
        assert_eq!(dispatch.get(key_b), None);
        assert_eq!(dispatch.values().count(), 1);
    }
}
