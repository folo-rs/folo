//! Raw pointer-based senders and receivers for thread-safe events.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::Event;

/// A sender that can send a value through a thread-safe event using raw pointer.
///
/// The sender holds a raw pointer to the event. The caller is responsible for
/// ensuring the event remains valid for the lifetime of the sender.
/// After calling [`send`](ByPtrEventSender::send), the sender is consumed.
///
/// # Safety
///
/// This type is only safe to use when the caller guarantees that the event
/// pointed to remains valid and pinned for the entire lifetime of this sender.
#[derive(Debug)]
pub struct ByPtrEventSender<T>
where
    T: Send,
{
    pub(super) event: *const Event<T>,
}

// SAFETY: ByPtrEventSender can be Send as long as T: Send, since we only
// send the value T across threads, and the pointer is only used to access
// the thread-safe Event<T>.
unsafe impl<T> Send for ByPtrEventSender<T> where T: Send {}

// SAFETY: ByPtrEventSender can be Sync as long as T: Send, since the
// Event<T> it points to is thread-safe.
unsafe impl<T> Sync for ByPtrEventSender<T> where T: Send {}

impl<T> ByPtrEventSender<T>
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
    /// use std::pin::Pin;
    ///
    /// use events::once::Event;
    ///
    /// let mut event = Event::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, _receiver) = unsafe { pinned_event.by_ptr() };
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        // SAFETY: The caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        drop(event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe event using raw pointer.
///
/// The receiver holds a raw pointer to the event. The caller is responsible for
/// ensuring the event remains valid for the lifetime of the receiver.
/// After awaiting the receiver, it is consumed.
///
/// # Safety
///
/// This type is only safe to use when the caller guarantees that the event
/// pointed to remains valid and pinned for the entire lifetime of this receiver.
#[derive(Debug)]
pub struct ByPtrEventReceiver<T>
where
    T: Send,
{
    pub(super) event: *const Event<T>,
}

// SAFETY: ByPtrEventReceiver can be Send as long as T: Send, since we only
// receive the value T across threads, and the pointer is only used to access
// the thread-safe Event<T>.
unsafe impl<T> Send for ByPtrEventReceiver<T> where T: Send {}

// Note: We don't implement Sync for ByPtrEventReceiver to match the pattern
// of other receiver types in this codebase.

impl<T> Future for ByPtrEventReceiver<T>
where
    T: Send,
{
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: The caller guarantees the event pointer is valid
        let event = unsafe { &*self.event };
        event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}
