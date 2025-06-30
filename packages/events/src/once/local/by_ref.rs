//! Reference-based senders and receivers for single-threaded events.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::Disconnected;

use super::LocalOnceEvent;

/// A sender that can send a value through a single-threaded event.
///
/// The sender holds a reference to the event and can only be used once.
/// After calling [`send`](ByRefLocalOnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefLocalOnceSender<'e, T> {
    pub(super) event: &'e LocalOnceEvent<T>,
    pub(super) used: bool,
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// let (sender, _receiver) = event.bind_by_ref();
    /// sender.send(42);
    /// ```
    pub fn send(mut self, value: T) {
        self.used = true;
        drop(self.event.try_set(value));
    }
}

impl<T> Drop for ByRefLocalOnceSender<'_, T> {
    fn drop(&mut self) {
        if !self.used {
            self.event.sender_dropped();
        }
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
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, Poll::Ready)
    }
}
