//! Rc-based senders and receivers for single-threaded events.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use super::LocalOnceEvent;
use crate::Disconnected;

/// A sender that can send a value through a single-threaded event using Rc ownership.
///
/// The sender owns an Rc to the event and is single-threaded.
/// After calling [`send`](ByRcLocalOnceSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRcLocalOnceSender<T> {
    pub(super) event: Rc<LocalOnceEvent<T>>,
}

impl<T> ByRcLocalOnceSender<T> {
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let (sender, _receiver) = event.bind_by_rc();
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        self.event.set(value);
    }
}

impl<T> Drop for ByRcLocalOnceSender<T> {
    fn drop(&mut self) {
        self.event.sender_dropped();
    }
}

/// A receiver that can receive a value from a single-threaded event using Rc ownership.
///
/// The receiver owns an Rc to the event and is single-threaded.
/// After awaiting the receiver, it is consumed.
#[derive(Debug)]
pub struct ByRcLocalOnceReceiver<T> {
    pub(super) event: Rc<LocalOnceEvent<T>>,
}

impl<T> Future for ByRcLocalOnceReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll(cx.waker())
            .map_or_else(|| Poll::Pending, Poll::Ready)
    }
}
