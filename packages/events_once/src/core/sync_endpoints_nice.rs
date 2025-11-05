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
    inner: SenderCore<BoxedRef<T>>,
}

impl<T: Send> BoxedSender<T> {
    pub(crate) fn new(inner: SenderCore<BoxedRef<T>>) -> Self {
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
/// This kind of endpoint is used for boxed events, which are heap-allocated and automatically
/// destroyed when both the sender and receiver are dropped.
pub struct BoxedReceiver<T: Send> {
    inner: ReceiverCore<BoxedRef<T>>,
}

impl<T: Send> BoxedReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<BoxedRef<T>>) -> Self {
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
    inner: SenderCore<PtrRef<T>>,
}

impl<T: Send> RawSender<T> {
    pub(crate) fn new(inner: SenderCore<PtrRef<T>>) -> Self {
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
/// This kind of endpoint is used with events for which the storage is provided by the
/// owner of the endpoint. They are also responsible for ensuring that the event that
/// connects the sender-receiver pair outlives both endpoints.
pub struct RawReceiver<T: Send> {
    inner: ReceiverCore<PtrRef<T>>,
}

impl<T: Send> RawReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<PtrRef<T>>) -> Self {
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
