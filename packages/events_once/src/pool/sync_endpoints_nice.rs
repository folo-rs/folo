//! This simply wraps the core endpoints with a nicer API surface that eliminates
//! the outer generic type parameter, leaving only the inner T of the payload.

use std::any::type_name;
use std::pin::Pin;
use std::task::Poll;
use std::{fmt, task};

use crate::{Disconnected, IntoValueError, PooledRef, ReceiverCore, SenderCore};
/// Delivers a single value to the receiver connected to the same event.
///
/// This kind of endpoint is used for events stored in an event pool or event lake.
pub struct PooledSender<T: Send + 'static> {
    inner: SenderCore<PooledRef<T>, T>,
}

impl<T: Send + 'static> PooledSender<T> {
    pub(crate) fn new(inner: SenderCore<PooledRef<T>, T>) -> Self {
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
impl<T: Send + 'static> fmt::Debug for PooledSender<T> {
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
/// This kind of endpoint is used for events stored in an event pool or event lake.
pub struct PooledReceiver<T: Send + 'static> {
    inner: ReceiverCore<PooledRef<T>, T>,
}

impl<T: Send + 'static> PooledReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<PooledRef<T>, T>) -> Self {
        Self { inner }
    }

    /// Checks whether a value is ready to be received.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.inner.is_ready()
    }

    /// Consumes the receiver and transforms it into the received value, if the value is available.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available value without blocking. It returns `Ok(value)` if a value has
    /// already been sent, or returns the receiver if no value is currently available.
    ///
    /// # Panics
    ///
    /// Panics if the value has already been received via `Future::poll()`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use events_once::{Event, IntoValueError};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let (sender, receiver) = Event::<String>::boxed();
    ///
    ///     // into_value() is designed for synchronous scenarios where you do not want to wait but
    ///     // simply want to either obtain the received value or do nothing. First, we do nothing.
    ///     //
    ///     // If no value has been sent yet, into_value() returns Err(IntoValueError::Pending(self)).
    ///     let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
    ///         panic!(
    ///             "Expected receiver to indicate that it is still waiting for a payload to be sent."
    ///         );
    ///     };
    ///
    ///     sender.send("Hello, world!".to_string());
    ///
    ///     let message = receiver.into_value().unwrap();
    ///
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

impl<T: Send + 'static> Future for PooledReceiver<T> {
    type Output = Result<T, Disconnected>;

    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We never move out of `self`, only access its inner field.
        let inner = unsafe { self.map_unchecked_mut(|x| &mut x.inner) };

        inner.poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for PooledReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{EventPool, IntoValueError};

    assert_impl_all!(PooledSender<u32>: Send);
    assert_not_impl_any!(PooledSender<u32>: Sync);

    assert_impl_all!(PooledReceiver<u32>: Send);
    assert_not_impl_any!(PooledReceiver<u32>: Sync);

    #[test]
    fn into_value_disconnected() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        drop(sender);

        assert!(matches!(
            receiver.into_value(),
            Err(IntoValueError::Disconnected)
        ));
    }
}
