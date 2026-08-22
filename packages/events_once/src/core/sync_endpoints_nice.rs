//! This simply wraps the core endpoints with a nicer API surface that eliminates
//! the outer generic type parameter, leaving only the inner T of the payload.

use std::any::type_name;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::Poll;
use std::{fmt, task};

use crate::{BoxedRef, Disconnected, IntoValueError, PtrRef, ReceiverCore, SenderCore};

/// Delivers a single value to the receiver connected to the same event.
///
/// This kind of endpoint is used for boxed events, which are heap-allocated and automatically
/// destroyed when both the sender and receiver are dropped.
pub struct BoxedSender<T: Send + 'static> {
    inner: SenderCore<BoxedRef<T>, T>,
}

// Senders are one-shot and consumed on use. The UnsafeCell fields in the
// underlying event are guarded by an atomic state machine that prevents
// observing inconsistent state during unwind.
impl<T: Send + 'static> UnwindSafe for BoxedSender<T> {}
impl<T: Send + 'static> RefUnwindSafe for BoxedSender<T> {}

impl<T: Send + 'static> BoxedSender<T> {
    pub(crate) fn new(inner: SenderCore<BoxedRef<T>, T>) -> Self {
        Self { inner }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    pub fn send(self, value: T) {
        self.inner.send(value);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for BoxedSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

/// Receives a single value from the sender connected to the same event.
///
/// Awaiting the receiver will yield either the payload of type `T` or a [`Disconnected`] error.
///
/// This kind of endpoint is used for boxed events, which are heap-allocated and automatically
/// destroyed when both the sender and receiver are dropped.
///
/// # Reentrancy
///
/// Cloning a waker during polling may synchronously send through or drop the sender. Waking or
/// dropping a registered waker during completion or cancellation may synchronously poll this
/// receiver to completion or drop an endpoint. The event publishes the resulting state before
/// each callback.
pub struct BoxedReceiver<T: Send + 'static> {
    inner: ReceiverCore<BoxedRef<T>, T>,
}

// Receivers are one-shot. The UnsafeCell fields in the underlying event
// are guarded by an atomic state machine that prevents observing
// inconsistent state during unwind.
impl<T: Send + 'static> UnwindSafe for BoxedReceiver<T> {}
impl<T: Send + 'static> RefUnwindSafe for BoxedReceiver<T> {}

impl<T: Send + 'static> BoxedReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<BoxedRef<T>, T>) -> Self {
        Self { inner }
    }

    /// Checks whether the receiver has reached a terminal state: either a value has been sent
    /// or the sender has disconnected.
    ///
    /// Valid to call only before `Future::poll` has returned `Ready`, whether that completion
    /// is a successful receive or a disconnection.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }

    /// Consumes the receiver and returns the received value if it is already available.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available result without blocking. It returns `Ok(value)` once a value
    /// has been sent, `Err(IntoValueError::Disconnected)` once the sender has disconnected
    /// without sending a value, and otherwise `Err(IntoValueError::Pending(self))`, returning
    /// the receiver so the caller can try again later.
    ///
    /// Valid to call only before `Future::poll` has returned `Ready`, whether that completion
    /// is a successful receive or a disconnection.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use events_once::{Event, IntoValueError};
    ///
    /// fn main() {
    ///     let (sender, receiver) = Event::<String>::boxed();
    ///
    ///     // into_value() is for synchronous checks: it never blocks or requires polling.
    ///     // Before a value is sent, it returns the receiver so the caller can try again later.
    ///     let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
    ///         panic!("expected the receiver to still be waiting for a value");
    ///     };
    ///
    ///     sender.send("Hello, world!".to_string());
    ///
    ///     let message = receiver.into_value().unwrap();
    ///     println!("Received message: {message}");
    /// }
    /// ```
    pub fn into_value(self) -> Result<T, IntoValueError<Self>> {
        match self.inner.into_value() {
            Ok(value) => Ok(value),
            Err(IntoValueError::Pending(inner)) => Err(IntoValueError::Pending(Self { inner })),
            Err(IntoValueError::Disconnected) => Err(IntoValueError::Disconnected),
        }
    }
}

impl<T: Send + 'static> Future for BoxedReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        Pin::new(&mut this.inner).poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for BoxedReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

/// Delivers a single value to the receiver connected to the same event.
///
/// This kind of endpoint is used with events for which the storage is provided by the
/// owner of the endpoint. They are also responsible for ensuring that the event that
/// connects the sender-receiver pair outlives both endpoints.
pub struct RawSender<T: Send + 'static> {
    inner: SenderCore<PtrRef<T>, T>,
}

// See BoxedSender for justification.
impl<T: Send + 'static> UnwindSafe for RawSender<T> {}
impl<T: Send + 'static> RefUnwindSafe for RawSender<T> {}

