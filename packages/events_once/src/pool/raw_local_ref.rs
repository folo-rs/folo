use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

#[cfg(debug_assertions)]
use crate::RawLocalEventPoolCore;
use crate::{LocalEvent, LocalRef, destroy_local_event};

/// References an event rented from a [`RawLocalEventPool`][crate::RawLocalEventPool].
///
/// The slot of a rented event is owned by the pointer to it, not by the pool, so releasing the
/// event needs neither the pool nor a borrow of it (see `local_state.rs`). The pointer to the
/// pool core exists only in debug builds, where the event must be removed from the pool's
/// diagnostic registry before it is destroyed; unlike the managed pool, the raw pool relies on
/// the caller's promise that it outlives the endpoints to keep that pointer valid.
pub(crate) struct RawLocalPooledRef<T: 'static> {
    // Only debug builds need the core, to reach the diagnostic registry.
    #[cfg(debug_assertions)]
    core: NonNull<UnsafeCell<RawLocalEventPoolCore<T>>>,

    event: NonNull<UnsafeCell<LocalEvent<T>>>,
}

impl<T: 'static> RawLocalPooledRef<T> {
    /// Creates a reference to an event rented from a pool.
    ///
    /// # Safety
    ///
    /// The event must be one that `LocalPoolState::rent()` returned and that has not yet been
    /// released, in debug builds from the state inside `core`. Nothing may create an exclusive
    /// reference to the event while any endpoint created from this reference can access it. In
    /// debug builds, the pool that owns `core` must outlive every such endpoint.
    #[must_use]
    pub(crate) unsafe fn new(
        #[cfg(debug_assertions)] core: NonNull<UnsafeCell<RawLocalEventPoolCore<T>>>,
        event: NonNull<UnsafeCell<LocalEvent<T>>>,
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
        // SAFETY: The `new()` contract requires the pool that owns the core to outlive every
        // endpoint created from this reference, so the core is still live, initialized and
        // aligned while any endpoint can reach it.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: The core is reached only through this accessor and the pool's own equivalent,
        // both of which produce shared references, so no exclusive reference can alias this one.
        // Mutation of the core happens exclusively behind its cell.
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

// SAFETY: The caller of `new()` guaranteed that the event was rented from the pool and has not
// been released, so it is initialized and stays at a fixed address in the pool's storage, which
// outlives every event rented from it. Everything that reaches the event - the endpoints holding
// this reference and its clones, plus the pool's diagnostic registry in debug builds - creates
// only shared references, and the caller guaranteed that nothing creates an exclusive one.
// `release_event()` returns the slot through `destroy_local_event()`, the release operation of
// this storage strategy, and the reference does not touch the event afterwards.
unsafe impl<T: 'static> LocalRef<T> for RawLocalPooledRef<T> {
    unsafe fn release_event(&self) {
        #[cfg(debug_assertions)]
        self.core().state.borrow_mut().unregister(self.event);

        // SAFETY: The pointer came from the pool state's `rent()`, as the `new()` contract
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
        // its address in pool storage that outlives it, and cleanup ownership is granted to a
        // single endpoint, so the event is not yet released while any endpoint can call this.
        // Aliasing: the event is reached only through shared references, whether from an
        // endpoint or from the pool's debug-only registry, which is what its interior mutability
        // requires.
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
