//! Raw pointer-based senders and receivers for single-threaded events.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::LocalOnceEvent;
use crate::Disconnected;

/// A sender that can send a value through a single-threaded event using raw pointer access.
///
/// The sender holds a raw pointer to the event and the caller is responsible for
/// ensuring the event remains valid for the lifetime of the sender.
///
/// After calling [`send`](ByPtrLocalOnceSender::send), the sender is consumed.
///
/// # Safety
///
/// The caller must ensure that the event this instance is bound to remains
/// alive and pinned for the entire lifetime of this sender.
#[derive(Debug)]
pub struct ByPtrLocalOnceSender<T> {
    pub(super) event: *const LocalOnceEvent<T>,
}

impl<T> ByPtrLocalOnceSender<T> {
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
    /// let mut event = Box::pin(LocalOnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, _receiver) = unsafe { event.as_mut().bind_by_ptr() };
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        // SAFETY: Caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        event.set(value);
    }
}

impl<T> Drop for ByPtrLocalOnceSender<T> {
    fn drop(&mut self) {
        // SAFETY: Caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        event.sender_dropped();
    }
}

/// A receiver that can receive a value from a single-threaded event using raw pointer access.
///
/// The receiver holds a raw pointer to the event and the caller is responsible for
/// ensuring the event remains valid for the lifetime of the receiver.
/// After awaiting the receiver, it is consumed.
///
/// # Safety
///
/// The caller must ensure that the event remains valid and pinned for the entire
/// lifetime of this receiver.
#[derive(Debug)]
pub struct ByPtrLocalOnceReceiver<T> {
    pub(super) event: *const LocalOnceEvent<T>,
}

impl<T> Future for ByPtrLocalOnceReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: Caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        event
            .poll(cx.waker())
            .map_or_else(|| Poll::Pending, Poll::Ready)
    }
}
