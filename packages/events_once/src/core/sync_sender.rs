use std::any::type_name;
use std::cell::Cell;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::{fmt, ptr};

use crate::{Disconnected, Event, EventRef, ReflectiveTSend};

/// Delivers a single value to the receiver connected to the same event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `Sender<BoxedRef<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
pub(crate) struct SenderCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    event_ref: E,

    // We are not compatible with concurrent sender use from multiple threads.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E> SenderCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self {
            event_ref,
            _not_sync: PhantomData,
        }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    pub(crate) fn send(self, value: E::T) {
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

impl<E> Drop for SenderCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn drop(&mut self) {
        if Event::sender_dropped_without_set(&self.event_ref) == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            self.event_ref.release_event();
        }
    }
}

impl<E> fmt::Debug for SenderCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event_ref", &self.event_ref)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{BoxedRef, PtrRef};

    assert_impl_all!(SenderCore<BoxedRef<u32>>: Send);
    assert_not_impl_any!(SenderCore<BoxedRef<u32>>: Sync);

    assert_impl_all!(SenderCore<PtrRef<u32>>: Send);
    assert_not_impl_any!(SenderCore<PtrRef<u32>>: Sync);
}
