use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::rc::Rc;

#[cfg(debug_assertions)]
use crate::LocalEventRegistry;
use crate::{LocalEvent, LocalRef, destroy_local_event};

/// References a local event rented from a managed pool or lake.
///
/// [`LocalEventPool`][crate::LocalEventPool] and [`LocalEventLake`][crate::LocalEventLake] use the
/// same detached plurality-slot release path. The pointer owns the slot, so release needs neither
/// allocation owner nor borrow (see `local_state.rs`). In debug builds, the shared registry owner
/// keeps diagnostics alive until the event is unregistered.
pub(crate) struct PooledLocalRef<T: 'static> {
    // Only debug builds need the registry for awaiter inspection.
    #[cfg(debug_assertions)]
    registry: Rc<LocalEventRegistry>,

    event: NonNull<UnsafeCell<LocalEvent<T>>>,
}

impl<T: 'static> PooledLocalRef<T> {
    /// Creates a reference to an event rented from a managed pool or lake.
    ///
    /// # Safety
    ///
    /// The event must be one that `initialize_local_event()` returned and that has not yet been
    /// released. In debug builds, it must be registered in `registry`. Nothing may create an
    /// exclusive reference to the event while any endpoint can access it.
    #[must_use]
    pub(crate) unsafe fn new(
        #[cfg(debug_assertions)] registry: Rc<LocalEventRegistry>,
        event: NonNull<UnsafeCell<LocalEvent<T>>>,
    ) -> Self {
        Self {
            #[cfg(debug_assertions)]
            registry,
            event,
        }
    }
}

impl<T: 'static> Clone for PooledLocalRef<T> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(debug_assertions)]
            registry: Rc::clone(&self.registry),
            event: self.event,
        }
    }
}

// SAFETY: The caller of `new()` guaranteed that the event occupies an initialized, detached
// plurality slot and has not been released. The slot stays at a fixed address, and plurality keeps
// its backing storage alive while it is detached. Everything that reaches the event - the
// endpoints holding this reference and its clones, plus the diagnostic registry in debug builds -
// creates only shared references, and the caller guaranteed that nothing creates an exclusive one.
// `release_event()` returns the slot through `destroy_local_event()`, the release operation of
// this storage strategy, and the reference does not touch the event afterwards.
unsafe impl<T: 'static> LocalRef<T> for PooledLocalRef<T> {
    #[inline]
    unsafe fn release_event(&self) {
        #[cfg(debug_assertions)]
        {
            // SAFETY: The `new()` contract requires this registered event to remain in its stable
            // plurality slot with shared-only access. The caller owns its sole cleanup right, so
            // those conditions hold until this unregistration returns.
            unsafe {
                self.registry.unregister(self.event);
            }
        }

        // SAFETY: The pointer came from `initialize_local_event()`, as the `new()` contract
        // requires. The caller was granted sole cleanup ownership of the event by the state
        // machine, so this is the only release of this event and nothing accesses it afterwards.
        unsafe {
            destroy_local_event(self.event);
        }
    }
}

impl<T: 'static> Deref for PooledLocalRef<T> {
    type Target = UnsafeCell<LocalEvent<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Validity: the `new()` contract gives us a rented, initialized event that keeps
        // its address in plurality storage that outlives it, and cleanup ownership is granted to a
        // single endpoint, so the event is not yet released while any endpoint can call this.
        // Aliasing: the event is reached only through shared references, whether from an
        // endpoint or from the debug-only registry, which is what its interior mutability
        // requires.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for PooledLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct(type_name::<Self>());

        #[cfg(debug_assertions)]
        f.field("registry", &self.registry);

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
