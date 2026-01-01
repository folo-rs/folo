use std::alloc::{Layout, alloc, dealloc};
use std::any::type_name;
use std::fmt;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::NonNull;

use crate::LocalEvent;

/// Enables a sender or receiver to reference the event that connects them.
pub(crate) trait LocalRef<T: 'static>: Deref<Target = LocalEvent<T>> + fmt::Debug {
    /// Releases the event, asserting that the last endpoint has been dropped
    /// and nothing will access the event after this call.
    fn release_event(&self);
}

/// References an event stored anywhere, via raw pointer.
pub(crate) struct PtrLocalRef<T: 'static> {
    event: NonNull<LocalEvent<T>>,
}

impl<T: 'static> PtrLocalRef<T> {
    #[must_use]
    pub(crate) fn new(event: NonNull<LocalEvent<T>>) -> Self {
        Self { event }
    }
}

impl<T: 'static> LocalRef<T> for PtrLocalRef<T> {
    #[cfg_attr(test, mutants::skip)] // Does nothing, so nothing to test.
    fn release_event(&self) {}
}

impl<T: 'static> Deref for PtrLocalRef<T> {
    type Target = LocalEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}

impl<T: 'static> fmt::Debug for PtrLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

/// References an event stored on the heap.
pub(crate) struct BoxedLocalRef<T: 'static> {
    event: NonNull<LocalEvent<T>>,
}

impl<T: 'static> BoxedLocalRef<T> {
    #[must_use]
    pub(crate) fn new_pair() -> (Self, Self) {
        // SAFETY: The layout is correct for the type we are using - all is well.
        let event = NonNull::new(unsafe { alloc(Self::layout()) })
            .expect("memory allocation failed - fatal error")
            .cast();

        // SAFETY: MaybeUninit is a transparent wrapper, so the layout matches.
        // This is the only reference, so we have exclusive access rights.
        let event_as_maybe_uninit = unsafe { event.cast::<MaybeUninit<LocalEvent<T>>>().as_mut() };

        LocalEvent::new_in_inner(event_as_maybe_uninit);

        (Self { event }, Self { event })
    }

    const fn layout() -> Layout {
        Layout::new::<LocalEvent<T>>()
    }
}

impl<T: 'static> LocalRef<T> for BoxedLocalRef<T> {
    #[cfg_attr(test, mutants::skip)] // Impractical to test that memory is released. Miri will tell us about leaks.
    fn release_event(&self) {
        // The caller tells us that they are the last endpoint, so nothing else can possibly
        // be accessing the event any more. We can safely release the memory.

        // SAFETY: Still the same type - all is well. We rely on the event state machine
        // to ensure that there is no double-release happening.
        unsafe {
            dealloc(self.event.as_ptr().cast(), Self::layout());
        }
    }
}

impl<T: 'static> Deref for BoxedLocalRef<T> {
    type Target = LocalEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        unsafe { self.event.as_ref() }
    }
}

impl<T: 'static> fmt::Debug for BoxedLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(BoxedLocalRef<i32>: Send, Sync);
    assert_not_impl_any!(PtrLocalRef<i32>: Send, Sync);
}
