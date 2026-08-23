use std::any::type_name;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::{fmt, ptr};

use crate::{Disconnected, LocalEvent, LocalRef};

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
    #[inline]
    pub(crate) fn send(self, value: T) {
        // The drop logic is different before/after set(), so we prevent the sender destructor
        // from running and move its reference into an ordinary local. The local is still dropped
        // if a waker callback unwinds out of `set()`.
        let this = ManuallyDrop::new(self);

        // SAFETY: `this` will never be dropped, so reading its event reference transfers that
        // field into `event_ref` exactly once. The marker field needs no destruction.
        let event_ref = unsafe { ptr::read(&raw const this.event_ref) };

        if let Err(value) = LocalEvent::set(&event_ref, value) {
            // SAFETY: `set()` reported that the receiver had already disconnected, which is the
            // transition that grants this sender sole cleanup responsibility, and we do not
            // access the event afterwards.
            unsafe {
                event_ref.release_event();
            }

            // Release all event-owned resources before payload destruction invokes user code.
            drop(event_ref);
            drop(value);
            return;
        }

        drop(event_ref);
    }
}

impl<E, T> Drop for LocalSenderCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    #[inline]
    fn drop(&mut self) {
        if LocalEvent::sender_dropped_without_set(&self.event_ref) == Err(Disconnected) {
            // SAFETY: `sender_dropped_without_set()` reported that the receiver had already
            // disconnected, which is the transition that grants this sender sole cleanup
            // responsibility, and we do not access the event afterwards.
            unsafe {
                self.event_ref.release_event();
            }
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
    use crate::{BoxedLocalRef, PooledLocalRef, PtrLocalRef};

    assert_not_impl_any!(LocalSenderCore<BoxedLocalRef<i32>, i32>: Send, Sync);
    assert_not_impl_any!(LocalSenderCore<PtrLocalRef<i32>, i32>: Send, Sync);

    // The event payload being `!Unpin` should not cause the endpoints to become `!Unpin`.
    assert_impl_all!(LocalSenderCore<BoxedLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(LocalSenderCore<PtrLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(LocalSenderCore<PooledLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
}
