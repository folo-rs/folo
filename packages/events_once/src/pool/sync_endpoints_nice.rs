//! This simply wraps the core endpoints with a nicer API surface that eliminates
//! the outer generic type parameter, leaving only the inner T of the payload.
//!
//! Hot-path forwarders are inlined so this API layer does not interrupt the
//! generic core's inlining chain. Ref: `packages/events_once/AGENTS.md`,
//! "`#[inline]` annotations have outsized impact in this package".

use std::any::type_name;
use std::panic::{RefUnwindSafe, UnwindSafe};
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

// Senders are one-shot and consumed on use. The underlying event publishes stable state through
// its atomic state machine before callbacks that may unwind, so the endpoint cannot expose an
// inconsistent state across an unwind boundary.
impl<T: Send + 'static> UnwindSafe for PooledSender<T> {}
impl<T: Send + 'static> RefUnwindSafe for PooledSender<T> {}

impl<T: Send + 'static> PooledSender<T> {
    pub(crate) fn new(inner: SenderCore<PooledRef<T>, T>) -> Self {
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
///
/// # Reentrancy
///
/// Cloning a waker during polling may synchronously send through or drop the sender. Waking or
/// dropping a registered waker during completion may synchronously poll this receiver to completion
/// or drop an endpoint. Destruction of a registered waker or discarded payload during cancellation
/// may run arbitrary user code, including using the event's pool or lake. Before each callback, the
/// event publishes the resulting state and completes any endpoint or storage cleanup that must
/// survive unwinding.
pub struct PooledReceiver<T: Send + 'static> {
    inner: ReceiverCore<PooledRef<T>, T>,
}

// See PooledSender for justification.
impl<T: Send + 'static> UnwindSafe for PooledReceiver<T> {}
impl<T: Send + 'static> RefUnwindSafe for PooledReceiver<T> {}

impl<T: Send + 'static> PooledReceiver<T> {
    pub(crate) fn new(inner: ReceiverCore<PooledRef<T>, T>) -> Self {
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
    /// use events_once::{EventPool, IntoValueError};
    ///
    /// fn main() {
    ///     let pool = EventPool::<String>::new();
    ///     let (sender, receiver) = pool.rent();
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

impl<T: Send + 'static> Future for PooledReceiver<T> {
    type Output = Result<T, Disconnected>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        Pin::new(&mut this.inner).poll(cx)
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
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{EventPool, IntoValueError};

    assert_impl_all!(PooledSender<u32>: Send);
    assert_not_impl_any!(PooledSender<u32>: Sync);

    assert_impl_all!(PooledReceiver<u32>: Send);
    assert_not_impl_any!(PooledReceiver<u32>: Sync);

    // Trait object payloads must preserve Send (regression test for #142).
    assert_impl_all!(PooledSender<Box<dyn Send>>: Send);
    assert_impl_all!(PooledReceiver<Box<dyn Send>>: Send);

    // Verify that an async block awaiting a trait-object receiver is Send. Static assertions
    // alone do not catch the bug because it only manifests in async generator analysis.
    fn _assert_future_send<F: Future + Send>(_: F) {}

    fn _pooled_receiver_trait_object_future_is_send() {
        let pool = EventPool::<Box<dyn Send>>::new();
        let (_tx, rx) = pool.rent();

        _assert_future_send(async move {
            drop(rx.await);
        });
    }

    assert_impl_all!(
        PooledSender<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        PooledReceiver<u32>: UnwindSafe, RefUnwindSafe
    );

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
