use std::any::type_name;
use std::cell::Cell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::sync::atomic;
use std::task::{self, Poll};

use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, EVENT_SIGNALING,
    Event, EventRef, IntoValueError,
};

/// Receives a single value from the sender connected to the same event.
pub(crate) struct ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    // This is `None` if the receiver has already been polled to completion. We need to guard
    // against that because the event will be cleaned up after the first poll that signals "ready".
    event_ref: Option<E>,

    _t: PhantomData<fn() -> T>,

    // We are not compatible with concurrent receiver use from multiple threads.
    // This is just to leave us design flexibility until we have a concrete use case for it.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E, T> ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self {
            event_ref: Some(event_ref),
            _t: PhantomData,
            _not_sync: PhantomData,
        }
    }

    /// Checks whether a value is ready to be received.
    ///
    /// Both a real value and a "disconnected" signal count,
    /// as they are just different kinds of values.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    #[must_use]
    pub(crate) fn is_ready(&self) -> bool {
        let Some(event_ref) = &self.event_ref else {
            panic!("receiver queried after completion");
        };

        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method with `Some(event_ref)`.
        let event_maybe = unsafe { event_ref.get().as_ref() };
        // SAFETY: UnsafeCell pointer is never null.
        let event = unsafe { event_maybe.unwrap_unchecked() };

        event.is_set()
    }

    /// Consumes the receiver and transforms it into the received value, if the value is available.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available value without blocking. It returns `Some(value)` if a value has
    /// already been sent, or `None` if no value is currently available.
    ///
    /// # Panics
    ///
    /// Panics if the value has already been received via `Future::poll()`.
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    pub(crate) fn into_value(self) -> Result<T, IntoValueError<Self>> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("Receiver polled after completion: Future trait contract violated");

        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method with `Some(event_ref)`.
        let event_maybe = unsafe { event_ref.get().as_ref() };
        // SAFETY: UnsafeCell pointer is never null.
        let event = unsafe { event_maybe.unwrap_unchecked() };

        let current_state = event.state.load(atomic::Ordering::Acquire);

        match current_state {
            EVENT_BOUND | EVENT_AWAITING | EVENT_SIGNALING => {
                // No value available yet - return the receiver
                Err(IntoValueError::Pending(self))
            }
            EVENT_SET | EVENT_DISCONNECTED => {
                // Value available or disconnected - consume self and
                // let final_poll decide which endpoint performs the cleanup.
                let mut this = ManuallyDrop::new(self);
                let event_ref = this.event_ref.take().unwrap();

                match Event::final_poll(&event_ref) {
                    Ok(Some(value)) => {
                        event_ref.release_event();
                        Ok(value)
                    }
                    Ok(None) => {
                        // This should not happen - final_poll should return Some(value) or Err(Disconnected)
                        unreachable!("final_poll returned None")
                    }
                    Err(Disconnected) => {
                        event_ref.release_event();
                        Err(IntoValueError::Disconnected)
                    }
                }
            }
            _ => {
                unreachable!("Invalid event state: {}", current_state)
            }
        }
    }
}

impl<E, T> Future for ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    type Output = Result<T, Disconnected>;

    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("Receiver polled after completion: Future trait contract violated");

        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method with `Some(event_ref)`.
        let event_maybe = unsafe { event_ref.get().as_ref() };
        // SAFETY: UnsafeCell pointer is never null.
        let event = unsafe { event_maybe.unwrap_unchecked() };

        let inner_poll_result = event.poll(cx.waker());

        // If the poll returns `Some`, we need to clean up the event.
        if inner_poll_result.is_some() {
            // We have a slight problem here, though, because if we release the event here and
            // the memory is freed, what happens if someone foolishly tries to poll again? That
            // could lead to a memory safety violation if we did it naively. Therefore, we have
            // to guard against double polling via an `Option`.
            event_ref.release_event();

            // SAFETY: We are not moving anything, merely updating a field.
            let this = unsafe { self.get_unchecked_mut() };
            this.event_ref = None;
        }

        inner_poll_result.map_or_else(|| Poll::Pending, Poll::Ready)
    }
}

impl<E, T> Drop for ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn drop(&mut self) {
        if let Some(event_ref) = self.event_ref.take() {
            match Event::final_poll(&event_ref) {
                Ok(None) => {
                    // Nothing for us to do - the sender was still connected and had not
                    // sent any value, so it will perform the cleanup on its own.
                }
                _ => {
                    // The sender has already disconnected, so we need to clean up the event.
                    event_ref.release_event();
                }
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<E, T> fmt::Debug for ReceiverCore<E, T>
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

    assert_impl_all!(ReceiverCore<BoxedRef<u32>, u32>: Send);
    assert_not_impl_any!(ReceiverCore<BoxedRef<u32>, u32>: Sync);

    assert_impl_all!(ReceiverCore<PtrRef<u32>, u32>: Send);
    assert_not_impl_any!(ReceiverCore<PtrRef<u32>, u32>: Sync);

    // The event payload being `!Unpin` should not cause the endpoints to become `!Unpin`.
    assert_impl_all!(ReceiverCore<BoxedRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(ReceiverCore<PtrRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(ReceiverCore<PooledRef<PhantomPinned>, PhantomPinned>: Unpin);
}
