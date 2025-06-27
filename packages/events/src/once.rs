//! Events that can happen at most once (send/receive consumes the sender/receiver).
//!
//! This module provides one-time use events. Each event can only
//! be used once - after sending and receiving a value, the sender and receiver are consumed.
//!
//! # Usage Pattern
//!
//! 1. Create an instance of [`Event<T>`] (potentially inside an [`std::rc::Rc`])
//! 2. Call [`Event::sender()`] and [`Event::receiver()`] to get instances of each, or
//!    [`Event::endpoints()`] to get a tuple with both
//! 3. You can only do this once (panic on 2nd call; [`Event::sender_checked()`] is also
//!    supported, returning [`None`] on 2nd call instead)
//! 4. Use [`ByRefEventSender`]/[`ByRefEventReceiver`] as desired, either dropping them or
//!    consuming them via self-taking methods
//!
//! # Example
//!
//! ```rust
//! use events::once::Event;
//!
//! let event = Event::<i32>::new();
//! let (sender, receiver) = event.endpoints();
//!
//! sender.send(42);
//! let value = receiver.receive();
//! assert_eq!(value, 42);
//! ```

use std::cell::RefCell;
use std::marker::PhantomData;

/// A one-time event that can send and receive a value of type `T`.
///
/// The event can only be used once - after obtaining the sender and receiver,
/// subsequent calls to obtain them will panic (or return [`None`] for the checked variants).
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
pub struct Event<T> {
    channel: RefCell<Option<(oneshot::Sender<T>, oneshot::Receiver<T>)>>,
    _single_threaded: PhantomData<*const ()>,
}

impl<T> Event<T> {
    /// Creates a new event.
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
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let sender = event.sender();
    /// ```
    pub fn sender(&self) -> ByRefEventSender<'_, T> {
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
    pub fn receiver(&self) -> ByRefEventReceiver<'_, T> {
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
    pub fn endpoints(&self) -> (ByRefEventSender<'_, T>, ByRefEventReceiver<'_, T>) {
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
    pub fn sender_checked(&self) -> Option<ByRefEventSender<'_, T>> {
        let (sender, _) = self.take_channel()?;
        Some(ByRefEventSender {
            sender: Some(sender),
            _lifetime: PhantomData,
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
    pub fn receiver_checked(&self) -> Option<ByRefEventReceiver<'_, T>> {
        let (_, receiver) = self.take_channel()?;
        Some(ByRefEventReceiver {
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
    /// use events::once::Event;
    ///
    /// let event = Event::<i32>::new();
    /// let endpoints = event.endpoints_checked().unwrap();
    /// let endpoints2 = event.endpoints_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn endpoints_checked(
        &self,
    ) -> Option<(ByRefEventSender<'_, T>, ByRefEventReceiver<'_, T>)> {
        let (sender, receiver) = self.take_channel()?;
        Some((
            ByRefEventSender {
                sender: Some(sender),
                _lifetime: PhantomData,
            },
            ByRefEventReceiver {
                receiver: Some(receiver),
                _lifetime: PhantomData,
            },
        ))
    }

    fn take_channel(&self) -> Option<(oneshot::Sender<T>, oneshot::Receiver<T>)> {
        self.channel.borrow_mut().take()
    }
}

impl<T> Default for Event<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A sender that can send a value through an event.
///
/// The sender holds a reference to the event and can only be used once.
/// After calling [`send`](ByRefEventSender::send), the sender is consumed.
#[derive(Debug)]
pub struct ByRefEventSender<'e, T> {
    sender: Option<oneshot::Sender<T>>,
    _lifetime: PhantomData<&'e ()>,
}

impl<T> ByRefEventSender<'_, T> {
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

/// A receiver that can receive a value from an event.
///
/// The receiver holds a reference to the event and can only be used once.
/// After calling [`receive`](ByRefEventReceiver::receive), the receiver is consumed.
#[derive(Debug)]
pub struct ByRefEventReceiver<'e, T> {
    receiver: Option<oneshot::Receiver<T>>,
    _lifetime: PhantomData<&'e ()>,
}

impl<T> ByRefEventReceiver<'_, T> {
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

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;
    use std::time::Duration;

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
    fn endpoints_method_provides_both() {
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
    fn sender_panics_after_endpoints_retrieved() {
        let event = Event::<i32>::new();
        let _endpoints = event.endpoints();
        let _sender = event.sender(); // Should panic
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn receiver_panics_after_endpoints_retrieved() {
        let event = Event::<i32>::new();
        let _endpoints = event.endpoints();
        let _receiver = event.receiver(); // Should panic
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn endpoints_panics_after_sender_retrieved() {
        let event = Event::<i32>::new();
        let _sender = event.sender();
        let _endpoints = event.endpoints(); // Should panic
    }

    #[test]
    fn sender_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let sender1 = event.sender_checked();
        assert!(sender1.is_some());

        let sender2 = event.sender_checked();
        assert!(sender2.is_none());
    }

    #[test]
    fn receiver_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let receiver1 = event.receiver_checked();
        assert!(receiver1.is_some());

        let receiver2 = event.receiver_checked();
        assert!(receiver2.is_none());
    }

    #[test]
    fn endpoints_checked_returns_none_after_use() {
        let event = Event::<i32>::new();
        let endpoints1 = event.endpoints_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.endpoints_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn send_succeeds_without_receiver() {
        let event = Event::<i32>::new();
        let sender = event.sender();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn single_threaded_types() {
        // Event should not implement Send or Sync due to RefCell and PhantomData<*const ()>
        assert_not_impl_any!(Event<i32>: Send, Sync);
    }
}
