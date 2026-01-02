use std::any::type_name;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::NonNull;
use std::{fmt, mem};

use infinity_pool::RawPooled;

use crate::{Event, EventRef, RawEventPoolCore};

pub(crate) struct RawPooledRef<T: Send + 'static> {
    core: NonNull<UnsafeCell<RawEventPoolCore<T>>>,
    event: RawPooled<UnsafeCell<MaybeUninit<Event<T>>>>,
}

impl<T: Send + 'static> RawPooledRef<T> {
    #[must_use]
    pub(crate) fn new(
        core: NonNull<UnsafeCell<RawEventPoolCore<T>>>,
        event: RawPooled<UnsafeCell<MaybeUninit<Event<T>>>>,
    ) -> Self {
        Self { core, event }
    }
}

impl<T: Send + 'static> Clone for RawPooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            core: self.core,
            event: self.event,
        }
    }
}

impl<T: Send + 'static> EventRef<T> for RawPooledRef<T> {
    fn release_event(&self) {
        // SAFETY: Our owner promised the pool that the pool (the owner of the core) stays alive
        // longer than the event endpoints, so we know it remains valid. We only ever
        // create shared references to it, so no conflicting exclusive references can exist.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: See above.
        let core_maybe = unsafe { core_cell.get().as_ref() };

        // SAFETY: UnsafeCell pointer is never null.
        let core = unsafe { core_maybe.unwrap_unchecked() };

        let mut pool = core.pool.lock();

        // SAFETY: The event state machine guarantees that nothing references the event
        // once it signals the "you need to clean me up now". We hold the last reference.
        unsafe {
            pool.remove(self.event);
        }
    }
}

impl<T: Send + 'static> Deref for RawPooledRef<T> {
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event remains in the pool.
        let event_cell = unsafe { self.event.as_ref() };

        // SAFETY: We assert that the event has been initialized. This is always the case
        // by the time the RawPooledRef is created - the MaybeUninit wrapper is just there because
        // the items are delay-initialized after renting to avoid spurious memory copies.
        unsafe {
            mem::transmute::<&UnsafeCell<MaybeUninit<Event<T>>>, &UnsafeCell<Event<T>>>(event_cell)
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for RawPooledRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .field("event", &self.event)
            .finish()
    }
}

// SAFETY: The events are synchronization primitives and can be referenced from any thread.
// The reference itself is not synchronized, so is not Sync, but it can move between threads.
unsafe impl<T: Send + 'static> Send for RawPooledRef<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(RawPooledRef<u32>: Send);
    assert_not_impl_any!(RawPooledRef<u32>: Sync);
}
