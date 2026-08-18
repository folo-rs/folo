use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::rc::Rc;

#[cfg(debug_assertions)]
use crate::LocalPoolCore;
use crate::{LocalEvent, LocalRef, destroy_local_event};

pub(crate) struct PooledLocalRef<T: 'static> {
    // Releasing an event does not need the pool, so this only exists in debug builds, where the
    // event must be removed from the pool's diagnostic registry before it is destroyed.
    #[cfg(debug_assertions)]
    core: Rc<LocalPoolCore<T>>,

    event: NonNull<UnsafeCell<LocalEvent<T>>>,
}

impl<T: 'static> PooledLocalRef<T> {
    #[must_use]
    pub(crate) fn new(
        #[cfg(debug_assertions)] core: Rc<LocalPoolCore<T>>,
        event: NonNull<UnsafeCell<LocalEvent<T>>>,
    ) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core,
            event,
        }
    }
}

impl<T: 'static> Clone for PooledLocalRef<T> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core: Rc::clone(&self.core),
            event: self.event,
        }
    }
}

impl<T: 'static> LocalRef<T> for PooledLocalRef<T> {
    #[inline]
    fn release_event(&self) {
        #[cfg(debug_assertions)]
        self.core.state.borrow_mut().unregister(self.event);

        // SAFETY: The event state machine guarantees that nothing references the event once it
        // signals that it needs to be cleaned up now, so we hold the last reference and this is
        // the only release of this event.
        unsafe {
            destroy_local_event(self.event);
        }
    }
}

impl<T: 'static> Deref for PooledLocalRef<T> {
    type Target = UnsafeCell<LocalEvent<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event stays alive for as long as
        // any endpoint references it.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for PooledLocalRef<T> {
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

    assert_not_impl_any!(PooledLocalRef<u32>: Send, Sync);

    // Cloning a reference is on the hot path (there is one per endpoint), so in release builds
    // the reference is a bare pointer and cloning it copies rather than touching a refcount.
    #[cfg(debug_assertions)]
    assert_eq_size!(PooledLocalRef<u32>, (Rc<()>, NonNull<()>));
    #[cfg(not(debug_assertions))]
    assert_eq_size!(PooledLocalRef<u32>, NonNull<()>);
}
