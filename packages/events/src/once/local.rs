//! Single-threaded one-time events.
//!
//! This module provides single-threaded event types that have lower overhead
//! but cannot be shared across threads.

use std::cell::RefCell;
use std::marker::PhantomData;

/// A one-time event that can send and receive a value of type `T` (single-threaded variant).
///
/// This is the single-threaded variant that has lower overhead but cannot be shared across threads.
/// The event can only be used once - after obtaining the sender and receiver,
/// subsequent calls to obtain them will panic (or return [`None`] for the checked variants).
///
/// For thread-safe usage, see [`crate::once::Event`] which can be used with `Arc<T>`.
///
/// # Example
///
/// ```rust
/// use events::once::LocalEvent;
///
/// let event = LocalEvent::<String>::new();
/// let (sender, receiver) = event.endpoints();
///
/// sender.send("Hello".to_string());
/// let message = receiver.receive();
/// assert_eq!(message, "Hello");
/// ```
#[derive(Debug)]
pub struct LocalEvent<T> {
    channel: RefCell<Option<(oneshot::Sender<T>, oneshot::Receiver<T>)>>,
    _single_threaded: PhantomData<*const ()>,
}

impl<T> LocalEvent<T> {
    /// Creates a new single-threaded event.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        let (sender, receiver) = oneshot::channel();
        Self {
            channel: RefCell::new(Some((sender, receiver))),
            _single_threaded: PhantomData,
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
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let sender = event.sender();
    /// ```
    pub fn sender(&self) -> ByRefLocalEventSender<'_, T> {
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
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let receiver = event.receiver();
    /// ```
    pub fn receiver(&self) -> ByRefLocalEventReceiver<'_, T> {
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
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let (sender, receiver) = event.endpoints();
    /// ```
    pub fn endpoints(&self) -> (ByRefLocalEventSender<'_, T>, ByRefLocalEventReceiver<'_, T>) {
        self.endpoints_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns a sender for this event, or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let sender = event.sender_checked().unwrap();
    /// let sender2 = event.sender_checked(); // Returns None
    /// assert!(sender2.is_none());
    /// ```
    pub fn sender_checked(&self) -> Option<ByRefLocalEventSender<'_, T>> {
        let (sender, _) = self.take_channel()?;
        Some(ByRefLocalEventSender {
            sender: Some(sender),
            _lifetime: PhantomData,
        })
    }

    /// Returns a receiver for this event, or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let receiver = event.receiver_checked().unwrap();
    /// let receiver2 = event.receiver_checked(); // Returns None
    /// assert!(receiver2.is_none());
    /// ```
    pub fn receiver_checked(&self) -> Option<ByRefLocalEventReceiver<'_, T>> {
        let (_, receiver) = self.take_channel()?;
        Some(ByRefLocalEventReceiver {
            receiver: Some(receiver),
            _lifetime: PhantomData,
        })
    }

    /// Returns both the sender and receiver for this event, or [`None`] if endpoints have
    /// already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let endpoints = event.endpoints_checked().unwrap();
    /// let endpoints2 = event.endpoints_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn endpoints_checked(
        &self,
    ) -> Option<(ByRefLocalEventSender<'_, T>, ByRefLocalEventReceiver<'_, T>)> {
        let (sender, receiver) = self.take_channel()?;
        Some((
            ByRefLocalEventSender {
                sender: Some(sender),
                _lifetime: PhantomData,
            },
            ByRefLocalEventReceiver {
                receiver: Some(receiver),
                _lifetime: PhantomData,
            },
        ))
    }

    fn take_channel(&self) -> Option<(oneshot::Sender<T>, oneshot::Receiver<T>)> {
        self.channel.borrow_mut().take()
    }
}

impl<T> Default for LocalEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A sender that can send a value through a single-threaded event.
///
/// The sender holds a reference to the event and can only be used once.
/// After calling [`send`](ByRefLocalEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefLocalEventSender<'e, T> {
    sender: Option<oneshot::Sender<T>>,
    _lifetime: PhantomData<&'e ()>,
}

impl<T> ByRefLocalEventSender<'_, T> {
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
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

/// A receiver that can receive a value from a single-threaded event.
///
/// The receiver holds a reference to the event and can only be used once.
/// After calling [`receive`](ByRefLocalEventReceiver::receive), the receiver is consumed.
#[derive(Debug)]
pub struct ByRefLocalEventReceiver<'e, T> {
    receiver: Option<oneshot::Receiver<T>>,
    _lifetime: PhantomData<&'e ()>,
}

impl<T> ByRefLocalEventReceiver<'_, T> {
    /// Receives a value from the event.
    ///
    /// This method consumes the receiver and waits for a sender to send a value.
    /// If the sender is dropped without sending, this method will wait forever.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
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

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::time::Duration;

    use static_assertions::assert_not_impl_any;

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
    fn local_event_new_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalEvent::<i32>::new();
            // Should be able to get endpoints once
            let (sender, receiver) = event.endpoints();
            sender.send(42);
            let value = receiver.receive();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn local_event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalEvent::<String>::default();
            let (sender, receiver) = event.endpoints();
            sender.send("test".to_string());
            let value = receiver.receive();
            assert_eq!(value, "test");
        });
    }

    #[test]
    fn local_event_endpoints_method_provides_both() {
        with_watchdog(|| {
            let event = LocalEvent::<u64>::new();
            let (sender, receiver) = event.endpoints();

            sender.send(123);
            let value = receiver.receive();
            assert_eq!(value, 123);
        });
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn local_event_sender_panics_after_endpoints_retrieved() {
        let event = LocalEvent::<i32>::new();
        let _endpoints = event.endpoints();
        let _sender = event.sender(); // Should panic
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn local_event_receiver_panics_after_endpoints_retrieved() {
        let event = LocalEvent::<i32>::new();
        let _endpoints = event.endpoints();
        let _receiver = event.receiver(); // Should panic
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn local_event_endpoints_panics_after_sender_retrieved() {
        let event = LocalEvent::<i32>::new();
        let _sender = event.sender();
        let _endpoints = event.endpoints(); // Should panic
    }

    #[test]
    fn local_event_sender_checked_returns_none_after_use() {
        let event = LocalEvent::<i32>::new();
        let sender1 = event.sender_checked();
        assert!(sender1.is_some());

        let sender2 = event.sender_checked();
        assert!(sender2.is_none());
    }

    #[test]
    fn local_event_receiver_checked_returns_none_after_use() {
        let event = LocalEvent::<i32>::new();
        let receiver1 = event.receiver_checked();
        assert!(receiver1.is_some());

        let receiver2 = event.receiver_checked();
        assert!(receiver2.is_none());
    }

    #[test]
    fn local_event_endpoints_checked_returns_none_after_use() {
        let event = LocalEvent::<i32>::new();
        let endpoints1 = event.endpoints_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.endpoints_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn local_event_send_succeeds_without_receiver() {
        let event = LocalEvent::<i32>::new();
        let sender = event.sender();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn local_event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(LocalEvent::<String>::new());
            let (sender, receiver) = event.endpoints();

            sender.send("Hello from Rc".to_string());
            let value = receiver.receive();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn single_threaded_types() {
        // LocalEvent should not implement Send or Sync due to RefCell and PhantomData<*const ()>
        assert_not_impl_any!(LocalEvent<i32>: Send, Sync);
    }
}
