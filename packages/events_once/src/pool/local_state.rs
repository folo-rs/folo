use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::fmt;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::Arc;

use plurality::Pool;

#[cfg(debug_assertions)]
use crate::EventRegistry;
use crate::{EVENT_COUNT_FITS_IN_USIZE, LocalEvent};

/// The storage that backs a pool of single-threaded events.
///
/// [`LocalEventPool`][crate::LocalEventPool] and
/// [`RawLocalEventPool`][crate::RawLocalEventPool] differ only in how they keep this alive, so
/// they share it. Both reach it through a cell, which also covers the diagnostic registry -
/// renting an event therefore registers it without taking a second borrow.
///
/// Releasing an event does not go through here at all: a slot is owned by the pointer that
/// [`rent()`][Self::rent] returns and is given back by [`destroy_local_event()`], which needs
/// neither the pool nor a borrow of it.
pub(crate) struct LocalPoolState<T: 'static> {
    events: Pool<LocalEvent<T>>,

    #[cfg(debug_assertions)]
    registry: EventRegistry<LocalEvent<T>>,
}

impl<T: 'static> LocalPoolState<T> {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            events: Pool::new(),
            #[cfg(debug_assertions)]
            registry: EventRegistry::new(),
        }
    }

    /// Allocates an event and initializes it, handing ownership of its slot to the caller.
    ///
    /// The caller becomes responsible for passing the pointer to [`destroy_local_event()`]
    /// exactly once, having first deregistered it via [`unregister()`][Self::unregister].
    #[must_use]
    #[cfg_attr(
        not(debug_assertions),
        expect(
            clippy::needless_pass_by_ref_mut,
            reason = "the diagnostic registry that requires exclusive access is debug-only; \
                      the signature stays uniform so callers need not differ"
        )
    )]
    pub(crate) fn rent(&mut self) -> NonNull<LocalEvent<T>> {
        let mut storage = self.events.alloc_uninit_box();

        // SAFETY: The slot is still uninitialized, so it carries no pinning invariant that
        // unwrapping the `Pin` could violate. We only use the reference to initialize the event
        // in place, which moves nothing.
        let place = unsafe { storage.as_pin_mut().get_unchecked_mut() };

        LocalEvent::new_in_inner(place);

        // SAFETY: `new_in_inner()` initialized the event. The `MaybeUninit` fields inside the
        // event are uninitialized by design and are not part of what this asserts.
        let storage = unsafe { storage.assume_init() };

        let event = plurality::Box::into_raw(storage);

        #[cfg(debug_assertions)]
        self.registry.register(event);

        event
    }

    /// Stops enumerating an event that is about to be destroyed.
    #[cfg(debug_assertions)]
    pub(crate) fn unregister(&mut self, event: NonNull<LocalEvent<T>>) {
        self.registry.unregister(event);
    }

    /// Whether no events have been rented and not yet released.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// The number of events that have been rented and not yet released.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        usize::try_from(self.events.len()).expect(EVENT_COUNT_FITS_IN_USIZE)
    }

    /// Snapshots the backtrace of the most recent awaiter of each awaited event in the pool.
    ///
    /// Each snapshot is a shared owner of the backtrace, so it stays valid even if its event is
    /// released before the caller looks at it.
    #[cfg(debug_assertions)]
    #[must_use]
    pub(crate) fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        let mut backtraces = Vec::with_capacity(self.registry.len());

        for event in self.registry.iter() {
            // SAFETY: The registry only names events that have been rented and not yet released,
            // so the slot is still occupied by a live event. We only ever create shared
            // references to an event, so no exclusive reference can alias this one.
            let event = unsafe { event.as_ref() };

            if let Some(backtrace) = event.awaiter_backtrace() {
                backtraces.push(backtrace);
            }
        }

        backtraces
    }
}

/// Drops an event that was rented from a pool and returns its slot to the pool.
///
/// The slot is owned by the pointer, not by the pool, so this needs neither a reference to the
/// pool nor a borrow of it. The pool's storage stays alive while any event rented from it is
/// alive, so this remains valid even if the pool itself is already gone.
///
/// # Safety
///
/// The pointer must come from [`LocalPoolState::rent()`] and must not have been passed here
/// before. Nothing may reference the event afterwards.
pub(crate) unsafe fn destroy_local_event<T: 'static>(event: NonNull<LocalEvent<T>>) {
    // SAFETY: Forwarding the guarantees of the caller, who promises that this is the single
    // `from_raw()` call matching the `into_raw()` that renting performed for this pointer.
    drop(unsafe { plurality::Box::<LocalEvent<T>>::from_raw(event) });
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for LocalPoolState<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct(type_name::<Self>());

        #[cfg(debug_assertions)]
        f.field("registry", &self.registry);

        f.field("events", &self.events).finish()
    }
}
