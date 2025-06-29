//! Rc-based senders and receivers for thread-safe events.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use super::Event;

/// A sender that can send a value through a thread-safe event using Rc ownership.
///
/// The sender owns an Rc to the event and is single-threaded.
/// After calling [`send`](ByRcEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRcEventSender<T>
where
    T: Send,
{
    pub(super) event: Rc<Event<T>>,
}

impl<T> ByRcEventSender<T>
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
    /// use std::rc::Rc;
    ///
    /// use events::once::Event;
    ///
    /// let event = Rc::new(Event::<i32>::new());
    /// let (sender, _receiver) = event.by_rc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        drop(self.event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe event using Rc ownership.
///
/// The receiver owns an Rc to the event and is single-threaded.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct ByRcEventReceiver<T>
where
    T: Send,
{
    pub(super) event: Rc<Event<T>>,
}

impl<T> Future for ByRcEventReceiver<T>
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
