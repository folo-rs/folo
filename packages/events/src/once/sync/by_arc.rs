//! Arc-based senders and receivers for thread-safe `OnceEvents`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use super::OnceEvent;

/// A sender that can send a value through a thread-safe `OnceEvent` using Arc ownership.
///
/// The sender owns an Arc to the `OnceEvent` and can be moved across threads.
/// After calling [`send`](ByArcOnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByArcOnceSender<T>
where
    T: Send,
{
    pub(super) once_event: Arc<OnceEvent<T>>,
}

impl<T> ByArcOnceSender<T>
where
    T: Send,
{
    /// Sends a value through the `OnceEvent`.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// let (sender, _receiver) = event.bind_by_arc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        drop(self.once_event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe `OnceEvent` using Arc ownership.
///
/// The receiver owns an Arc to the `OnceEvent` and can be moved across threads.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct ByArcOnceReceiver<T>
where
    T: Send,
{
    pub(super) once_event: Arc<OnceEvent<T>>,
}

impl<T> Future for ByArcOnceReceiver<T>
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
