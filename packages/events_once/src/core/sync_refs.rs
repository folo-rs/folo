use std::alloc::{Layout, alloc, dealloc};
use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::NonNull;

use crate::Event;

/// Enables a sender or receiver to reference the event that connects them.
pub(crate) trait EventRef<T>: Deref<Target = UnsafeCell<Event<T>>> + fmt::Debug
where
    T: Send,
{
    /// Releases the event, asserting that the last endpoint has been dropped
    /// and nothing will access the event after this call.
    fn release_event(&self);
}

/// References an event stored anywhere, via raw pointer.
pub(crate) struct PtrRef<T>
where
    T: Send,
{
    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T: Send> PtrRef<T> {
    #[must_use]
    pub(crate) fn new(event: NonNull<UnsafeCell<Event<T>>>) -> Self {
        Self { event }
    }
}

impl<T> EventRef<T> for PtrRef<T>
where
    T: Send,
{
    #[cfg_attr(test, mutants::skip)] // Does nothing, so nothing to test.
    fn release_event(&self) {}
}

impl<T> Deref for PtrRef<T>
where
    T: Send,
{
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}

// SAFETY: This is only used with the thread-safe event (the event is Sync).
unsafe impl<T> Send for PtrRef<T> where T: Send {}

impl<T: Send> fmt::Debug for PtrRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

/// References an event stored on the heap.
pub(crate) struct BoxedRef<T>
where
    T: Send,
{
    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T> BoxedRef<T>
where
    T: Send,
{
    #[must_use]
    pub(crate) fn new_pair() -> (Self, Self) {
        // SAFETY: The layout is correct for the type we are using - all is well.
        let event = NonNull::new(unsafe { alloc(Self::layout()) })
            .expect("memory allocation failed - fatal error")
            .cast();

        // SAFETY: MaybeUninit is a transparent wrapper, so the layout matches.
        // This is the only reference, so we have exclusive access rights.
        let event_as_maybe_uninit =
            unsafe { event.cast::<UnsafeCell<MaybeUninit<Event<T>>>>().as_mut() };

        Event::new_in_inner(event_as_maybe_uninit);

        (Self { event }, Self { event })
    }

    const fn layout() -> Layout {
        Layout::new::<Event<T>>()
    }
}

impl<T> EventRef<T> for BoxedRef<T>
where
    T: Send,
{
    #[cfg_attr(test, mutants::skip)] // Impractical to test deallocation - Miri will complain if we leak.
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

impl<T> Deref for BoxedRef<T>
where
    T: Send,
{
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        unsafe { self.event.as_ref() }
    }
}

// SAFETY: This is only used with the thread-safe event (the event is Sync).
unsafe impl<T> Send for BoxedRef<T> where T: Send {}

impl<T: Send> fmt::Debug for BoxedRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(BoxedRef<u32>: Send);
    assert_not_impl_any!(BoxedRef<u32>: Sync);

    assert_impl_all!(PtrRef<u32>: Send);
    assert_not_impl_any!(PtrRef<u32>: Sync);
}