impl<T: Send + 'static> RawSender<T> {
    pub(crate) fn new(inner: SenderCore<PtrRef<T>, T>) -> Self {
        Self { inner }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    pub fn send(self, value: T) {
        self.inner.send(value);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for RawSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

/// Receives a single value from the sender connected to the same event.
///
/// Awaiting the receiver will yield either the payload of type `T` or a [`Disconnected`] error.
///
/// This kind of endpoint is used with events for which the storage is provided by the
/// owner of the endpoint. They are also responsible for ensuring that the event that
/// connects the sender-receiver pair outlives both endpoints.
///
/// # Reentrancy
///
/// Cloning a waker during polling may synchronously send through or drop the sender. Waking or
/// dropping a registered waker during completion or cancellation may synchronously poll this
/// receiver to completion or drop an endpoint. The event publishes the resulting state before
/// each callback.
pub struct RawReceiver<T: Send + 'static> {
    inner: ReceiverCore<PtrRef<T>, T>,
}

// See BoxedReceiver for justification.
impl<T: Send + 'static> UnwindSafe for RawReceiver<T> {}
impl<T: Send + 'static> RefUnwindSafe for RawReceiver<T> {}

impl<T: Send + 'static> RawReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<PtrRef<T>, T>) -> Self {
        Self { inner }
    }

    /// Checks whether the receiver has reached a terminal state: either a value has been sent
    /// or the sender has disconnected.
    ///
    /// Valid to call only before `Future::poll` has returned `Ready`, whether that completion
    /// is a successful receive or a disconnection.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }

    /// Consumes the receiver and returns the received value if it is already available.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available result without blocking. It returns `Ok(value)` once a value
    /// has been sent, `Err(IntoValueError::Disconnected)` once the sender has disconnected
    /// without sending a value, and otherwise `Err(IntoValueError::Pending(self))`, returning
    /// the receiver so the caller can try again later.
    ///
    /// Valid to call only before `Future::poll` has returned `Ready`, whether that completion
    /// is a successful receive or a disconnection.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use events_once::{EmbeddedEvent, Event, IntoValueError};
    ///
    /// fn main() {
    ///     let mut place = Box::pin(EmbeddedEvent::<String>::new());
    ///
    ///     // SAFETY: `place` was just constructed, so it is not in use by another event; it
    ///     // stays pinned and writable for as long as the `Box` lives, which outlasts `sender`
    ///     // and `receiver` below, both of which are consumed before `place` is dropped.
    ///     let (sender, receiver) = unsafe { Event::placed(place.as_mut()) };
    ///
    ///     // into_value() is for synchronous checks: it never blocks or requires polling.
    ///     // Before a value is sent, it returns the receiver so the caller can try again later.
    ///     let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
    ///         panic!("expected the receiver to still be waiting for a value");
    ///     };
    ///
    ///     sender.send("Hello, world!".to_string());
    ///
    ///     let message = receiver.into_value().unwrap();
    ///     println!("Received message: {message}");
    /// }
    /// ```
    pub fn into_value(self) -> Result<T, IntoValueError<Self>> {
        match self.inner.into_value() {
            Ok(value) => Ok(value),
            Err(IntoValueError::Pending(inner)) => Err(IntoValueError::Pending(Self { inner })),
            Err(IntoValueError::Disconnected) => Err(IntoValueError::Disconnected),
        }
    }
}

impl<T: Send + 'static> Future for RawReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        Pin::new(&mut this.inner).poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for RawReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(BoxedSender<u32>: Send);
    assert_not_impl_any!(BoxedSender<u32>: Sync);
    assert_impl_all!(BoxedReceiver<u32>: Send);
    assert_not_impl_any!(BoxedReceiver<u32>: Sync);
    assert_impl_all!(RawSender<u32>: Send);
    assert_not_impl_any!(RawSender<u32>: Sync);
    assert_impl_all!(RawReceiver<u32>: Send);
    assert_not_impl_any!(RawReceiver<u32>: Sync);

    // Trait object payloads must preserve Send (regression test for #142).
    assert_impl_all!(BoxedSender<Box<dyn Send>>: Send);
    assert_impl_all!(BoxedReceiver<Box<dyn Send>>: Send);

    // Verify that an async block awaiting a trait-object receiver is Send. Static assertions
    // alone do not catch the bug because it only manifests in async generator analysis.
    fn _assert_future_send<F: Future + Send>(_: F) {}

    fn _boxed_receiver_trait_object_future_is_send() {
        let (_tx, rx) = crate::Event::<Box<dyn Send>>::boxed();

        _assert_future_send(async move {
            drop(rx.await);
        });
    }

    assert_impl_all!(
        BoxedSender<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        BoxedReceiver<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        RawSender<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        RawReceiver<u32>: UnwindSafe, RefUnwindSafe
    );
}
