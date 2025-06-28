//! Thread-safe one-time events.
//!
//! This module provides thread-safe event types that can be shared across threads
//! and used for cross-thread communication.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A one-time event that can send and receive a value of type `T`.
///
/// This is the thread-safe variant that can create sender and receiver endpoints
/// that can be used across threads. The event itself can be shared across threads
/// but is typically used as a factory to create endpoints and then discarded.
///
/// The event can only be used once - after obtaining the sender and receiver,
/// subsequent calls to obtain them will panic (or return [`None`] for the checked variants).
///
/// For single-threaded usage, see [`crate::once::LocalEvent`] which has lower overhead.
///
/// # Thread Safety
///
/// This type requires `T: Send` as values will be sent across thread boundaries.
/// The sender and receiver implement `Send + Sync` when `T: Send`.
/// The Event itself is thread-safe because it can be referenced from other threads
/// via the sender and receiver endpoints (which hold references to the Event).
/// This thread-safety is needed even though the Event is typically used once
/// and then discarded.
///
/// # Example
///
/// ```rust
/// use events::once::Event;
///
/// let event = Event::<String>::new();
/// let (sender, receiver) = event.by_ref();
///
/// sender.send("Hello".to_string());
/// let message = receiver.recv();
/// assert_eq!(message, "Hello");
/// ```
#[derive(Debug)]
pub struct Event<T>
where
    T: Send,
{
    // AtomicBool to track whether endpoints have been retrieved (thread-safe)
    used: AtomicBool,
    // Mutex is needed because the Event can be referenced from multiple threads
    // via the sender and receiver endpoints, requiring thread-safe access
    // to the channel for one-time endpoint extraction
    channel: Mutex<Option<(oneshot::Sender<T>, oneshot::Receiver<T>)>>,
}

impl<T> Event<T>
where
    T: Send,
{
    /// Creates a new thread-safe event.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        let (sender, receiver) = oneshot::channel();
        Self {
            used: AtomicBool::new(false),
            channel: Mutex::new(Some((sender, receiver))),
        }
    }

    /// Returns both the sender and receiver for this event, connected by reference.
    ///
    /// # Panics
    ///
    /// Panics if this method or [`by_ref_checked`](Event::by_ref_checked) has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    /// ```
    pub fn by_ref(&self) -> (ByRefEventSender<'_, T>, ByRefEventReceiver<'_, T>) {
        self.by_ref_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event, connected by reference,
    /// or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let endpoints = event.by_ref_checked().unwrap();
    /// let endpoints2 = event.by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn by_ref_checked(&self) -> Option<(ByRefEventSender<'_, T>, ByRefEventReceiver<'_, T>)> {
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        let (sender, receiver) = self.channel.lock().unwrap().take()?;
        Some((
            ByRefEventSender {
                _event: self,
                sender: Some(sender),
            },
            ByRefEventReceiver {
                _event: self,
                receiver: Some(receiver),
            },
        ))
    }
}

impl<T> Default for Event<T>
where
    T: Send,
{
    fn default() -> Self {
        Self::new()
    }
}

/// A sender that can send a value through a thread-safe event.
///
/// The sender holds a reference to the event and can be moved across threads.
/// After calling [`send`](ByRefEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefEventSender<'e, T>
where
    T: Send,
{
    _event: &'e Event<T>,
    sender: Option<oneshot::Sender<T>>,
}

impl<T> ByRefEventSender<'_, T>
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
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, _receiver) = event.by_ref();
    /// sender.send(42);
    /// ```
    pub fn send(mut self, value: T) {
        if let Some(sender) = self.sender.take() {
            // We don't care if the receiver is dropped - sending always succeeds
            drop(sender.send(value));
        }
    }
}

/// A receiver that can receive a value from a thread-safe event.
///
/// The receiver holds a reference to the event and can be moved across threads.
/// After calling [`recv`](ByRefEventReceiver::recv), the receiver is consumed.
#[derive(Debug)]
pub struct ByRefEventReceiver<'e, T>
where
    T: Send,
{
    _event: &'e Event<T>,
    receiver: Option<oneshot::Receiver<T>>,
}

