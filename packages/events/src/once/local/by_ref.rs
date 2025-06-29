//! Reference-based senders and receivers for single-threaded events.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::LocalEvent;

/// A sender that can send a value through a single-threaded event.
///
/// The sender holds a reference to the event and can only be used once.
/// After calling [`send`](ByRefLocalEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefLocalEventSender<'e, T> {
    pub(super) event: &'e LocalEvent<T>,
}

impl<T> ByRefLocalEventSender<'_, T> {
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let (sender, _receiver) = event.by_ref();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a single-threaded event.
///
/// The receiver holds a reference to the event and can only be used once.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct ByRefLocalEventReceiver<'e, T> {
    pub(super) event: &'e LocalEvent<T>,
}

impl<T> Future for ByRefLocalEventReceiver<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}
