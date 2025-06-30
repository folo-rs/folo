//! Raw pointer-based senders and receivers for thread-safe `OnceEvents`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::OnceEvent;

/// A sender that can send a value through a thread-safe `OnceEvent` using raw pointer.
///
/// The sender holds a raw pointer to the `OnceEvent`. The caller is responsible for
/// ensuring the `OnceEvent` remains valid for the lifetime of the sender.
/// After calling [`send`](ByPtrOnceSender::send), the sender is consumed.
///
/// # Safety
///
/// This type is only safe to use when the caller guarantees that the `OnceEvent`
/// pointed to remains valid and pinned for the entire lifetime of this sender.
#[derive(Debug)]
pub struct ByPtrOnceSender<T>
where
    T: Send,
{
    pub(super) once_event: *const OnceEvent<T>,
}

// SAFETY: ByPtrOnceSender can be Send as long as T: Send, since we only
// send the value T across threads, and the pointer is only used to access
// the thread-safe OnceEvent<T>.
unsafe impl<T> Send for ByPtrOnceSender<T> where T: Send {}

// SAFETY: ByPtrOnceSender can be Sync as long as T: Send, since the
// OnceEvent<T> it points to is thread-safe.
unsafe impl<T> Sync for ByPtrOnceSender<T> where T: Send {}

impl<T> ByPtrOnceSender<T>
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
    /// use std::pin::Pin;
    ///
    /// use events::OnceEvent;
    ///
    /// let mut event = OnceEvent::<i32>::new();
    /// let pinned_event = Pin::new(&mut event);
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, _receiver) = unsafe { pinned_event.bind_by_ptr() };
    /// sender.send(42);
    /// ```
    pub fn send(self, value: T) {
        // SAFETY: The caller guarantees the once_event pointer is valid
        let once_event = unsafe { &*self.once_event };
        drop(once_event.try_set(value));
    }
}

/// A receiver that can receive a value from a thread-safe `OnceEvent` using raw pointer.
///
/// The receiver holds a raw pointer to the `OnceEvent`. The caller is responsible for
/// ensuring the `OnceEvent` remains valid for the lifetime of the receiver.
/// After awaiting the receiver, it is consumed.
///
/// # Safety
///
/// This type is only safe to use when the caller guarantees that the `OnceEvent`
/// pointed to remains valid and pinned for the entire lifetime of this receiver.
#[derive(Debug)]
pub struct ByPtrOnceReceiver<T>
where
    T: Send,
{
    pub(super) once_event: *const OnceEvent<T>,
}

// SAFETY: ByPtrOnceReceiver can be Send as long as T: Send, since we only
// receive the value T across threads, and the pointer is only used to access
// the thread-safe OnceEvent<T>.
unsafe impl<T> Send for ByPtrOnceReceiver<T> where T: Send {}

// Note: We don't implement Sync for ByPtrOnceReceiver to match the pattern
// of other receiver types in this codebase.

impl<T> Future for ByPtrOnceReceiver<T>
where
    T: Send,
{
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: The caller guarantees the once_event pointer is valid
        let once_event = unsafe { &*self.once_event };
        once_event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}
