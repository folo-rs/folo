//! Arc-based senders and receivers for thread-safe events.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use super::Event;

/// A sender that can send a value through a thread-safe event using Arc ownership.
///
/// The sender owns an Arc to the event and can be moved across threads.
/// After calling [`send`](ByArcEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByArcEventSender<T>
where
    T: Send,
{
    pub(super) event: Arc<Event<T>>,
}

impl<T> ByArcEventSender<T>
where
    T: Send,
{
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::once::Event;
    ///
    /// let event = Arc::new(Event::<i32>::new());
    /// let (sender, _receiver) = event.by_arc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe event using Arc ownership.
///
/// The receiver owns an Arc to the event and can be moved across threads.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct ByArcEventReceiver<T>
where
    T: Send,
{
    pub(super) event: Arc<Event<T>>,
}

impl<T> Future for ByArcEventReceiver<T>
where
    T: Send,
{
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}
