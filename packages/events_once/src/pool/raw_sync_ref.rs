use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

use crate::{Event, EventRef, destroy_event};
#[cfg(debug_assertions)]
use crate::{NEVER_POISONED, RawEventPoolCore};

/// References an event rented from a [`RawEventPool`][crate::RawEventPool].
///
/// The slot of a rented event is owned by the pointer to it, not by the pool, so releasing the
/// event needs neither the pool nor its lock (see `state.rs`). The pointer to the pool core
/// exists only in debug builds, where the event must be removed from the pool's diagnostic
/// registry before it is destroyed; unlike the managed pool, the raw pool relies on the caller's
/// promise that it outlives the endpoints to keep that pointer valid.
pub(crate) struct RawPooledRef<T: 'static> {
    // Only debug builds need the core, to reach the diagnostic registry.
    #[cfg(debug_assertions)]
    core: NonNull<UnsafeCell<RawEventPoolCore<T>>>,

    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T: Send + 'static> RawPooledRef<T> {
    /// Creates a reference to an event rented from a pool.
    ///
    /// # Safety
    ///
    /// The event must be one that `PoolState::rent()` returned and that has not yet been
    /// released, in debug builds from the state inside `core`. Nothing may create an exclusive
    /// reference to the event while any endpoint created from this reference can access it. In
    /// debug builds, the pool that owns `core` must outlive every such endpoint.
    #[must_use]
    pub(crate) unsafe fn new(
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
        // SAFETY: The `new()` contract requires the pool that owns the core to outlive every
        // endpoint created from this reference, so the core is still live, initialized and
        // aligned while any endpoint can reach it.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: The core is reached only through this accessor and the pool's own equivalent,
        // both of which produce shared references, so no exclusive reference can alias this one.
        // Mutation of the core happens exclusively behind its mutex.
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

// SAFETY: The caller of `new()` guaranteed that the event was rented from the pool and has not
// been released, so it is initialized and stays at a fixed address in the pool's storage, which
// outlives every event rented from it. Everything that reaches the event - the endpoints holding
// this reference and its clones, plus the pool's diagnostic registry in debug builds - creates
// only shared references, and the caller guaranteed that nothing creates an exclusive one.
// `release_event()` returns the slot through `destroy_event()`, the release operation of this
// storage strategy, and the reference does not touch the event afterwards.
unsafe impl<T: Send + 'static> EventRef<T> for RawPooledRef<T> {
    unsafe fn release_event(&self) {
        #[cfg(debug_assertions)]
        self.core()
            .state
            .lock()
            .expect(NEVER_POISONED)
            .unregister(self.event);

        // SAFETY: The pointer came from the pool state's `rent()`, as the `new()` contract
        // requires. The caller was granted sole cleanup ownership of the event by the state
        // machine, so this is the only release of this event and nothing accesses it afterwards.
        unsafe {
            destroy_event(self.event);
        }
    }
}

impl<T: Send + 'static> Deref for RawPooledRef<T> {
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Validity: the `new()` contract gives us a rented, initialized event that keeps
        // its address in pool storage that outlives it, and cleanup ownership is granted to a
        // single endpoint, so the event is not yet released while any endpoint can call this.
        // Aliasing: the event is reached only through shared references, whether from an
        // endpoint or from the pool's debug-only registry; the event synchronizes access to its
        // own interior fields.
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

// SAFETY: Both stored pointers remain usable after the reference moves to another thread. The
// event is a synchronization primitive that synchronizes access to itself, and moving the
// reference can carry a value of `T` to the destination thread, which is why `T: Send` is
// required. Releasing the event from the destination thread hands the slot back through
// `plurality::Box`, whose slot bookkeeping is atomic, so it needs no further synchronization.
// The debug-only core pointer stays valid because the caller of `new()` promised that the pool
// owning the core outlives every endpoint, and the registry behind it is only reached while
// holding `RawEventPoolCore::state`, so unregistering an event from another thread is
// synchronized.
// The reference is not synchronized as a whole, so it is not `Sync`: only moving it between
// threads is permitted, not sharing it between them.
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
