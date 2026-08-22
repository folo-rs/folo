//! This simply wraps the core endpoints with a nicer API surface that eliminates
//! the outer generic type parameter, leaving only the inner T of the payload.
//!
//! Hot-path forwarders are inlined so this API layer does not interrupt the generic core's
//! inlining chain. Ref: `packages/events_once/AGENTS.md`, "`#[inline]` annotations have outsized
//! impact in this package".

use std::any::type_name;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::Poll;
use std::{fmt, task};

use crate::{
    BoxedLocalRef, Disconnected, IntoValueError, LocalReceiverCore, LocalSenderCore, PtrLocalRef,
};

/// Delivers a single value to the receiver connected to the same event.
///
/// This kind of endpoint is used for boxed events, which are heap-allocated and automatically
/// destroyed when both the sender and receiver are dropped.
pub struct BoxedLocalSender<T: 'static> {
    inner: LocalSenderCore<BoxedLocalRef<T>, T>,
}

// Senders are one-shot and consumed on use. The UnsafeCell around the
// underlying event is guarded by a state machine that prevents observing
// inconsistent state during unwind.
impl<T: 'static> UnwindSafe for BoxedLocalSender<T> {}
impl<T: 'static> RefUnwindSafe for BoxedLocalSender<T> {}

impl<T: 'static> BoxedLocalSender<T> {
    pub(crate) fn new(inner: LocalSenderCore<BoxedLocalRef<T>, T>) -> Self {
        Self { inner }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    #[inline]
    pub fn send(self, value: T) {
        self.inner.send(value);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for BoxedLocalSender<T> {
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
/// dropping a registered waker during completion may synchronously poll this receiver to completion
/// or drop an endpoint. Destruction of a registered waker or discarded payload during cancellation
/// may run arbitrary user code, including re-entering related user state or dropping a surviving
/// endpoint. Before each callback, the event publishes the resulting state and completes any
/// endpoint or storage cleanup that must survive unwinding.
pub struct BoxedLocalReceiver<T: 'static> {
    inner: LocalReceiverCore<BoxedLocalRef<T>, T>,
}

// Receivers are one-shot. The UnsafeCell around the underlying event is
// guarded by a state machine that prevents observing inconsistent state
// during unwind.
impl<T: 'static> UnwindSafe for BoxedLocalReceiver<T> {}
impl<T: 'static> RefUnwindSafe for BoxedLocalReceiver<T> {}

impl<T: 'static> BoxedLocalReceiver<T> {
    pub(crate) fn new(inner: LocalReceiverCore<BoxedLocalRef<T>, T>) -> Self {
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
    #[inline]
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
    /// use events_once::{IntoValueError, LocalEvent};
    ///
    /// fn main() {
    ///     let (sender, receiver) = LocalEvent::<String>::boxed();
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

impl<T: 'static> Future for BoxedLocalReceiver<T> {
    type Output = Result<T, Disconnected>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        Pin::new(&mut this.inner).poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for BoxedLocalReceiver<T> {
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
pub struct RawLocalSender<T: 'static> {
    inner: LocalSenderCore<PtrLocalRef<T>, T>,
}

// See BoxedLocalSender for justification.
impl<T: 'static> UnwindSafe for RawLocalSender<T> {}
impl<T: 'static> RefUnwindSafe for RawLocalSender<T> {}

impl<T: 'static> RawLocalSender<T> {
    pub(crate) fn new(inner: LocalSenderCore<PtrLocalRef<T>, T>) -> Self {
        Self { inner }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    #[inline]
    pub fn send(self, value: T) {
        self.inner.send(value);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for RawLocalSender<T> {
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
/// dropping a registered waker during completion may synchronously poll this receiver to completion
/// or drop an endpoint. Destruction of a registered waker or discarded payload during cancellation
/// may run arbitrary user code, including re-entering related user state or dropping a surviving
/// endpoint. Before each callback, the event publishes the resulting state and completes any
/// endpoint or storage cleanup that must survive unwinding.
pub struct RawLocalReceiver<T: 'static> {
    inner: LocalReceiverCore<PtrLocalRef<T>, T>,
}

// See BoxedLocalReceiver for justification.
impl<T: 'static> UnwindSafe for RawLocalReceiver<T> {}
impl<T: 'static> RefUnwindSafe for RawLocalReceiver<T> {}

impl<T: 'static> RawLocalReceiver<T> {
    pub(crate) fn new(inner: LocalReceiverCore<PtrLocalRef<T>, T>) -> Self {
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
    #[inline]
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
    /// use events_once::{EmbeddedLocalEvent, IntoValueError, LocalEvent};
    ///
    /// fn main() {
    ///     let mut place = Box::pin(EmbeddedLocalEvent::<String>::new());
    ///
    ///     // SAFETY: `place` was just constructed, so it is not in use by another event; it
    ///     // stays pinned and writable for as long as the `Box` lives, which outlasts `sender`
    ///     // and `receiver` below, both of which are consumed before `place` is dropped.
    ///     let (sender, receiver) = unsafe { LocalEvent::placed(place.as_mut()) };
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

impl<T: 'static> Future for RawLocalReceiver<T> {
    type Output = Result<T, Disconnected>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        Pin::new(&mut this.inner).poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for RawLocalReceiver<T> {
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

    assert_not_impl_any!(BoxedLocalSender<u32>: Send, Sync);
    assert_not_impl_any!(BoxedLocalReceiver<u32>: Send, Sync);
    assert_not_impl_any!(RawLocalSender<u32>: Send, Sync);
    assert_not_impl_any!(RawLocalReceiver<u32>: Send, Sync);

    assert_impl_all!(
        BoxedLocalSender<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        BoxedLocalReceiver<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        RawLocalSender<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        RawLocalReceiver<u32>: UnwindSafe, RefUnwindSafe
    );
}
