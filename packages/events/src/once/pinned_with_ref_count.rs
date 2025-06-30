//! Reference-counted wrapper for use in pinned contexts.
//!
//! This module provides a utility type that combines a value with a reference count,
//! designed specifically for use with pinned memory pools. All mutating operations
//! take pinned references to avoid burdening callers with pinning conversions.

use std::pin::Pin;

/// Combines a value and a reference count, designed for use in pinned contexts.
/// All mutating operations take pinned references to avoid burdening callers with pinning conversions.
#[derive(Debug)]
pub(crate) struct PinnedWithRefCount<T> {
    value: T,
    ref_count: usize,
}

impl<T> PinnedWithRefCount<T> {
    /// Creates a new reference-counted wrapper with an initial reference count of 0.
    #[must_use]
    pub(crate) fn new(value: T) -> Self {
        Self {
            value,
            ref_count: 0,
        }
    }

    /// Returns a shared reference to the wrapped value.
    #[must_use]
    pub(crate) fn get(self: Pin<&Self>) -> Pin<&T> {
        // SAFETY: We're only projecting to a field and T is never moved
        unsafe { self.map_unchecked(|this| &this.value) }
    }

    /// Increments the reference count.
    pub(crate) fn inc_ref(self: Pin<&mut Self>) {
        // SAFETY: We're only modifying the ref_count field, not moving the struct
        let this = unsafe { self.get_unchecked_mut() };
        this.ref_count = this.ref_count.saturating_add(1);
    }

    /// Decrements the reference count.
    pub(crate) fn dec_ref(self: Pin<&mut Self>) {
        // SAFETY: We're only modifying the ref_count field, not moving the struct
        let this = unsafe { self.get_unchecked_mut() };
        this.ref_count = this.ref_count.saturating_sub(1);
    }

    /// Returns the current reference count.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn ref_count(&self) -> usize {
        self.ref_count
    }

    /// Returns `true` if the reference count is greater than 0.
    #[must_use]
    pub(crate) fn is_referenced(&self) -> bool {
        self.ref_count > 0
    }
}

impl<T> Default for PinnedWithRefCount<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;

    use super::*;

    #[test]
    fn with_ref_count_basic() {
        let mut wrapper = PinnedWithRefCount::new(42);
        let wrapper_pin = Pin::new(&wrapper);
        assert_eq!(*wrapper_pin.get(), 42);
        assert_eq!(wrapper.ref_count(), 0);
        assert!(!wrapper.is_referenced());

        let mut wrapper_pin = Pin::new(&mut wrapper);
        wrapper_pin.as_mut().inc_ref();
        assert_eq!(wrapper_pin.ref_count(), 1);
        assert!(wrapper_pin.is_referenced());

        wrapper_pin.as_mut().dec_ref();
        assert_eq!(wrapper_pin.ref_count(), 0);
        assert!(!wrapper_pin.is_referenced());
    }

    #[test]
    fn ref_count_tracking_works() {
        let mut wrapper = PinnedWithRefCount::new(String::from("test"));

        // Simulate what happens in the pool
        let mut wrapper_pin = Pin::new(&mut wrapper);
        wrapper_pin.as_mut().inc_ref(); // For sender
        wrapper_pin.as_mut().inc_ref(); // For receiver
        assert_eq!(wrapper_pin.ref_count(), 2);

        wrapper_pin.as_mut().dec_ref(); // Sender dropped
        assert_eq!(wrapper_pin.ref_count(), 1);
        assert!(wrapper_pin.is_referenced());

        wrapper_pin.as_mut().dec_ref(); // Receiver dropped
        assert_eq!(wrapper_pin.ref_count(), 0);
        assert!(!wrapper_pin.is_referenced());
    }
}
