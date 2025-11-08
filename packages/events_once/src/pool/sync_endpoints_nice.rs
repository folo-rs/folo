//! This simply wraps the core endpoints with a nicer API surface that eliminates
//! the outer generic type parameter, leaving only the inner T of the payload.

use std::any::type_name;
use std::pin::Pin;
use std::task::Poll;
use std::{fmt, task};

use crate::{Disconnected, PooledRef, ReceiverCore, SenderCore};
/// Delivers a single value to the receiver connected to the same event.
///
/// This kind of endpoint is used for events stored in an event pool or event lake.
pub struct PooledSender<T: Send> {
    inner: SenderCore<PooledRef<T>, T>,
}

impl<T: Send> PooledSender<T> {
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

impl<T: Send> fmt::Debug for PooledSender<T> {
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
pub struct PooledReceiver<T: Send> {
    inner: ReceiverCore<PooledRef<T>, T>,
}

impl<T: Send> PooledReceiver<T> {
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
    /// use events_once::{Disconnected, EventPool};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let pool = EventPool::<String>::new();
    /// let (sender, receiver) = pool.rent();
    ///
    /// let receiver = match receiver.into_value() {
    ///     Ok(result) => {
    ///         match result {
    ///             Ok(message) => {
    ///                 println!("Received message: {message}");
    ///                 return;
    ///             }
    ///             Err(Disconnected) => {
    ///                 panic!("The sender was disconnected before sending a message.");
    ///             }
    ///         }
    ///     }
    ///     Err(receiver) => receiver,
    /// };
    ///
    /// sender.send("Hello, world!".to_string());
    ///
    /// match receiver.into_value() {
    ///     Ok(result) => {
    ///         match result {
    ///             Ok(message) => {
    ///                 println!("Received message: {message}");
    ///             }
    ///             Err(Disconnected) => {
    ///                 panic!("The sender was disconnected before sending a message.");
    ///             }
    ///         }
    ///     }
    ///     Err(_) => {
    ///         panic!("No value was received even after send(). This should never happen.");
    ///     }
    /// };
    /// # }
    /// ```
    pub fn into_value(self) -> Result<Result<T, Disconnected>, Self> {
        match self.inner.into_value() {
            Ok(value) => Ok(value),
            Err(inner) => Err(Self { inner }),
        }
    }
}

impl<T: Send> Future for PooledReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We never move out of `self`, only access its inner field.
        let inner = unsafe { self.map_unchecked_mut(|x| &mut x.inner) };

        inner.poll(cx)
    }
}

impl<T: Send> fmt::Debug for PooledReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(PooledSender<u32>: Send);
    assert_not_impl_any!(PooledSender<u32>: Sync);

    assert_impl_all!(PooledReceiver<u32>: Send);
    assert_not_impl_any!(PooledReceiver<u32>: Sync);
}
