//! Reference-based senders and receivers for thread-safe `OnceEvents`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::Disconnected;

use super::OnceEvent;

/// A sender that can send a value through a thread-safe `OnceEvent`.
///
/// The sender holds a reference to the `OnceEvent` and can be moved across threads.
/// After calling [`send`](ByRefOnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefOnceSender<'e, T>
where
    T: Send,
{
    pub(super) once_event: &'e OnceEvent<T>,
    pub(super) used: bool,
}

impl<T> ByRefOnceSender<'_, T>
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
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// let (sender, _receiver) = event.bind_by_ref();
    /// sender.send(42);
    /// ```
    pub fn send(mut self, value: T) {
        self.used = true;
        drop(self.once_event.try_set(value));
    }
}

impl<T> Drop for ByRefOnceSender<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        if !self.used {
            self.once_event.sender_dropped();
        }
    }
}

/// A receiver that can receive a value from a thread-safe `OnceEvent`.
///
/// The receiver holds a reference to the `OnceEvent` and can be moved across threads.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct ByRefOnceReceiver<'e, T>
where
    T: Send,
{
    pub(super) once_event: &'e OnceEvent<T>,
}

impl<T> Future for ByRefOnceReceiver<'_, T>
where
    T: Send,
{
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.once_event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, Poll::Ready)
    }
}
