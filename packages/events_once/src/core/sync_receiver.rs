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
    Event, EventRef, ReflectiveTSend,
};

/// Receives a single value from the sender connected to the same event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `Receiver<BoxedRef<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
pub(crate) struct ReceiverCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    // This is `None` if the receiver has already been polled to completion. We need to guard
    // against that because the event will be cleaned up after the first poll that signals "ready".
    event_ref: Option<E>,

    // We are not compatible with concurrent receiver use from multiple threads.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E> ReceiverCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self {
            event_ref: Some(event_ref),
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
    pub(crate) fn into_value(self) -> Result<Result<E::T, Disconnected>, Self> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("Receiver polled after completion");

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
                Err(self)
            }
            EVENT_SET | EVENT_DISCONNECTED => {
                // Value available or disconnected - consume self and
                // let final_poll decide which endpoint performs the cleanup.
                let mut this = ManuallyDrop::new(self);
                let event_ref = this.event_ref.take().unwrap();

                match Event::final_poll(&event_ref) {
                    Ok(Some(value)) => {
                        event_ref.release_event();
                        Ok(Ok(value))
                    }
                    Ok(None) => {
                        // This shouldn't happen - final_poll should return Some(value) or Err(Disconnected)
                        unreachable!("final_poll returned None")
                    }
                    Err(Disconnected) => {
                        event_ref.release_event();
                        Ok(Err(Disconnected))
                    }
                }
            }
            _ => {
                unreachable!("Invalid event state: {}", current_state)
            }
        }
    }
}

impl<E> Future for ReceiverCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    type Output = Result<E::T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("Receiver polled after completion");

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

impl<E> Drop for ReceiverCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
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

impl<E> fmt::Debug for ReceiverCore<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event_ref", &self.event_ref)
            .finish()
    }
}