impl<T> ByRefEventReceiver<'_, T>
where
    T: Send,
{
    /// Receives a value from the event.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn recv(mut self) -> T {
        self.receiver.take().map_or_else(
            || unreachable!("receiver should always be Some when recv is called"),
            |receiver| {
                receiver.recv().unwrap_or_else(|_| {
                    // If the sender was dropped without sending, we wait forever
                    // as per the specification
                    loop {
                        std::thread::park();
                    }
                })
            },
        )
    }

    /// Receives a value from the event asynchronously.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    /// use futures::executor::block_on;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    ///
    /// sender.send(42);
    /// let value = block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub async fn recv_async(mut self) -> T {
        match self.receiver.take() {
            Some(receiver) => {
                // Use the oneshot receiver's native async support
                // The receiver implements Future directly, so we can await it
                match receiver.await {
                    Ok(value) => value,
                    Err(_) => {
                        // If the sender was dropped without sending, we wait forever as per spec
                        // This matches the behavior of the sync receive() method
                        loop {
                            std::future::pending::<()>().await;
                        }
                    }
                }
            }
            None => unreachable!("receiver should always be Some when recv_async is called"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::test_utils::with_watchdog;

    #[test]
    fn event_new_creates_valid_event() {
        with_watchdog(|| {
            let event = Event::<i32>::new();
            // Should be able to get endpoints once
            let (sender, receiver) = event.by_ref();
            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = Event::<String>::default();
            let (sender, receiver) = event.by_ref();
            sender.send("test".to_string());
            let value = receiver.recv();
            assert_eq!(value, "test");
        });
    }

    #[test]
    fn event_by_ref_method_provides_both() {
        with_watchdog(|| {
            let event = Event::<u64>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(123);
            let value = receiver.recv();
            assert_eq!(value, 123);
        });
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn event_by_ref_panics_on_second_call() {
        let event = Event::<i32>::new();
        let _endpoints = event.by_ref();
        let _endpoints2 = event.by_ref(); // Should panic
    }

    #[test]
    fn event_by_ref_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let endpoints1 = event.by_ref_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn event_send_succeeds_without_receiver() {
        let event = Event::<i32>::new();
        let (sender, _receiver) = event.by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn event_works_in_arc() {
        with_watchdog(|| {
            let event = Arc::new(Event::<String>::new());
            let (sender, receiver) = event.by_ref();

            sender.send("Hello from Arc".to_string());
            let value = receiver.recv();
            assert_eq!(value, "Hello from Arc");
        });
    }

    #[test]
    fn event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(Event::<String>::new());
            let (sender, receiver) = event.by_ref();

            sender.send("Hello from Rc".to_string());
            let value = receiver.recv();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn event_cross_thread_communication() {
        with_watchdog(|| {
            // For cross-thread usage, we need the Event to live long enough
            // In practice, this would typically be done with Arc<Event>
            static EVENT: std::sync::OnceLock<Event<String>> = std::sync::OnceLock::new();
            let event = EVENT.get_or_init(Event::<String>::new);
            let (sender, receiver) = event.by_ref();

            let sender_handle = thread::spawn(move || {
                sender.send("Hello from thread!".to_string());
            });

            let receiver_handle = thread::spawn(move || receiver.recv());

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap();
            assert_eq!(message, "Hello from thread!");
        });
    }

    #[test]
    fn thread_safe_types() {
        // Event should implement Send and Sync
        assert_impl_all!(Event<i32>: Send, Sync);
        // Sender should implement Send and Sync
        assert_impl_all!(ByRefEventSender<'_, i32>: Send, Sync);
        // Receiver should implement Send but not necessarily Sync (based on oneshot::Receiver)
        assert_impl_all!(ByRefEventReceiver<'_, i32>: Send);
    }

    #[test]
    fn event_receive_async_basic() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = Event::<i32>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_receive_async_cross_thread() {
        use futures::executor::block_on;

        with_watchdog(|| {
            static EVENT: std::sync::OnceLock<Event<String>> = std::sync::OnceLock::new();
            let event = EVENT.get_or_init(Event::<String>::new);
            let (sender, receiver) = event.by_ref();

            let sender_handle = thread::spawn(move || {
                // Add a small delay to ensure receiver is waiting
                thread::sleep(Duration::from_millis(10));
                sender.send("Hello async!".to_string());
            });

            let receiver_handle = thread::spawn(move || block_on(receiver.recv_async()));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap();
            assert_eq!(message, "Hello async!");
        });
    }

    #[test]
    fn event_receive_async_mixed_with_sync() {
        use futures::executor::block_on;

        with_watchdog(|| {
            // Test that sync and async can be used in the same test
            let event1 = Event::<i32>::new();
            let (sender1, receiver1) = event1.by_ref();

            let event2 = Event::<i32>::new();
            let (sender2, receiver2) = event2.by_ref();

            sender1.send(1);
            sender2.send(2);

            let value1 = receiver1.recv();
            let value2 = block_on(receiver2.recv_async());

            assert_eq!(value1, 1);
            assert_eq!(value2, 2);
        });
    }
}
