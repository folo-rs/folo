use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{self, AtomicUsize};

// TODO: Our ref counts are always 0, 1 or 2. Could we simplify this down to a boolean?
// e.g. is_last_reference (starts false, becomes true after first decrement, second decrement
// destroys object)

/// Combines a value and a thread-safe reference count.
#[derive(Debug)]
pub(crate) struct WithRefCount<T> {
    value: T,
    ref_count: AtomicUsize,
}

impl<T> WithRefCount<T> {
    /// Creates a new reference-counted wrapper around `T` with an `initial` number of references.
    #[must_use]
    pub(crate) fn new(initial: usize, value: T) -> Self {
        Self {
            value,
            ref_count: AtomicUsize::new(initial),
        }
    }

    /// Increments the reference count.
    ///
    /// # Panics
    ///
    /// Panics if the reference count would overflow.
    ///
    /// Panics if the reference count was zero (indicating resurrection).
    #[cfg_attr(not(test), expect(dead_code, reason = "maybe useful in the future"))]
    pub(crate) fn inc_ref(&self) {
        assert_ne!(0, self.ref_count.fetch_add(1, atomic::Ordering::Acquire));
    }

    /// Decrements the reference count and returns true if this was the last reference.
    ///
    /// # Panics
    ///
    /// Panics if the reference count would underflow (go below zero).
    pub(crate) fn dec_ref(&self) -> bool {
        match self.ref_count.fetch_sub(1, atomic::Ordering::Relaxed) {
            1 => {
                // We need an Acquire fence here to ensure we have observed all writes before drop.
                // On x86 this does nothing but weaker architectures may delay writes.
                atomic::fence(atomic::Ordering::Acquire);

                true
            }
            0 => panic!(
                "reference count underflow - indicates a serious bug in reference counting logic"
            ),
            _ => false,
        }
    }

    /// Returns the current reference count.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn ref_count(&self) -> usize {
        self.ref_count.load(atomic::Ordering::Relaxed)
    }
}

impl<T> Deref for WithRefCount<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

/// Combines a value and a single-threaded reference count.
#[derive(Debug)]
pub(crate) struct LocalWithRefCount<T> {
    value: T,
    ref_count: Cell<usize>,

    _single_threaded: PhantomData<*const ()>,
}

impl<T> LocalWithRefCount<T> {
    /// Creates a new reference-counted wrapper around `T` with an `initial` number of references.
    #[must_use]
    pub(crate) fn new(initial: usize, value: T) -> Self {
        Self {
            value,
            ref_count: Cell::new(initial),
            _single_threaded: PhantomData,
        }
    }

    /// Increments the reference count.
    ///
    /// # Panics
    ///
    /// Panics if the reference count would overflow.
    ///
    /// Panics if the reference count was zero (indicating resurrection).
    #[cfg_attr(not(test), expect(dead_code, reason = "maybe useful in the future"))]
    pub(crate) fn inc_ref(&self) {
        let previous = self.ref_count.get();

        assert_ne!(0, previous);

        self.ref_count.set(previous.checked_add(1).expect(
            "reference count overflow - indicates a serious bug in reference counting logic",
        ));
    }

    /// Decrements the reference count and returns true if this was the last reference.
    ///
    /// # Panics
    ///
    /// Panics if the reference count would underflow (go below zero).
    pub(crate) fn dec_ref(&self) -> bool {
        let previous = self.ref_count.get();

        let new = previous.checked_sub(1).expect(
            "reference count underflow - indicates a serious bug in reference counting logic",
        );

        self.ref_count.set(new);
        new == 0
    }

    /// Returns the current reference count.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn ref_count(&self) -> usize {
        self.ref_count.get()
    }
}

impl<T> Deref for LocalWithRefCount<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    #[test]
    fn with_ref_count_basic() {
        let wrapper = WithRefCount::new(1, 42);
        assert_eq!(*wrapper, 42);
        assert_eq!(wrapper.ref_count(), 1);

        wrapper.inc_ref();
        assert_eq!(wrapper.ref_count(), 2);

        assert!(!wrapper.dec_ref());
        assert_eq!(wrapper.ref_count(), 1);

        assert!(wrapper.dec_ref()); // Last reference dropped.
        assert_eq!(wrapper.ref_count(), 0);
    }

    #[test]
    fn local_with_ref_count_basic() {
        let wrapper = LocalWithRefCount::new(1, 42);
        assert_eq!(*wrapper, 42);
        assert_eq!(wrapper.ref_count(), 1);

        wrapper.inc_ref();
        assert_eq!(wrapper.ref_count(), 2);

        assert!(!wrapper.dec_ref());
        assert_eq!(wrapper.ref_count(), 1);

        assert!(wrapper.dec_ref()); // Last reference dropped.
        assert_eq!(wrapper.ref_count(), 0);
    }

    #[test]
    fn thread_safety() {
        assert_impl_all!(WithRefCount<usize>: Send, Sync);
        assert_not_impl_any!(LocalWithRefCount<usize>: Send, Sync);
    }
}
