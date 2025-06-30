//! Reference-based senders and receivers for single-threaded events.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::LocalOnceEvent;

/// A sender that can send a value through a single-threaded event.
///
/// The sender holds a reference to the event and can only be used once.
/// After calling [`send`](ByRefLocalOnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefLocalOnceSender<'e, T> {
    pub(super) event: &'e LocalOnceEvent<T>,
}

impl<T> ByRefLocalOnceSender<'_, T> {
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
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
pub struct ByRefLocalOnceReceiver<'e, T> {
    pub(super) event: &'e LocalOnceEvent<T>,
}

impl<T> Future for ByRefLocalOnceReceiver<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}
