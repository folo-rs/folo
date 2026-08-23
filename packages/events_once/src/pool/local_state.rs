use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::ptr::{self, NonNull};

use plurality::{Box as PoolBox, Pool};

use crate::{EVENT_COUNT_FITS_IN_USIZE, LocalEvent};

/// The storage that backs a pool of single-threaded events.
///
/// [`LocalEventPool`][crate::LocalEventPool] and
/// [`RawLocalEventPool`][crate::RawLocalEventPool] differ only in how they keep this alive, so
/// they share it. `plurality::Pool` allocates through shared references and is itself confined to
/// the thread of its local owner.
///
/// Releasing an event does not go through here at all: a slot is owned by the pointer that
/// [`rent()`][Self::rent] returns and is given back by [`destroy_local_event()`], which needs
/// neither the pool nor a borrow of it.
pub(crate) struct LocalPoolState<T: 'static> {
    events: Pool<UnsafeCell<LocalEvent<T>>>,
}

impl<T: 'static> LocalPoolState<T> {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            events: Pool::new(),
        }
    }

    /// Allocates an event and initializes it, handing ownership of its slot to the caller.
    ///
    /// The caller becomes responsible for passing the pointer to [`destroy_local_event()`]
    /// exactly once.
    #[must_use]
    pub(crate) fn rent(&self) -> NonNull<UnsafeCell<LocalEvent<T>>> {
        initialize_local_event(self.events.alloc_uninit_box())
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

/// Initializes a local event in freshly allocated plurality storage.
///
/// Both typed pools and heterogeneous lakes use the same placement path so that endpoint
/// references can release either source through [`destroy_local_event()`].
#[must_use]
pub(crate) fn initialize_local_event<T: 'static>(
    mut storage: PoolBox<MaybeUninit<UnsafeCell<LocalEvent<T>>>>,
) -> NonNull<UnsafeCell<LocalEvent<T>>> {
    // SAFETY: The slot is still uninitialized, so it carries no pinning invariant that unwrapping
    // the `Pin` could violate. We only use the reference to initialize the event in place.
    let place = unsafe { storage.as_pin_mut().get_unchecked_mut() };

    let place = ptr::from_mut(place).cast::<UnsafeCell<MaybeUninit<LocalEvent<T>>>>();

    // SAFETY: `MaybeUninit` and `UnsafeCell` are transparent wrappers, so both nestings describe
    // the same storage. No reference to the uninitialized slot remains from the owner above.
    let place = unsafe { &mut *place };

    LocalEvent::new_in_inner(place);

    // SAFETY: `new_in_inner()` initialized the event. Its internal `MaybeUninit` fields are
    // intentionally uninitialized according to the event state and do not invalidate the event.
    let storage = unsafe { storage.assume_init() };

    PoolBox::into_raw(storage)
}

/// Drops an event that was rented from a pool and returns its slot to the pool.
///
/// The slot is owned by the pointer, not by the pool, so this needs neither a reference to the
/// pool nor a borrow of it. The pool's storage stays alive while any event rented from it is
/// alive, so this remains valid even if the pool itself is already gone.
///
/// # Safety
///
/// The pointer must come from [`initialize_local_event()`] and must not have been passed here
/// before. Nothing may reference the event afterwards.
pub(crate) unsafe fn destroy_local_event<T: 'static>(event: NonNull<UnsafeCell<LocalEvent<T>>>) {
    // SAFETY: Forwarding the guarantees of the caller, who promises that this is the single
    // `from_raw()` call matching the `into_raw()` that renting performed for this pointer.
    drop(unsafe { plurality::Box::<UnsafeCell<LocalEvent<T>>>::from_raw(event) });
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for LocalPoolState<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("events", &self.events)
            .finish()
    }
}
