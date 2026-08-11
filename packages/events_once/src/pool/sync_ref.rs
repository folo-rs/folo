use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::Arc;

use crate::{Event, EventRef, destroy_event};
#[cfg(debug_assertions)]
use crate::{EventPoolCore, NEVER_POISONED};

pub(crate) struct PooledRef<T: 'static> {
    // Releasing an event does not need the pool, so this only exists in debug builds, where the
    // event must be removed from the pool's diagnostic registry before it is destroyed.
    #[cfg(debug_assertions)]
    core: Arc<EventPoolCore<T>>,

    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T: Send + 'static> PooledRef<T> {
    #[must_use]
    pub(crate) fn new(
        #[cfg(debug_assertions)] core: Arc<EventPoolCore<T>>,
        event: NonNull<UnsafeCell<Event<T>>>,
    ) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core,
            event,
        }
    }
}

impl<T: Send + 'static> Clone for PooledRef<T> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(debug_assertions)]
            core: Arc::clone(&self.core),
            event: self.event,
        }
    }
}

impl<T: Send + 'static> EventRef<T> for PooledRef<T> {
    // Deliberately not `#[inline]`: inlining this into `ReceiverCore::poll` grows that method
    // past the threshold at which it is itself inlined into the caller, which costs more than
    // the call saved here. Measured with the Callgrind lifecycle benchmarks.
    fn release_event(&self) {
        #[cfg(debug_assertions)]
        self.core
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

impl<T: Send + 'static> Deref for PooledRef<T> {
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event state machine guarantees that the event stays alive for as long as
        // any endpoint references it.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for PooledRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct(type_name::<Self>());

        #[cfg(debug_assertions)]
        f.field("core", &self.core);

        f.field("event", &self.event).finish()
    }
}

// SAFETY: This is only used with the thread-safe event, which may be referenced from any thread.
// The reference itself is not synchronized, so is not Sync, but it can move between threads.
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
