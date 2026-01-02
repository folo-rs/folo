use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

use infinity_pool::RawPooled;

use crate::{LocalEvent, LocalRef, RawLocalEventPoolCore};

pub(crate) struct RawLocalPooledRef<T: 'static> {
    core: NonNull<UnsafeCell<RawLocalEventPoolCore<T>>>,
    event: RawPooled<LocalEvent<T>>,
}

impl<T: 'static> RawLocalPooledRef<T> {
    #[must_use]
    pub(crate) fn new(
        core: NonNull<UnsafeCell<RawLocalEventPoolCore<T>>>,
        event: RawPooled<LocalEvent<T>>,
    ) -> Self {
        Self { core, event }
    }
}

impl<T: 'static> Clone for RawLocalPooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            core: self.core,
            event: self.event,
        }
    }
}

impl<T: 'static> LocalRef<T> for RawLocalPooledRef<T> {
    fn release_event(&self) {
        // SAFETY: Our owner promised the pool that the pool (the owner of the core) stays alive
        // longer than the event endpoints, so we know it remains valid. We only ever
        // create shared references to it, so no conflicting exclusive references can exist.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: See above.
        let core_maybe = unsafe { core_cell.get().as_ref() };

        // SAFETY: UnsafeCell pointer is never null.
        let core = unsafe { core_maybe.unwrap_unchecked() };

        let mut pool = core.pool.borrow_mut();

        // SAFETY: The event state machine guarantees that nothing references the event
        // once it signals the "you need to clean me up now". We hold the last reference.
        unsafe {
            pool.remove(self.event);
        }
    }
}

impl<T: 'static> Deref for RawLocalPooledRef<T> {
    type Target = LocalEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event remains in the pool.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for RawLocalPooledRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .field("event", &self.event)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(RawLocalPooledRef<u32>: Send, Sync);
}
