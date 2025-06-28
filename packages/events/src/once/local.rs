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
/// let (sender, receiver) = event.by_ref();
///
/// sender.send("Hello".to_string());
/// let message = receiver.recv();
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

    /// Returns both the sender and receiver for this event, connected by reference.
    ///
    /// # Panics
    ///
    /// Panics if this method or [`by_ref_checked`](LocalEvent::by_ref_checked) has been called before.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let (sender, receiver) = event.by_ref();
    /// ```
    pub fn by_ref(&self) -> (ByRefLocalEventSender<'_, T>, ByRefLocalEventReceiver<'_, T>) {
        self.by_ref_checked()
            .expect("Event endpoints have already been retrieved")
    }

    /// Returns both the sender and receiver for this event, connected by reference,
    /// or [`None`] if endpoints have already been retrieved.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEvent;
    ///
    /// let event = LocalEvent::<i32>::new();
    /// let endpoints = event.by_ref_checked().unwrap();
    /// let endpoints2 = event.by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    pub fn by_ref_checked(
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

/// A receiver that can receive a value from a single-threaded event.
///
/// The receiver holds a reference to the event and can only be used once.
/// After calling [`recv`](ByRefLocalEventReceiver::recv), the receiver is consumed.
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
    /// use events::once::LocalEvent;
    /// use futures::executor::block_on;
    ///
    /// let event = LocalEvent::<i32>::new();
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

    use static_assertions::assert_not_impl_any;
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn local_event_new_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalEvent::<i32>::new();
            // Should be able to get endpoints once
            let (sender, receiver) = event.by_ref();
            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn local_event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalEvent::<String>::default();
            let (sender, receiver) = event.by_ref();
            sender.send("test".to_string());
            let value = receiver.recv();
            assert_eq!(value, "test");
        });
    }

    #[test]
    fn local_event_by_ref_method_provides_both() {
        with_watchdog(|| {
            let event = LocalEvent::<u64>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(123);
            let value = receiver.recv();
            assert_eq!(value, 123);
        });
    }

    #[test]
    #[should_panic(expected = "Event endpoints have already been retrieved")]
    fn local_event_by_ref_panics_on_second_call() {
        let event = LocalEvent::<i32>::new();
        let _endpoints = event.by_ref();
        let _endpoints2 = event.by_ref(); // Should panic
    }

    #[test]
    fn local_event_by_ref_checked_returns_none_after_use() {
        let event = LocalEvent::<i32>::new();
        let endpoints1 = event.by_ref_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn local_event_send_succeeds_without_receiver() {
        let event = LocalEvent::<i32>::new();
        let (sender, _receiver) = event.by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn local_event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(LocalEvent::<String>::new());
            let (sender, receiver) = event.by_ref();

            sender.send("Hello from Rc".to_string());
            let value = receiver.recv();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn single_threaded_types() {
        // LocalEvent should not implement Send or Sync due to RefCell and PhantomData<*const ()>
        assert_not_impl_any!(LocalEvent<i32>: Send, Sync);
    }

    #[test]
    fn local_event_receive_async_basic() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = LocalEvent::<i32>::new();
            let (sender, receiver) = event.by_ref();

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn local_event_receive_async_mixed_with_sync() {
        use futures::executor::block_on;

        with_watchdog(|| {
            // Test that sync and async can be used in the same test
            let event1 = LocalEvent::<i32>::new();
            let (sender1, receiver1) = event1.by_ref();

            let event2 = LocalEvent::<i32>::new();
            let (sender2, receiver2) = event2.by_ref();

            sender1.send(1);
            sender2.send(2);

            let value1 = receiver1.recv();
            let value2 = block_on(receiver2.recv_async());

            assert_eq!(value1, 1);
            assert_eq!(value2, 2);
        });
    }

    #[test]
    fn local_event_receive_async_string() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let event = LocalEvent::<String>::new();
            let (sender, receiver) = event.by_ref();

            sender.send("Hello async world!".to_string());
            let message = block_on(receiver.recv_async());
            assert_eq!(message, "Hello async world!");
        });
    }
}
