use std::any::type_name;
use std::cell::Cell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::sync::atomic;
use std::task::{self, Poll};

use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, EVENT_SIGNALING,
    Event, EventRef, IntoValueError,
};

/// Receives a single value from the sender connected to the same event.
pub(crate) struct ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    // This is `None` if the receiver has already completed. We need to guard against that
    // because the event may be released as soon as the receiver reaches a terminal state.
    event_ref: Option<E>,

    _t: PhantomData<fn() -> T>,

    // Cell<()> is natively Send + !Sync, which opts the type out of Sync without requiring
    // an unsafe impl Send. Using PhantomData<*mut ()> + unsafe impl Send would be simpler
    // but triggers a rustc bug (rust-lang/rust#110338) in async generator Send inference
    // See: https://github.com/folo-rs/folo/issues/142
    _not_sync: PhantomData<Cell<()>>,
}

impl<E, T> ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    #[must_use]
    pub(crate) fn new(event_ref: E) -> Self {
        Self {
            event_ref: Some(event_ref),
            _t: PhantomData,
            _not_sync: PhantomData,
        }
    }

    /// Returns a shared reference to the event that an endpoint reference identifies.
    ///
    /// This is the single place where the receiver turns the storage policy's cell into an
    /// event reference, so the proof for that conversion is made once.
    #[inline]
    fn event(event_ref: &E) -> &Event<T> {
        // SAFETY: Validity: the `EventRef` contract guarantees that dereferencing yields one
        // initialized and aligned event that stays valid for as long as this endpoint can
        // access it, which includes the returned reference. Aliasing: the same contract limits
        // all access to shared references, and the event synchronizes its interior fields
        // itself, so no exclusive reference to the event can exist.
        unsafe { &*event_ref.get() }
    }

    /// Checks whether the event has completed, in which case reception can finish immediately.
    ///
    /// Completion means either that a value has been sent or that the sender disconnected
    /// without sending one, so a completed event does not necessarily yield a value. See
    /// `state.rs` for the canonical meaning of the event states.
    ///
    /// Only valid before the receiver has completed.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    #[must_use]
    pub(crate) fn is_ready(&self) -> bool {
        let Some(event_ref) = &self.event_ref else {
            panic!("receiver inspected after completion");
        };

        Self::event(event_ref).is_set()
    }

    /// Consumes the receiver and returns the value, if the event has completed with one.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available value without blocking. It returns `Ok(value)` if a value has
    /// been sent, `Err(IntoValueError::Pending(self))` if the event has not completed yet, and
    /// `Err(IntoValueError::Disconnected)` if the sender disconnected without sending a value.
    ///
    /// Only valid before the receiver has completed.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    pub(crate) fn into_value(self) -> Result<T, IntoValueError<Self>> {
        let Some(event_ref) = self.event_ref.as_ref() else {
            panic!("receiver consumed after completion");
        };

        let current_state = Self::event(event_ref).state.load(atomic::Ordering::Acquire);

        match current_state {
            EVENT_BOUND | EVENT_AWAITING | EVENT_SIGNALING => {
                // The event has not completed, so we return the receiver to the caller and the
                // event remains in the care of both endpoints.
                Err(IntoValueError::Pending(self))
            }
            EVENT_SET | EVENT_DISCONNECTED => {
                // The event has completed - consume self and extract its terminal result.
                let mut this = ManuallyDrop::new(self);
                let event_ref = this.event_ref.take().expect(
                    "event_ref was proven present above and neither the state inspection nor \
                     wrapping self in ManuallyDrop can clear it",
                );

                match Event::take_result(&event_ref) {
                    Ok(value) => {
                        // SAFETY: `take_result` returning a value means the state machine made
                        // the receiver the last endpoint and assigned it cleanup ownership. We
                        // do not access the event after this call - the receiver is consumed
                        // and its reference goes out of scope.
                        unsafe {
                            event_ref.release_event();
                        }

                        drop(event_ref);
                        Ok(value)
                    }
                    Err(Disconnected) => {
                        // SAFETY: `take_result` reporting disconnection means the state machine
                        // made the receiver the last endpoint and assigned it cleanup ownership.
                        // We do not access the event after this call - the receiver is consumed
                        // and its reference goes out of scope.
                        unsafe {
                            event_ref.release_event();
                        }

                        drop(event_ref);
                        Err(IntoValueError::Disconnected)
                    }
                }
            }
            // Defensive: state machine guarantees this is unreachable.
            _ => {
                unreachable!(
                    "unreachable {} state on into_value: {current_state}",
                    type_name::<Event<T>>()
                )
            }
        }
    }
}

impl<E, T> Future for ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    type Output = Result<T, Disconnected>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // The receiver core is never structurally pinned: it holds an endpoint reference, which
        // is a pointer handle, and refers to the payload only through markers. Projecting via
        // `get_mut` keeps that fact compiler-verified instead of assumed.
        let this = self.get_mut();

        let inner_poll_result = {
            let Some(event_ref) = this.event_ref.as_ref() else {
                panic!("receiver polled after completion: Future trait contract violated");
            };

            Self::event(event_ref).poll(cx.waker())
        };

        if inner_poll_result.is_some() {
            // We remove the reference from the receiver before releasing the event, which is
            // what makes any later operation on the receiver panic instead of reaching released
            // storage.
            let event_ref = this.event_ref.take().expect(
                "the poll above panics unless the reference is present, and it cannot be \
                 cleared while the poll holds an exclusive reference to the receiver",
            );

            // SAFETY: A result from `poll` means the state machine reached a terminal state and
            // assigned cleanup ownership to the receiver. The reference no longer belongs to the
            // receiver, so nothing accesses the event after this call.
            unsafe {
                event_ref.release_event();
            }
        }

        inner_poll_result.map_or_else(|| Poll::Pending, Poll::Ready)
    }
}

