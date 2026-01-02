use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::{fmt, ptr};

use crate::{Disconnected, LocalRef};

/// Delivers a single value to the receiver connected to the same event.
pub(crate) struct LocalSenderCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    event_ref: E,

    _t: PhantomData<fn(T)>,
}

impl<E, T> LocalSenderCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self {
            event_ref,
            _t: PhantomData,
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

impl<E, T> Drop for LocalSenderCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn drop(&mut self) {
        if self.event_ref.sender_dropped_without_set() == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            self.event_ref.release_event();
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<E, T> fmt::Debug for LocalSenderCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalSender")
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
    use crate::{BoxedLocalRef, PooledLocalRef, PtrLocalRef};

    assert_not_impl_any!(LocalSenderCore<BoxedLocalRef<i32>, i32>: Send, Sync);
    assert_not_impl_any!(LocalSenderCore<PtrLocalRef<i32>, i32>: Send, Sync);

    // The event payload being `!Unpin` should not cause the endpoints to become `!Unpin`.
    assert_impl_all!(LocalSenderCore<BoxedLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(LocalSenderCore<PtrLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(LocalSenderCore<PooledLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
}
