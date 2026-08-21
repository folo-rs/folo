use std::any::type_name;
use std::cell::Cell;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::{fmt, ptr};

use crate::{Disconnected, Event, EventRef, ReleaseGuard};

/// Delivers a single value to the receiver connected to the same event.
pub(crate) struct SenderCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    event_ref: E,

    _t: PhantomData<fn(T)>,

    // Cell<()> is natively Send + !Sync, which opts the type out of Sync without requiring
    // an unsafe impl Send. Using PhantomData<*mut ()> + unsafe impl Send would be simpler
    // but triggers a rustc bug (rust-lang/rust#110338) in async generator Send inference
    // See: https://github.com/folo-rs/folo/issues/142
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
    #[inline]
    pub(crate) fn send(self, value: T) {
        // The sender's Drop path signals disconnection, which must not run after sending.
        let mut this = ManuallyDrop::new(self);
        let event_ref_ptr = &raw mut this.event_ref;
        let _drop_event_ref = ReleaseGuard::new(|| {
            // SAFETY: The pointer targets the initialized endpoint handle in `this`. The sender is
            // manually dropped, this guard runs exactly once, and no later code accesses the field.
            unsafe {
                ptr::drop_in_place(event_ref_ptr);
            }
        });

        // SAFETY: The pointer targets the initialized endpoint handle above. Only shared access is
        // created before the drop guard destroys it at the end of this method or during unwinding.
        let event_ref = unsafe { &*event_ref_ptr };

        if let Err(value) = Event::set(event_ref, value) {
            let _release = ReleaseGuard::new(|| {
                // SAFETY: `set()` returned the undelivered value after observing that the receiver
                // had disconnected, which grants this sender sole cleanup responsibility. The
                // guard runs at most once and this method never accesses the event afterwards.
                unsafe {
                    event_ref.release_event();
                }
            });

            drop(value);
        }
    }
}

impl<E, T> Drop for SenderCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    #[inline]
    fn drop(&mut self) {
        if Event::sender_dropped_without_set(&self.event_ref) == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.

            // SAFETY: `sender_dropped_without_set` reporting disconnection is how the state
            // machine assigns cleanup ownership to the sender, which by then is the last
            // endpoint. The sender is being dropped, so nothing accesses the event afterwards.
            unsafe {
                self.event_ref.release_event();
            }
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

    // Trait object payloads must preserve Send (regression test for #142).
    assert_impl_all!(SenderCore<BoxedRef<Box<dyn Send>>, Box<dyn Send>>: Send);

    // The event payload being `!Unpin` should not cause the endpoints to become `!Unpin`.
    assert_impl_all!(SenderCore<BoxedRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(SenderCore<PtrRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(SenderCore<PooledRef<PhantomPinned>, PhantomPinned>: Unpin);
}