/// Cancels a receiver that still owns an endpoint reference.
///
/// This path invokes user destructors and may release storage, while completed receivers only need
/// to observe that their reference is absent. Keeping cancellation out of line preserves that
/// common drop path through the public endpoint wrappers.
// Callgrind and disassembly show that permitting inlining pulls this callback-heavy state machine
// into generic receiver drop glue and regresses completed-event lifecycles.
#[cold]
#[inline(never)]
fn cancel_receiver<E, T>(event_ref: E)
where
    E: EventRef<T>,
    T: Send + 'static,
{
    let cancellation = Event::cancel(&event_ref);
    let release_event = !matches!(&cancellation.result, Ok(None));

    if release_event {
        // Either a value was waiting for us or the sender has disconnected. Both outcomes leave
        // the receiver as the last endpoint, so we release the event.
        //
        // SAFETY: `cancel` returned a terminal outcome, which is how the state machine assigns
        // cleanup ownership to the receiver. The receiver is being dropped and its reference goes
        // out of scope, so nothing accesses the event after this call.
        unsafe {
            event_ref.release_event();
        }
    }

    // Endpoint bookkeeping and any owned storage release must complete before destructors for the
    // discarded payload or extracted waker invoke user code.
    drop(event_ref);
    drop(cancellation);
}

impl<E, T> Drop for ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    #[inline]
    fn drop(&mut self) {
        if let Some(event_ref) = self.event_ref.take() {
            cancel_receiver(event_ref);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<E, T> fmt::Debug for ReceiverCore<E, T>
where
    E: EventRef<T>,
    T: Send + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event_ref", &self.event_ref)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::marker::PhantomPinned;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::with_watchdog;

    use super::*;
    use crate::{BoxedRef, PooledRef, PtrRef};

    // `Box<dyn Send>` is the minimally qualified payload here: it satisfies the `T: Send +
    // 'static` bound that the receiver requires and lacks every other auto trait, so these
    // assertions cannot be satisfied by the payload's own markers. Preserving `Send` for such
    // payloads is also a regression test for #142.
    assert_impl_all!(ReceiverCore<BoxedRef<Box<dyn Send>>, Box<dyn Send>>: Send);
    assert_not_impl_any!(ReceiverCore<BoxedRef<Box<dyn Send>>, Box<dyn Send>>: Sync);

    assert_impl_all!(ReceiverCore<PtrRef<Box<dyn Send>>, Box<dyn Send>>: Send);
    assert_not_impl_any!(ReceiverCore<PtrRef<Box<dyn Send>>, Box<dyn Send>>: Sync);

    // The event payload being `!Unpin` should not cause the endpoints to become `!Unpin`.
    assert_impl_all!(ReceiverCore<BoxedRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(ReceiverCore<PtrRef<PhantomPinned>, PhantomPinned>: Unpin);
    assert_impl_all!(ReceiverCore<PooledRef<PhantomPinned>, PhantomPinned>: Unpin);

    // `into_value()` inspects the shared event state and may finalize and release the event
    // while the sender is still acting on the other thread. `Future::poll` does not cover that
    // sequence because it takes the waker path instead. Both tests accept either a completed or
    // a pending outcome, because the interleaving is what varies; what must hold is that a
    // pending outcome returns a still-usable receiver that later observes the sender's action.
    //
    // The two tests reach different intermediate states. The send test cannot reach the
    // transient SIGNALING state: its event has no awaiter, so the send transition goes straight
    // from BOUND to SET. The disconnect test can observe SIGNALING, because
    // `sender_dropped_without_set` swaps into SIGNALING even from BOUND before publishing
    // DISCONNECTED; a receiver that observes that window classifies it as pending. Neither
    // interleaving is guaranteed, so deterministic coverage of the SIGNALING classification
    // lives in `core::sync::tests::boxed_into_value_pending_while_sender_signaling`, which parks
    // a sender in SIGNALING with the state machine's test hook.

    #[test]
    fn boxed_into_value_races_send_mt() {
        with_watchdog(|| {
            let (sender, receiver) = Event::<u32>::boxed_core();

            thread::scope(|scope| {
                let send_thread = scope.spawn(move || {
                    sender.send(42);
                });

                let pending_receiver = match receiver.into_value() {
                    Ok(value) => {
                        assert_eq!(value, 42);
                        None
                    }
                    Err(IntoValueError::Pending(receiver)) => Some(receiver),
                    Err(IntoValueError::Disconnected) => {
                        panic!("the sender sends a value instead of disconnecting")
                    }
                };

                send_thread.join().unwrap();

                if let Some(receiver) = pending_receiver {
                    // The sender has finished, so the value must now be extractable.
                    assert_eq!(receiver.into_value().unwrap(), 42);
                }
            });
        });
    }

    #[test]
    fn boxed_into_value_races_disconnect_mt() {
        with_watchdog(|| {
            let (sender, receiver) = Event::<u32>::boxed_core();

            thread::scope(|scope| {
                let send_thread = scope.spawn(move || {
                    drop(sender);
                });

                let pending_receiver = match receiver.into_value() {
                    Ok(value) => panic!("no value was ever sent but received {value}"),
                    Err(IntoValueError::Pending(receiver)) => Some(receiver),
                    Err(IntoValueError::Disconnected) => None,
                };

                send_thread.join().unwrap();

                if let Some(receiver) = pending_receiver {
                    // The sender has finished, so disconnection must now be visible.
                    assert!(matches!(
                        receiver.into_value(),
                        Err(IntoValueError::Disconnected)
                    ));
                }
            });
        });
    }
}
