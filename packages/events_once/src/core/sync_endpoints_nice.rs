//! This simply wraps the core endpoints with a nicer API surface that eliminates
//! the outer generic type parameter, leaving only the inner T of the payload.

use std::any::type_name;
use std::pin::Pin;
use std::task::Poll;
use std::{fmt, task};

use crate::{BoxedRef, Disconnected, PtrRef, ReceiverCore, SenderCore};

/// Delivers a single value to the receiver connected to the same event.
///
/// This kind of endpoint is used for boxed events, which are heap-allocated and automatically
/// destroyed when both the sender and receiver are dropped.
pub struct BoxedSender<T: Send> {
    inner: SenderCore<BoxedRef<T>, T>,
}

impl<T: Send> BoxedSender<T> {
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

impl<T: Send> fmt::Debug for BoxedSender<T> {
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
pub struct BoxedReceiver<T: Send> {
    inner: ReceiverCore<BoxedRef<T>, T>,
}

impl<T: Send> BoxedReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<BoxedRef<T>, T>) -> Self {
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
    /// use events_once::{Disconnected, Event};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let (sender, receiver) = Event::<String>::boxed();
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

impl<T: Send> Future for BoxedReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We never move out of `self`, only access its inner field.
        let inner = unsafe { self.map_unchecked_mut(|x| &mut x.inner) };

        inner.poll(cx)
    }
}

impl<T: Send> fmt::Debug for BoxedReceiver<T> {
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
pub struct RawSender<T: Send> {
    inner: SenderCore<PtrRef<T>, T>,
}

impl<T: Send> RawSender<T> {
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

impl<T: Send> fmt::Debug for RawSender<T> {
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
pub struct RawReceiver<T: Send> {
    inner: ReceiverCore<PtrRef<T>, T>,
}

impl<T: Send> RawReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<PtrRef<T>, T>) -> Self {
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
    /// use events_once::{Disconnected, EmbeddedEvent, Event};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut event = Box::pin(EmbeddedEvent::<String>::new());
    ///
    /// // SAFETY: We promise that `event` lives longer than any of the endpoints returned.
    /// let (sender, receiver) = unsafe { Event::placed(event.as_mut()) };
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

impl<T: Send> Future for RawReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We never move out of `self`, only access its inner field.
        let inner = unsafe { self.map_unchecked_mut(|x| &mut x.inner) };

        inner.poll(cx)
    }
}

impl<T: Send> fmt::Debug for RawReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish()
    }
}
