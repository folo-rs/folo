//! This simply wraps the core endpoints with a nicer API surface that eliminates
//! the outer generic type parameter, leaving only the inner T of the payload.

use std::any::type_name;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::Poll;
use std::{fmt, task};

use crate::{Disconnected, IntoValueError, LocalReceiverCore, LocalSenderCore, RawLocalPooledRef};

/// Delivers a single value to the receiver connected to the same event.
///
/// This kind of endpoint is used for events stored in a raw single-threaded event pool or event lake.
pub struct RawLocalPooledSender<T: 'static> {
    inner: LocalSenderCore<RawLocalPooledRef<T>, T>,
}

// The NonNull<UnsafeCell<...>> in RawLocalPooledRef causes
// !RefUnwindSafe via auto-trait inference. The pointed-to pool core
// is protected by a RefCell and cannot be observed in an inconsistent
// state during unwind.
impl<T: 'static> UnwindSafe for RawLocalPooledSender<T> {}
impl<T: 'static> RefUnwindSafe for RawLocalPooledSender<T> {}

impl<T: 'static> RawLocalPooledSender<T> {
    pub(crate) fn new(inner: LocalSenderCore<RawLocalPooledRef<T>, T>) -> Self {
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
impl<T: 'static> fmt::Debug for RawLocalPooledSender<T> {
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
/// This kind of endpoint is used for events stored in a raw single-threaded event pool or event lake.
///
/// # Reentrancy
///
/// Cloning a waker during polling may synchronously send through or drop the sender. Waking or
/// dropping a registered waker during completion or cancellation may synchronously poll this
/// receiver to completion or drop an endpoint. The event publishes the resulting state before
/// each callback, and catching a callback panic returns its slot to the pool once cleanup completes.
pub struct RawLocalPooledReceiver<T: 'static> {
    inner: LocalReceiverCore<RawLocalPooledRef<T>, T>,
}

// See RawLocalPooledSender for justification.
impl<T: 'static> UnwindSafe for RawLocalPooledReceiver<T> {}
impl<T: 'static> RefUnwindSafe for RawLocalPooledReceiver<T> {}

impl<T: 'static> RawLocalPooledReceiver<T> {
    pub(crate) fn new(inner: LocalReceiverCore<RawLocalPooledRef<T>, T>) -> Self {
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
    /// use events_once::{IntoValueError, RawLocalEventPool};
    ///
    /// fn main() {
    ///     let pool = Box::pin(RawLocalEventPool::<String>::new());
    ///
    ///     // SAFETY: `pool` was pinned via `Box::pin` before renting and remains pinned and
    ///     // valid for as long as the `Box` is alive; `sender` and `receiver` are both consumed
    ///     // below, before `pool` goes out of scope and drops.
    ///     let (sender, receiver) = unsafe { pool.as_ref().rent() };
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

impl<T: 'static> Future for RawLocalPooledReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        Pin::new(&mut this.inner).poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for RawLocalPooledReceiver<T> {
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
    use crate::{IntoValueError, RawLocalEventPool};

    assert_not_impl_any!(RawLocalPooledSender<u32>: Send, Sync);
    assert_not_impl_any!(RawLocalPooledReceiver<u32>: Send, Sync);

    assert_impl_all!(
        RawLocalPooledSender<u32>: UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        RawLocalPooledReceiver<u32>: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn into_value_disconnected() {
        let pool = Box::pin(RawLocalEventPool::<i32>::new());

        // SAFETY: We guarantee the pool outlives the endpoints.
        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        drop(sender);

        assert!(matches!(
            receiver.into_value(),
            Err(IntoValueError::Disconnected)
        ));
    }
}
