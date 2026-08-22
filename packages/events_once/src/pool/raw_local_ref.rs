use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

#[cfg(debug_assertions)]
use crate::LocalEventRegistry;
use crate::{LocalEvent, LocalRef, destroy_local_event};

/// References a local event rented from a raw pool or lake.
///
/// [`RawLocalEventPool`][crate::RawLocalEventPool] and
/// [`RawLocalEventLake`][crate::RawLocalEventLake] use the same detached plurality-slot release
/// path. The pointer owns the slot, so release needs neither allocation owner nor borrow (see
/// `local_state.rs`). In debug builds, the raw owner-outlives-endpoints contract keeps the
/// registry pointer valid until the event is unregistered.
pub(crate) struct RawLocalPooledRef<T: 'static> {
    // Only debug builds need the registry for awaiter inspection.
    #[cfg(debug_assertions)]
    registry: NonNull<LocalEventRegistry>,

    event: NonNull<UnsafeCell<LocalEvent<T>>>,
}

impl<T: 'static> RawLocalPooledRef<T> {
    /// Creates a reference to an event rented from a raw pool or lake.
    ///
    /// # Safety
    ///
    /// The event must be one that `initialize_local_event()` returned and that has not yet been
    /// released. In debug builds, it must be registered in `registry`, whose owner must outlive
    /// every endpoint. Nothing may create an exclusive reference to the event while any endpoint
    /// can access it.
    #[must_use]
    pub(crate) unsafe fn new(
        #[cfg(debug_assertions)] registry: NonNull<LocalEventRegistry>,
        event: NonNull<UnsafeCell<LocalEvent<T>>>,
    ) -> Self {
        Self {
            #[cfg(debug_assertions)]
            registry,
            event,
        }
    }

    /// Returns a shared reference to the diagnostic registry.
    #[cfg(debug_assertions)]
    fn registry(&self) -> &LocalEventRegistry {
        // SAFETY: Validity follows from `new()` requiring the registry owner to outlive every
        // endpoint. Only shared references to the registry exist, and its `RefCell` mediates
        // mutation on the one thread where the raw local owner may be used.
        unsafe { self.registry.as_ref() }
    }
}

impl<T: 'static> Clone for RawLocalPooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(debug_assertions)]
            registry: self.registry,
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
unsafe impl<T: 'static> LocalRef<T> for RawLocalPooledRef<T> {
    unsafe fn release_event(&self) {
        #[cfg(debug_assertions)]
        {
            // SAFETY: The `new()` contract requires this live event to be registered here, and
            // the caller owns its sole cleanup right, so it remains live through unregistration.
            unsafe {
                self.registry().unregister(self.event);
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

impl<T: 'static> Deref for RawLocalPooledRef<T> {
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
impl<T: 'static> fmt::Debug for RawLocalPooledRef<T> {
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

    assert_not_impl_any!(RawLocalPooledRef<u32>: Send, Sync);

    // Cloning a reference is on the hot path (there is one per endpoint), so in release builds
    // the reference is a bare pointer to the event and nothing else.
    #[cfg(debug_assertions)]
    assert_eq_size!(RawLocalPooledRef<u32>, (NonNull<()>, NonNull<()>));
    #[cfg(not(debug_assertions))]
    assert_eq_size!(RawLocalPooledRef<u32>, NonNull<()>);
}
