//! Rc-based senders and receivers for thread-safe OnceEvents.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use super::OnceEvent;

/// A sender that can send a value through a thread-safe OnceEvent using Rc ownership.
///
/// The sender owns an Rc to the OnceEvent and is single-threaded.
/// After calling [`send`](ByRcOnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRcOnceSender<T>
where
    T: Send,
{
    pub(super) once_event: Rc<OnceEvent<T>>,
}

impl<T> ByRcOnceSender<T>
where
    T: Send,
{
    /// Sends a value through the OnceEvent.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use OnceEvents::once::OnceEvent;
    ///
    /// let OnceEvent = Rc::new(once_event::<i32>::new());
    /// let (sender, _receiver) = OnceEvent.by_rc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        drop(self.once_event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe OnceEvent using Rc ownership.
///
/// The receiver owns an Rc to the OnceEvent and is single-threaded.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct ByRcOnceReceiver<T>
where
    T: Send,
{
    pub(super) once_event: Rc<OnceEvent<T>>,
}

impl<T> Future for ByRcOnceReceiver<T>
where
    T: Send,
{
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.once_event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}
