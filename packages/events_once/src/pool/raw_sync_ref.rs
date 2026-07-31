use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

use crate::{Event, EventRef, destroy_event};
#[cfg(debug_assertions)]
use crate::{NEVER_POISONED, RawEventPoolCore};

pub(crate) struct RawPooledRef<T: 'static> {
    // Releasing an event does not need the pool, so this only exists in debug builds, where the
    // event must be removed from the pool's diagnostic registry before it is destroyed.
    #[cfg(debug_assertions)]
    core: NonNull<UnsafeCell<RawEventPoolCore<T>>>,

    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T: Send + 'static> RawPooledRef<T> {
    #[must_use]
    pub(crate) fn new(
        #[cfg(debug_assertions)] core: NonNull<UnsafeCell<RawEventPoolCore<T>>>,
        event: NonNull<UnsafeCell<Event<T>>>,
    ) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core,
            event,
        }
    }

    /// Returns a shared reference to the pool's core.
    #[cfg(debug_assertions)]
    fn core(&self) -> &RawEventPoolCore<T> {
        // SAFETY: Our owner promised the pool that the pool (the owner of the core) stays alive
        // longer than the event endpoints, so we know it remains valid.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: We only ever create shared references to the core, so no conflicting exclusive
        // references can exist.
        unsafe { &*core_cell.get() }
    }
}

impl<T: Send + 'static> Clone for RawPooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core: self.core,
            event: self.event,
        }
    }
}

impl<T: Send + 'static> EventRef<T> for RawPooledRef<T> {
    fn release_event(&self) {
        #[cfg(debug_assertions)]
        self.core()
            .state
            .lock()
            .expect(NEVER_POISONED)
            .unregister(self.event);

        // SAFETY: The event state machine guarantees that nothing references the event once it
        // signals that it needs to be cleaned up now, so we hold the last reference and this is
        // the only release of this event.
        unsafe {
            destroy_event(self.event);
        }
    }
}

impl<T: Send + 'static> Deref for RawPooledRef<T> {
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event stays alive for as long as
        // any endpoint references it.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for RawPooledRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct(type_name::<Self>());

        #[cfg(debug_assertions)]
        f.field("core", &self.core);

        f.field("event", &self.event).finish()
    }
}

// SAFETY: The events are synchronization primitives and can be referenced from any thread.
// The reference itself is not synchronized, so is not Sync, but it can move between threads.
// The `'static` bound is already on the struct, so it is not repeated here. Repeating it
// would trigger a rustc bug (rust-lang/rust#110338) in async generator Send inference
unsafe impl<T: Send> Send for RawPooledRef<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_eq_size, assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(RawPooledRef<u32>: Send);
    assert_not_impl_any!(RawPooledRef<u32>: Sync);

    // Trait object payloads must preserve Send (regression test for #142).
    assert_impl_all!(RawPooledRef<Box<dyn Send>>: Send);

    // Cloning a reference is on the hot path (there is one per endpoint), so in release builds
    // the reference is a bare pointer to the event and nothing else.
    #[cfg(debug_assertions)]
    assert_eq_size!(RawPooledRef<u32>, (NonNull<()>, NonNull<()>));
    #[cfg(not(debug_assertions))]
    assert_eq_size!(RawPooledRef<u32>, NonNull<()>);
}
