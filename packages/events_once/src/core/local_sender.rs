use std::mem::ManuallyDrop;
use std::{fmt, ptr};

use crate::{Disconnected, LocalRef, ReflectiveT};

/// Delivers a single value to the receiver connected to the same event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `LocalSender<BoxedLocalRef<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
pub(crate) struct LocalSenderCore<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    event_ref: E,
}

impl<E> LocalSenderCore<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self { event_ref }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    pub(crate) fn send(self, value: E::T) {
        // The drop logic is different before/after set(), so we switch to manual drop here.
        let mut this = ManuallyDrop::new(self);

        if this.event_ref.set(value) == Err(Disconnected) {
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

impl<E> Drop for LocalSenderCore<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    fn drop(&mut self) {
        if self.event_ref.sender_dropped_without_set() == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            self.event_ref.release_event();
        }
    }
}

impl<E> fmt::Debug for LocalSenderCore<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalSender")
            .field("event_ref", &self.event_ref)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;
    use crate::{BoxedLocalRef, PtrLocalRef};

    assert_not_impl_any!(LocalSenderCore<BoxedLocalRef<i32>>: Send, Sync);
    assert_not_impl_any!(LocalSenderCore<PtrLocalRef<i32>>: Send, Sync);
}
