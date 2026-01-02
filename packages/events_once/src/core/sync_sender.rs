use std::any::type_name;
use std::cell::Cell;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::{fmt, ptr};

use crate::{Disconnected, Event, EventRef};

/// Delivers a single value to the receiver connected to the same event.
pub(crate) struct SenderCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    event_ref: E,

    _t: PhantomData<fn(T)>,

    // We are not compatible with concurrent sender use from multiple threads.
    // This is just to leave us design flexibility - the API consumes the sender
    // so there is not really anything you can do with it concurrently anyway.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E, T> SenderCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self {
            event_ref,
            _t: PhantomData,
            _not_sync: PhantomData,
        }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    pub(crate) fn send(self, value: T) {
        // The drop logic is different before/after set(), so we switch to manual drop here.
        let mut this = ManuallyDrop::new(self);

        if Event::set(&this.event_ref, value) == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            this.event_ref.release_event();
        }

        // SAFETY: The field contains a valid object of the right type. We avoid a double-drop
        // via ManuallyDrop above. We consume `self` so nothing further can happen.
        unsafe {
            ptr::drop_in_place(&raw mut this.event_ref);
        }
    }
}

impl<E, T> Drop for SenderCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn drop(&mut self) {
        if Event::sender_dropped_without_set(&self.event_ref) == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            self.event_ref.release_event();
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<E, T> fmt::Debug for SenderCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event_ref", &self.event_ref)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::marker::PhantomPinned;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{BoxedRef, PooledRef, PtrRef};

    assert_impl_all!(SenderCore<BoxedRef<u32>, u32>: Send);
    assert_not_impl_any!(SenderCore<BoxedRef<u32>, u32>: Sync);

    assert_impl_all!(SenderCore<PtrRef<u32>, u32>: Send);
    assert_not_impl_any!(SenderCore<PtrRef<u32>, u32>: Sync);

    // The event payload being `!Unpin` should not cause the endpoints to become `!Unpin`.
    assert_impl_all!(SenderCore<BoxedRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(SenderCore<PtrRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(SenderCore<PooledRef<PhantomPinned>, PhantomPinned>: Unpin);
}
