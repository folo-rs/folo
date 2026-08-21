use std::any::type_name;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, IntoValueError,
    LocalEvent, LocalRef,
};

/// Receives a single value from the sender connected to the same event.
pub(crate) struct LocalReceiverCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    // This is `None` if the receiver has already been polled to completion. We need to guard
    // against that because the event will be cleaned up after the first poll that signals "ready".
    event_ref: Option<E>,

    _t: PhantomData<fn() -> T>,
}

impl<E, T> LocalReceiverCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self {
            event_ref: Some(event_ref),
            _t: PhantomData,
        }
    }

    /// Derives the shared event reference that every operation of this endpoint works through.
    fn event(event_ref: &E) -> &LocalEvent<T> {
        // SAFETY: Validity: the `LocalRef` contract guarantees that dereferencing yields one
        // initialized and aligned event that stays valid for as long as this endpoint can access
        // it, which includes the returned reference. Aliasing: the same contract limits all
        // access to shared references, and the event manages its interior fields itself, so no
        // exclusive reference to the event can exist.
        unsafe { &*event_ref.get() }
    }

    /// Checks whether the event has completed, in which case reception can finish immediately.
    ///
    /// Completion means either that a value has been sent or that the sender disconnected
    /// without sending one, so a completed event does not necessarily yield a value. See
    /// `state.rs` for the canonical meaning of the event states.
    ///
    /// Only valid before the receiver has completed.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    #[must_use]
    pub(crate) fn is_ready(&self) -> bool {
        let Some(event_ref) = &self.event_ref else {
            panic!("receiver inspected after completion");
        };

        Self::event(event_ref).is_set()
    }

    /// Consumes the receiver and returns the value, if the event has completed with one.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available value without blocking. It returns `Ok(value)` if a value has
    /// been sent, `Err(IntoValueError::Pending(self))` if the event has not completed yet, and
    /// `Err(IntoValueError::Disconnected)` if the sender disconnected without sending a value.
    ///
    /// Only valid before the receiver has completed.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    pub(crate) fn into_value(self) -> Result<T, IntoValueError<Self>> {
        let Some(event_ref) = self.event_ref.as_ref() else {
            panic!("receiver consumed after completion");
        };

        let current_state = Self::event(event_ref).state.get();

        match current_state {
            EVENT_BOUND | EVENT_AWAITING => {
                // The event has not completed, so we return the receiver to the caller and the
                // event remains in the care of both endpoints.
                Err(IntoValueError::Pending(self))
            }
            EVENT_SET | EVENT_DISCONNECTED => {
                // The event has completed - consume self and
                // let final_poll decide which endpoint performs the cleanup.
                let mut this = ManuallyDrop::new(self);
                let event_ref = this.event_ref.take().expect(
                    "event_ref was proven present above and neither the state inspection nor \
                     wrapping self in ManuallyDrop can clear it",
                );

                match LocalEvent::final_poll(&event_ref) {
                    Ok(Some(value)) => {
                        // SAFETY: `final_poll` returning a value means the state machine made
                        // the receiver the last endpoint and assigned it cleanup ownership. We
                        // do not access the event after this call - the receiver is consumed
                        // and its reference goes out of scope.
                        unsafe {
                            event_ref.release_event();
                        }

                        Ok(value)
                    }
                    Ok(None) => {
                        // Defensive: state machine guarantees final_poll
                        // always returns Some or Err when the event has completed.
                        unreachable!(
                            "{} reported no result on value extraction from a terminal state",
                            type_name::<LocalEvent<T>>()
                        )
                    }
                    Err(Disconnected) => {
                        // SAFETY: `final_poll` reporting disconnection means the state machine
                        // made the receiver the last endpoint and assigned it cleanup ownership.
                        // We do not access the event after this call - the receiver is consumed
                        // and its reference goes out of scope.
                        unsafe {
                            event_ref.release_event();
                        }

                        Err(IntoValueError::Disconnected)
                    }
                }
            }
            // Defensive: state machine guarantees this is unreachable.
            _ => {
                unreachable!(
                    "unreachable {} state on value extraction: {current_state}",
                    type_name::<LocalEvent<T>>()
                )
            }
        }
    }
}

impl<E, T> Future for LocalReceiverCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    type Output = Result<T, Disconnected>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // The receiver core is never structurally pinned: it holds an endpoint reference, which
        // is a pointer handle, and refers to the payload only through markers. Projecting via
        // `get_mut` keeps that fact compiler-verified instead of assumed.
        let this = self.get_mut();

        let inner_poll_result = {
            let Some(event_ref) = this.event_ref.as_ref() else {
                panic!("receiver polled after completion: Future trait contract violated");
            };

            Self::event(event_ref).poll(cx.waker())
        };

        if inner_poll_result.is_some() {
            // A result means the event reached a terminal state, and it may have handed us a
            // payload out of storage that the state still advertises as initialized (see
            // `LocalEvent::poll_set()`). We therefore release the event right here, with no
            // user-controlled code in between and no further state-driven cleanup.

            // We remove the reference from the receiver before releasing the event, which is
            // what makes any later operation on the receiver panic instead of reaching released
            // storage.
            let event_ref = this.event_ref.take().expect(
                "the poll above panics unless the reference is present, and it cannot be \
                 cleared while the poll holds an exclusive reference to the receiver",
            );

            // SAFETY: A result from `poll` means the state machine reached a terminal state and
            // assigned cleanup ownership to the receiver. The reference no longer belongs to the
            // receiver, so nothing accesses the event after this call.
            unsafe {
                event_ref.release_event();
            }
        }

        inner_poll_result.map_or_else(|| Poll::Pending, Poll::Ready)
    }
}

impl<E, T> Drop for LocalReceiverCore<E, T>
where
    E: LocalRef<T>,
    T: 'static,
{
    #[inline]
    fn drop(&mut self) {
        if let Some(event_ref) = self.event_ref.take() {
            match LocalEvent::final_poll(&event_ref) {
                Ok(None) => {
                    // Nothing for us to do - the sender was still connected and had not
                    // sent any value, so it will perform the cleanup on its own.
                }
                _ => {
                    // Either a value was waiting for us or the sender has disconnected. Both
                    // outcomes leave the receiver as the last endpoint, so we release the event.
                    // A value delivered here is intentionally discarded with the match
                    // temporary - nobody is left to receive it.

                    // SAFETY: `final_poll` returned a terminal outcome, which is how the state
                    // machine assigns cleanup ownership to the receiver. The receiver is being
                    // dropped and its reference goes out of scope, so nothing accesses the event
                    // after this call.
                    unsafe {
                        event_ref.release_event();
                    }
                }
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<E, T> fmt::Debug for LocalReceiverCore<E, T>
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

    assert_not_impl_any!(LocalReceiverCore<BoxedLocalRef<i32>, i32>: Send, Sync);
    assert_not_impl_any!(LocalReceiverCore<PtrLocalRef<i32>, i32>: Send, Sync);

    // The event payload being `!Unpin` should not cause the endpoints to become `!Unpin`.
    assert_impl_all!(LocalReceiverCore<BoxedLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(LocalReceiverCore<PtrLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(LocalReceiverCore<PooledLocalRef<PhantomPinned>, PhantomPinned>: Unpin);
}
