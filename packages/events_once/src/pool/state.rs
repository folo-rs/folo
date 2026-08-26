use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::ptr::{self, NonNull};

use plurality::{Box as PoolBox, Pool};

use crate::{EVENT_COUNT_FITS_IN_USIZE, Event};

/// The storage that backs a pool of thread-safe events.
///
/// [`EventPool`][crate::EventPool] and [`RawEventPool`][crate::RawEventPool] differ only in how
/// they keep this alive, so they share it. `plurality::Pool` allocates through shared references
/// but is not `Sync`, so both owners use a mutex to serialize access from different threads.
///
/// Releasing an event does not go through here at all: a slot is owned by the pointer that
/// [`rent()`][Self::rent] returns and is given back by [`destroy_event()`], which needs no pool
/// and no lock.
pub(crate) struct PoolState<T: 'static> {
    events: Pool<UnsafeCell<Event<T>>>,
}

impl<T: Send + 'static> PoolState<T> {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            events: Pool::new(),
        }
    }

    /// Allocates an event and initializes it, handing ownership of its slot to the caller.
    ///
    /// The caller becomes responsible for passing the pointer to [`destroy_event()`] exactly once.
    #[must_use]
    pub(crate) fn rent(&self) -> NonNull<UnsafeCell<Event<T>>> {
        initialize_event(self.events.alloc_uninit_box())
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
}

/// Initializes an event in freshly allocated plurality storage.
///
/// Both typed pools and heterogeneous lakes use the same placement path so that endpoint
/// references can release either source through [`destroy_event()`].
#[must_use]
pub(crate) fn initialize_event<T: Send + 'static>(
    mut storage: PoolBox<MaybeUninit<UnsafeCell<Event<T>>>>,
) -> NonNull<UnsafeCell<Event<T>>> {
    // SAFETY: The slot is still uninitialized, so it carries no pinning invariant that unwrapping
    // the `Pin` could violate. We only use the reference to initialize the event in place.
    let place = unsafe { storage.as_pin_mut().get_unchecked_mut() };

    let place = ptr::from_mut(place).cast::<UnsafeCell<MaybeUninit<Event<T>>>>();

    // SAFETY: `MaybeUninit` and `UnsafeCell` are transparent wrappers, so both nestings describe
    // the same storage. No reference to the uninitialized slot remains from the owner above.
    let place = unsafe { &mut *place };

    Event::new_in_inner(place);

    // SAFETY: `new_in_inner()` initialized the event. Its internal `MaybeUninit` fields are
    // intentionally uninitialized according to the event state and do not invalidate `Event<T>`.
    let storage = unsafe { storage.assume_init() };

    PoolBox::into_raw(storage)
}

/// Drops an event that was rented from a pool and returns its slot to the pool.
///
/// The slot is owned by the pointer, not by the pool, so this needs neither a reference to the
/// pool nor its lock. The pool's storage stays alive while any event rented from it is alive, so
/// this remains valid even if the pool itself is already gone.
///
/// # Safety
///
/// The pointer must come from [`initialize_event()`] and must not have been passed here before.
/// Nothing may reference the event afterwards.
pub(crate) unsafe fn destroy_event<T: Send + 'static>(event: NonNull<UnsafeCell<Event<T>>>) {
    // SAFETY: Forwarding the guarantees of the caller, who promises that this is the single
    // `from_raw()` call matching the `into_raw()` that renting performed for this pointer.
    drop(unsafe { plurality::Box::<UnsafeCell<Event<T>>>::from_raw(event) });
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for PoolState<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("events", &self.events)
            .finish()
    }
}
