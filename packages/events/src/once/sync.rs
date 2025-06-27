//! Thread-safe one-time events.
//!
//! This module provides thread-safe event types that can be shared across threads
//! and used for cross-thread communication.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A one-time event that can send and receive a value of type `T`.
///
/// This is the thread-safe variant that can create sender and receiver endpoints
/// that can be used across threads. The event itself doesn't need to be shared
/// across threads - only the sender and receiver do.
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
///
/// # Example
///
/// ```rust
/// use events::once::Event;
///
/// let event = Event::<String>::new();
/// let (sender, receiver) = event.endpoints();
///
/// sender.send("Hello".to_string());
/// let message = receiver.receive();
/// assert_eq!(message, "Hello");
/// ```
#[derive(Debug)]
pub struct Event<T>
where
    T: Send,
{
    used: AtomicBool,
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

    /// Returns a sender for this event.
    ///
    /// # Panics
    ///
    /// Panics if this method or any other endpoint retrieval method has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let sender = event.sender();
    /// ```
    pub fn sender(&self) -> EventSender<T> {
        self.sender_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns a receiver for this event.
    ///
    /// # Panics
    ///
    /// Panics if this method or any other endpoint retrieval method has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let receiver = event.receiver();
    /// ```
    pub fn receiver(&self) -> EventReceiver<T> {
        self.receiver_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event.
    ///
    /// # Panics
    ///
    /// Panics if this method or any other endpoint retrieval method has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let (sender, receiver) = event.endpoints();
    /// ```
    pub fn endpoints(&self) -> (EventSender<T>, EventReceiver<T>) {
        self.endpoints_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns a sender for this event, or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let sender = event.sender_checked().unwrap();
    /// let sender2 = event.sender_checked(); // Returns None
    /// assert!(sender2.is_none());
    /// ```
    pub fn sender_checked(&self) -> Option<EventSender<T>> {
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        let (sender, _receiver) = self.channel.lock().unwrap().take()?;
        Some(EventSender {
            sender: Some(sender),
        })
    }

    /// Returns a receiver for this event, or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let receiver = event.receiver_checked().unwrap();
    /// let receiver2 = event.receiver_checked(); // Returns None
    /// assert!(receiver2.is_none());
    /// ```
    pub fn receiver_checked(&self) -> Option<EventReceiver<T>> {
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        let (_sender, receiver) = self.channel.lock().unwrap().take()?;
        Some(EventReceiver {
            receiver: Some(receiver),
        })
    }

    /// Returns both the sender and receiver for this event, or [`None`] if endpoints have
    /// already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let endpoints = event.endpoints_checked().unwrap();
    /// let endpoints2 = event.endpoints_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn endpoints_checked(&self) -> Option<(EventSender<T>, EventReceiver<T>)> {
        if self.used.swap(true, Ordering::SeqCst) {
            return None;
        }

        let (sender, receiver) = self.channel.lock().unwrap().take()?;
        Some((
            EventSender {
                sender: Some(sender),
            },
            EventReceiver {
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
/// The sender owns the underlying channel and can be moved across threads.
/// After calling [`send`](EventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct EventSender<T>
where
    T: Send,
{
    sender: Option<oneshot::Sender<T>>,
}

impl<T> EventSender<T>
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
    /// let sender = event.sender();
    /// sender.send(42);
    /// ```
    pub fn send(mut self, value: T) {
        if let Some(sender) = self.sender.take() {
            // We don't care if the receiver is dropped - sending always succeeds
            drop(sender.send(value));
        }
    }
}

// SAFETY: EventSender can be sent across threads when T: Send, which is already required
unsafe impl<T: Send> Send for EventSender<T> {}
// SAFETY: EventSender can be shared across threads when T: Send
unsafe impl<T: Send> Sync for EventSender<T> {}

/// A receiver that can receive a value from a thread-safe event.
///
/// The receiver owns the underlying channel and can be moved across threads.
/// After calling [`receive`](EventReceiver::receive), the receiver is consumed.
#[derive(Debug)]
pub struct EventReceiver<T>
where
    T: Send,
{
    receiver: Option<oneshot::Receiver<T>>,
}

impl<T> EventReceiver<T>
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
    /// let (sender, receiver) = event.endpoints();
    ///
    /// sender.send(42);
    /// let value = receiver.receive();
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn receive(mut self) -> T {
        self.receiver.take().map_or_else(
            || unreachable!("receiver should always be Some when receive is called"),
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
}

// SAFETY: EventReceiver can be sent across threads when T: Send, which is already required
unsafe impl<T: Send> Send for EventReceiver<T> {}
// SAFETY: EventReceiver can be shared across threads when T: Send
unsafe impl<T: Send> Sync for EventReceiver<T> {}

// Keep the old names for backwards compatibility
/// A sender that can send a value through a thread-safe event.
///
/// **Deprecated:** Use [`EventSender`] instead. This type alias is kept for backwards compatibility.
pub type ByRefEventSender<'e, T> = EventSender<T>;

/// A receiver that can receive a value from a thread-safe event.
///
/// **Deprecated:** Use [`EventReceiver`] instead. This type alias is kept for backwards compatibility.
pub type ByRefEventReceiver<'e, T> = EventReceiver<T>;

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use static_assertions::assert_impl_all;

    use super::*;

    /// Runs a test with a 10-second timeout to prevent infinite hangs.
    /// If the test does not complete within 10 seconds, the function will panic.
    fn with_watchdog<F, R>(test_fn: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        use std::sync::mpsc;
        use std::thread;

        let (tx, rx) = mpsc::channel();

        // Run the test in a separate thread
        let test_handle = thread::spawn(move || {
            let result = test_fn();
            // Send the result back - if this fails, the receiver has timed out
            drop(tx.send(result));
        });

        // Wait for either the test to complete or timeout
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => {
                // Test completed successfully, join the thread to clean up
                test_handle.join().expect("Test thread should not panic");
                result
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Test timed out - this indicates the test is hanging
                panic!("Test exceeded 10-second timeout - likely hanging in receive()");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Thread panicked, join it to get the panic
                match test_handle.join() {
                    Ok(()) => panic!("Test thread disconnected unexpectedly"),
                    Err(e) => std::panic::resume_unwind(e),
                }
            }
        }
    }

    #[test]
    fn event_new_creates_valid_event() {
        with_watchdog(|| {
            let event = Event::<i32>::new();
            // Should be able to get endpoints once
            let (sender, receiver) = event.endpoints();
            sender.send(42);
            let value = receiver.receive();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = Event::<String>::default();
            let (sender, receiver) = event.endpoints();
            sender.send("test".to_string());
            let value = receiver.receive();
            assert_eq!(value, "test");
        });
    }

    #[test]
    fn event_endpoints_method_provides_both() {
        with_watchdog(|| {
            let event = Event::<u64>::new();
            let (sender, receiver) = event.endpoints();

            sender.send(123);
            let value = receiver.receive();
            assert_eq!(value, 123);
        });
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn event_sender_panics_after_endpoints_retrieved() {
        let event = Event::<i32>::new();
        let _endpoints = event.endpoints();
        let _sender = event.sender(); // Should panic
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn event_receiver_panics_after_endpoints_retrieved() {
        let event = Event::<i32>::new();
        let _endpoints = event.endpoints();
        let _receiver = event.receiver(); // Should panic
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn event_endpoints_panics_after_sender_retrieved() {
        let event = Event::<i32>::new();
        let _sender = event.sender();
        let _endpoints = event.endpoints(); // Should panic
    }

    #[test]
    fn event_sender_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let sender1 = event.sender_checked();
        assert!(sender1.is_some());

        let sender2 = event.sender_checked();
        assert!(sender2.is_none());
    }

    #[test]
    fn event_receiver_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let receiver1 = event.receiver_checked();
        assert!(receiver1.is_some());

        let receiver2 = event.receiver_checked();
        assert!(receiver2.is_none());
    }

    #[test]
    fn event_endpoints_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let endpoints1 = event.endpoints_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.endpoints_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn event_send_succeeds_without_receiver() {
        let event = Event::<i32>::new();
        let sender = event.sender();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn event_works_in_arc() {
        with_watchdog(|| {
            let event = Arc::new(Event::<String>::new());
            let (sender, receiver) = event.endpoints();

            sender.send("Hello from Arc".to_string());
            let value = receiver.receive();
            assert_eq!(value, "Hello from Arc");
        });
    }

    #[test]
    fn event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(Event::<String>::new());
            let (sender, receiver) = event.endpoints();

            sender.send("Hello from Rc".to_string());
            let value = receiver.receive();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn event_cross_thread_communication() {
        with_watchdog(|| {
            let event = Event::<String>::new();
            let (sender, receiver) = event.endpoints();

            let sender_handle = thread::spawn(move || {
                sender.send("Hello from thread!".to_string());
            });

            let receiver_handle = thread::spawn(move || receiver.receive());

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap();
            assert_eq!(message, "Hello from thread!");
        });
    }

    #[test]
    fn thread_safe_types() {
        // Event should implement Send and Sync
        assert_impl_all!(Event<i32>: Send, Sync);
        // Sender and Receiver should implement Send and Sync
        assert_impl_all!(EventSender<i32>: Send, Sync);
        assert_impl_all!(EventReceiver<i32>: Send, Sync);
    }
}
