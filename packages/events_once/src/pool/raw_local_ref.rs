use std::any::type_name;
#[cfg(debug_assertions)]
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

#[cfg(debug_assertions)]
use crate::RawLocalEventPoolCore;
use crate::{LocalEvent, LocalRef, destroy_local_event};

pub(crate) struct RawLocalPooledRef<T: 'static> {
    // Releasing an event does not need the pool, so this only exists in debug builds, where the
    // event must be removed from the pool's diagnostic registry before it is destroyed.
    #[cfg(debug_assertions)]
    core: NonNull<UnsafeCell<RawLocalEventPoolCore<T>>>,

    event: NonNull<LocalEvent<T>>,
}

impl<T: 'static> RawLocalPooledRef<T> {
    #[must_use]
    pub(crate) fn new(
        #[cfg(debug_assertions)] core: NonNull<UnsafeCell<RawLocalEventPoolCore<T>>>,
        event: NonNull<LocalEvent<T>>,
    ) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core,
            event,
        }
    }

    /// Returns a shared reference to the pool's core.
    #[cfg(debug_assertions)]
    fn core(&self) -> &RawLocalEventPoolCore<T> {
        // SAFETY: Our owner promised the pool that the pool (the owner of the core) stays alive
        // longer than the event endpoints, so we know it remains valid.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: We only ever create shared references to the core, so no conflicting exclusive
        // references can exist.
        unsafe { &*core_cell.get() }
    }
}

impl<T: 'static> Clone for RawLocalPooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core: self.core,
            event: self.event,
        }
    }
}

impl<T: 'static> LocalRef<T> for RawLocalPooledRef<T> {
    fn release_event(&self) {
        #[cfg(debug_assertions)]
        self.core().state.borrow_mut().unregister(self.event);

        // SAFETY: The event state machine guarantees that nothing references the event once it
        // signals that it needs to be cleaned up now, so we hold the last reference and this is
        // the only release of this event.
        unsafe {
            destroy_local_event(self.event);
        }
    }
}

impl<T: 'static> Deref for RawLocalPooledRef<T> {
    type Target = LocalEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event stays alive for as long as
        // any endpoint references it.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for RawLocalPooledRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct(type_name::<Self>());

        #[cfg(debug_assertions)]
        f.field("core", &self.core);

        f.field("event", &self.event).finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_eq_size, assert_not_impl_any};

    use super::*;

    assert_not_impl_any!(RawLocalPooledRef<u32>: Send, Sync);

    // Cloning a reference is on the hot path (there is one per endpoint), so in release builds
    // the reference is a bare pointer to the event and nothing else.
    #[cfg(debug_assertions)]
    assert_eq_size!(RawLocalPooledRef<u32>, (NonNull<()>, NonNull<()>));
    #[cfg(not(debug_assertions))]
    assert_eq_size!(RawLocalPooledRef<u32>, NonNull<()>);
}
