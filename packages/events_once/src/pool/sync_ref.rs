use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::Arc;

#[cfg(debug_assertions)]
use crate::EventRegistry;
use crate::{Event, EventRef, destroy_event};

/// References a thread-safe event rented from a managed pool or lake.
///
/// [`EventPool`][crate::EventPool] and [`EventLake`][crate::EventLake] use the same detached
/// plurality-slot release path. The pointer owns the slot, so release needs neither allocation
/// owner nor lock (see `state.rs`). In debug builds, the shared registry owner keeps diagnostics
/// alive until the event is unregistered.
pub(crate) struct PooledRef<T: 'static> {
    // Only debug builds need the registry for awaiter inspection.
    #[cfg(debug_assertions)]
    registry: Arc<EventRegistry>,

    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T: Send + 'static> PooledRef<T> {
    /// Creates a reference to an event rented from a managed pool or lake.
    ///
    /// # Safety
    ///
    /// The event must be one that `initialize_event()` returned and that has not yet been released.
    /// In debug builds, it must be registered in `registry`. Nothing may create an exclusive
    /// reference to the event while any endpoint can access it.
    #[must_use]
    pub(crate) unsafe fn new(
        #[cfg(debug_assertions)] registry: Arc<EventRegistry>,
        event: NonNull<UnsafeCell<Event<T>>>,
    ) -> Self {
        Self {
            #[cfg(debug_assertions)]
            registry,
            event,
        }
    }
}

impl<T: Send + 'static> Clone for PooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(debug_assertions)]
            registry: Arc::clone(&self.registry),
            event: self.event,
        }
    }
}

// SAFETY: The caller of `new()` guaranteed that the event occupies an initialized, detached
// plurality slot and has not been released. The slot stays at a fixed address, and plurality keeps
// its backing storage alive while it is detached. Everything that reaches the event - the
// endpoints holding this reference and its clones, plus the diagnostic registry in debug builds -
// creates only shared references, and the caller guaranteed that nothing creates an exclusive one.
// `release_event()` returns the slot through `destroy_event()`, the release operation of this
// storage strategy, and the reference does not touch the event afterwards.
unsafe impl<T: Send + 'static> EventRef<T> for PooledRef<T> {
    // Deliberately not `#[inline]`: inlining this into `ReceiverCore::poll` grows that method
    // past the threshold at which it is itself inlined into the caller, which costs more than
    // the call saved here. Measured with the Callgrind lifecycle benchmarks.
    unsafe fn release_event(&self) {
        #[cfg(debug_assertions)]
        {
            // SAFETY: The `new()` contract requires this live event to be registered here, and
            // the caller owns its sole cleanup right, so it remains live through unregistration.
            unsafe {
                self.registry.unregister(self.event);
            }
        }

        // SAFETY: The pointer came from `initialize_event()`, as the `new()` contract requires.
        // The caller was granted sole cleanup ownership of the event by the state machine, so
        // this is the only release of this event and nothing accesses it afterwards.
        unsafe {
            destroy_event(self.event);
        }
    }
}

impl<T: Send + 'static> Deref for PooledRef<T> {
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Validity: the `new()` contract gives us a rented, initialized event that keeps
        // its address in plurality storage that outlives it, and cleanup ownership is granted to a
        // single endpoint, so the event is not yet released while any endpoint can call this.
        // Aliasing: the event is reached only through shared references, whether from an
        // endpoint or from the debug-only registry; the event synchronizes access to its
        // own interior fields.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for PooledRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct(type_name::<Self>());

        #[cfg(debug_assertions)]
        f.field("registry", &self.registry);

        f.field("event", &self.event).finish()
    }
}

// SAFETY: Both stored fields remain usable after the reference moves to another thread. The
// event is a synchronization primitive that synchronizes access to itself, and moving the
// reference can carry a value of `T` to the destination thread, which is why `T: Send` is
// required. Releasing the event from the destination thread hands the slot back through
// `plurality::Box`, whose slot bookkeeping is atomic, so it needs no further synchronization.
// The debug-only `Arc` keeps the synchronized registry alive independently of the pool handle.
// The reference is not synchronized as a whole, so it is not `Sync`: only moving it between
// threads is permitted, not sharing it between them.
// The `'static` bound is already on the struct, so it is not repeated here. Repeating it
// would trigger a rustc bug (rust-lang/rust#110338) in async generator Send inference
// with trait object type params.
unsafe impl<T: Send> Send for PooledRef<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_eq_size, assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(PooledRef<u32>: Send);
    assert_not_impl_any!(PooledRef<u32>: Sync);

    // Trait object payloads must preserve Send (regression test for #142).
    assert_impl_all!(PooledRef<Box<dyn Send>>: Send);

    // Cloning a reference is on the hot path (there is one per endpoint), so in release builds
    // the reference is a bare pointer and cloning it copies rather than touching a refcount.
    #[cfg(debug_assertions)]
    assert_eq_size!(PooledRef<u32>, (Arc<()>, NonNull<()>));
    #[cfg(not(debug_assertions))]
    assert_eq_size!(PooledRef<u32>, NonNull<()>);
}
