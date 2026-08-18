use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{MaybeUninit, offset_of};
use std::panic::RefUnwindSafe;
use std::pin::Pin;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::Arc;
use std::task::Waker;

#[cfg(debug_assertions)]
use crate::{BacktraceType, capture_backtrace};
use crate::{
    BoxedLocalReceiver, BoxedLocalRef, BoxedLocalSender, Disconnected, EVENT_AWAITING, EVENT_BOUND,
    EVENT_DISCONNECTED, EVENT_SET, EmbeddedLocalEvent, LocalReceiverCore, LocalSenderCore,
    PtrLocalRef, RawLocalReceiver, RawLocalSender,
};

/// Coordinates delivery of a `T` at most once from a sender to a receiver on the same thread.
pub struct LocalEvent<T> {
    /// The logical state of the event; see constants in `state.rs`.
    pub(crate) state: Cell<u8>,

    /// If `state` is [`EVENT_AWAITING`], this field is initialized with the
    /// waker of whoever most recently awaited the receiver. In other states, this field is not
    /// initialized.
    ///
    /// We use `MaybeUninit` to minimize the storage and avoid an `Option` or enum overhead,
    /// as we already track the presence via `state`.
    ///
    /// We use `UnsafeCell` because we are a synchronization primitive and
    /// do our own synchronization of reads/writes.
    awaiter: UnsafeCell<MaybeUninit<Waker>>,

    /// If `state` is `EVENT_SET`, this field is initialized with the value that was sent by
    /// the sender. In other states, this field is not initialized.
    ///
    /// We use `MaybeUninit` to minimize the storage and avoid an `Option` or enum overhead,
    /// as we already track the presence via `state`.
    ///
    /// We use `UnsafeCell` because we are a synchronization primitive and
    /// do our own synchronization of reads/writes.
    value: UnsafeCell<MaybeUninit<T>>,

    // In debug builds, we save the backtrace of the most recent awaiter here. The value will be
    // retained for the entire lifetime of the event, even after the awaiter has been woken up,
    // to allow inspection after the fact.
    #[cfg(debug_assertions)]
    backtrace: RefCell<Option<BacktraceType>>,

    // Everything to do with this event is single-threaded,
    // even if T is thread-mobile or thread-safe.
    _single_threaded: PhantomData<*const ()>,

    // It is invalid to move this type once it has been pinned because
    // the sender and receiver connect via raw pointers to the event.
    _requires_pinning: PhantomPinned,
}

// The Cell and UnsafeCell fields (state, awaiter, value) cause auto-trait
// inference to mark LocalEvent as !RefUnwindSafe. However, the state
// machine prevents any shared reference from observing inconsistent state
// during unwind, making this safe.
impl<T: 'static> RefUnwindSafe for LocalEvent<T> {}

impl<T: 'static> LocalEvent<T> {
    /// In-place initializes a new instance in the `BOUND` state.
    ///
    /// This is for internal use only and is wrapped by public methods that also
    /// wire up the sender and receiver after doing the initialization. An event
    /// without a sender and receiver is invalid.
    pub(crate) fn new_in_inner(place: &mut MaybeUninit<Self>) -> NonNull<Self> {
        // We can skip initializing the MaybeUninit fields because they start uninitialized
        // by design and the UnsafeCell wrapper is transparent, only affecting accesses and
        // not the contents.
        let base_ptr = place.as_mut_ptr();

        // SAFETY: We are making a pointer to a known field at a compiler-guaranteed offset.
        let state_ptr = unsafe { base_ptr.byte_add(offset_of!(Self, state)) }.cast::<Cell<u8>>();

        // SAFETY: This is the matching field of the type we are initializing, so valid for writes.
        unsafe {
            state_ptr.write(Cell::new(EVENT_BOUND));
        }

        #[cfg(debug_assertions)]
        {
            // SAFETY: We are making a pointer to a known field at a compiler-guaranteed offset.
            let backtrace_ptr = unsafe { base_ptr.byte_add(offset_of!(Self, backtrace)) }
                .cast::<RefCell<Option<BacktraceType>>>();

            // SAFETY: This is the matching field of the type we are initializing, so valid for writes.
            unsafe {
                backtrace_ptr.write(RefCell::new(None));
            }
        }

        // SAFETY: This came from a reference so guaranteed non-null.
        unsafe { NonNull::new_unchecked(base_ptr) }
    }

    #[must_use]
    pub(crate) fn boxed_core() -> (
        LocalSenderCore<BoxedLocalRef<T>, T>,
        LocalReceiverCore<BoxedLocalRef<T>, T>,
    ) {
        let (sender_event, receiver_event) = BoxedLocalRef::new_pair();

        (
            LocalSenderCore::new(sender_event),
            LocalReceiverCore::new(receiver_event),
        )
    }

    /// Heap-allocates a new instance and returns the endpoints.
    ///
    /// The memory used is released when both endpoints are dropped.
    ///
    /// For more efficiency, consider using [`placed`][Self::placed], which allows you to
    /// initialize the event in preallocated storage as part of a larger structure.
    ///
    /// # Examples
    ///
    /// ```
    /// use events_once::LocalEvent;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let (sender, receiver) = LocalEvent::<String>::boxed();
    ///
    /// sender.send("Hello, world!".to_string());
    ///
    /// let message = receiver.await.unwrap();
    /// assert_eq!(message, "Hello, world!");
    /// # }
    /// ```
    #[must_use]
    pub fn boxed() -> (BoxedLocalSender<T>, BoxedLocalReceiver<T>) {
        let (sender_core, receiver_core) = Self::boxed_core();

        (
            BoxedLocalSender::new(sender_core),
            BoxedLocalReceiver::new(receiver_core),
        )
    }

    #[must_use]
    pub(crate) unsafe fn placed_core(
        place: Pin<&mut MaybeUninit<Self>>,
    ) -> (
        LocalSenderCore<PtrLocalRef<T>, T>,
        LocalReceiverCore<PtrLocalRef<T>, T>,
    ) {
        // SAFETY: Nothing is getting moved, we just temporarily unwrap the Pin wrapper.
        let event = Self::new_in_inner(unsafe { place.get_unchecked_mut() });

        (
            LocalSenderCore::new(PtrLocalRef::new(event)),
            LocalReceiverCore::new(PtrLocalRef::new(event)),
        )
    }

    /// Initializes the event in-place, returning the endpoints.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that:
    ///
    /// * The referenced place remains valid for writes for the entire lifetime of
    ///   the sender and receiver returned by this function.
    /// * The referenced place remains pinned for the entire lifetime of
    ///   the sender and receiver returned by this function.
    /// * The referenced place is not already in use by another instance of the event.
    ///
    /// # Examples
    ///
    /// ```
    /// use events_once::{EmbeddedLocalEvent, LocalEvent};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let mut place = Box::pin(EmbeddedLocalEvent::<String>::new());
    ///
    /// // SAFETY: We promise that `place` lives longer than the endpoints.
    /// let (sender, receiver) = unsafe { LocalEvent::placed(place.as_mut()) };
    ///
    /// sender.send("Hello from embedded event!".to_string());
    ///
    /// let message = receiver.await.unwrap();
    /// assert_eq!(message, "Hello from embedded event!");
    /// # }
    /// ```
    #[must_use]
    pub unsafe fn placed(
        place: Pin<&mut EmbeddedLocalEvent<T>>,
    ) -> (RawLocalSender<T>, RawLocalReceiver<T>) {
        // SAFETY: Not moving anything, just breaking through the public wrapper API.
        let place = unsafe { place.map_unchecked_mut(|container| &mut container.inner) };

        // SAFETY: Forwarding safety guarantees from the caller.
        let (sender_core, receiver_core) = unsafe { Self::placed_core(place) };

        (
            RawLocalSender::new(sender_core),
            RawLocalReceiver::new(receiver_core),
        )
    }

    /// Returns a snapshot of the backtrace of the most recent awaiter of this event,
    /// if there has been an awaiter and if backtrace capturing is enabled.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The snapshot is a shared owner of the backtrace and remains valid even if the event is
    /// released afterwards. Callers that want to hand a backtrace to user code must take a
    /// snapshot instead of inspecting the event under its borrow, so that no borrow is held
    /// while user code runs.
    #[cfg(debug_assertions)]
    pub(crate) fn awaiter_backtrace(&self) -> Option<Arc<Backtrace>> {
        let backtrace = self.backtrace.borrow();

        backtrace.as_ref().map(Arc::clone)
    }

    /// Releases the backtrace of the most recent awaiter of this event.
    ///
    /// The storage an event occupies may be reused or freed without dropping the event, so
    /// whoever releases an event calls this first, to release the memory that the backtrace
    /// occupies. Snapshots taken earlier remain valid because they are shared owners of the
    /// backtrace.
    #[cfg(debug_assertions)]
    pub(crate) fn clear_awaiter_backtrace(&self) {
        let mut backtrace = self.backtrace.borrow_mut();

        *backtrace = None;
    }

    /// Sets the value of the event and notifies the awaiter, if there is one.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    #[inline]
    pub(crate) fn set(&self, result: T) -> Result<(), Disconnected> {
        let value_cell = self.value.get();

        // We can start by setting the value - this has to happen no matter what.
        // Everything else we do here is just to get the awaiter to come pick it up.
        //
        // SAFETY: It is valid for the sender to write here because we know that nobody else will
        // be accessing this field at this time. This is guaranteed by:
        // * There is only one sender and it is single-threaded, so it cannot be used in parallel.
        // * The receiver will only access this field in the "Set" state, which can only be entered
        //   from later on in this method.
        unsafe {
            value_cell.write(MaybeUninit::new(result));
        }

        // We transition directly to EVENT_SET. The sync variant uses a `+= 1`
        // trick that exits the BOUND path in a single atomic fetch_add and uses
        // an intermediate EVENT_SIGNALING state to act as a mutex against the
        // concurrent receiver. LocalEvent is single-threaded, so the SIGNALING
        // intermediate has no purpose, and the non-atomic equivalent of `+= 1`
        // (load + add + store) is one instruction wider than a direct store.
        // `Cell::replace` emits a single load + store sequence and collapses
        // the AWAITING branch's two state writes (SIGNALING then SET) into
        // one. In the DISCONNECTED branch the SET we just stored is a
        // don't-care because the event is about to be deallocated, and no
        // external observer can witness the transient state.
        let previous_state = self.state.replace(EVENT_SET);

        match previous_state {
            EVENT_BOUND => {
                // There was nobody listening via the receiver - our work here is done.
                // The receiver is still connected, so it will clean up.
                Ok(())
            }
            EVENT_AWAITING => {
                // There was someone listening via the receiver. We need to
                // notify the awaiter that they can come back for the value now.
                //
                // The state is already EVENT_SET (set by the replace above), so
                // the reentrant receiver poll triggered by `wake()` will observe
                // a terminal state and proceed straight to value extraction.

                // Extract the waker in a tight scope so the `&mut MaybeUninit<Waker>`
                // borrow is released before the wake callback fires, matching the
                // callback-safety guidance in docs/callback-safety.md.
                let waker = {
                    // SAFETY: The only other potential references to the field are other
                    // short-lived references in this type, which cannot exist at the moment
                    // because the type is single-threaded and does not let any references
                    // escape.
                    let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
                    // SAFETY: UnsafeCell pointer is never null.
                    let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                    // We extract the waker and consider the field uninitialized again.
                    // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker
                    // in there.
                    unsafe { awaiter_cell.assume_init_read() }
                };

                // Come and get it.
                waker.wake();

                // The receiver is still connected, so it will clean up.
                Ok(())
            }
            EVENT_DISCONNECTED => {
                // The receiver has already disconnected, so we can clean up the event now.
                // We have to first drop the value that we inserted into the event, though.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let value_cell_maybe = unsafe { self.value.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let value_cell = unsafe { value_cell_maybe.unwrap_unchecked() };

                // We drop the value and consider the cell uninitialized.
                //
                // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
                unsafe {
                    value_cell.assume_init_drop();
                }

                Err(Disconnected)
            }
            // Defensive: state machine guarantees this is unreachable.
            _ => {
                unreachable!("unreachable LocalEvent state on set: {previous_state}");
            }
        }
    }

    /// We are intended to be polled via `Future::poll`, so we have a similar signature here,
    /// with `None` equating to `Poll::Pending`.
    ///
    /// If `Some` result is returned, the caller is the last remaining endpoint and responsible
    /// for cleaning up the event.
    #[inline]
    #[must_use]
    pub(crate) fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        #[cfg(debug_assertions)]
        self.backtrace.replace(Some(capture_backtrace()));

        match self.state.get() {
            EVENT_BOUND => {
                // The sender has not yet set any value, so we will have to wait.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                awaiter_cell.write(waker.clone());

                // The sender will wake us up when it has set the value.
                self.state.set(EVENT_AWAITING);
                None
            }
            EVENT_SET => {
                // The sender has delivered a value and we can complete the event.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let value_cell_maybe = unsafe { self.value.get().as_ref() };
                // SAFETY: UnsafeCell pointer is never null.
                let value_cell = unsafe { value_cell_maybe.unwrap_unchecked() };

                // We extract the value and consider the cell uninitialized.
                //
                // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
                let value = unsafe { value_cell.assume_init_read() };

                Some(Ok(value))
            }
            EVENT_AWAITING => {
                // We are re-polling after previously starting a wait. This is fine
                // and we just need to clean up the previous waker, replacing it with
                // a new one. The previous registration must be released exactly once;
                // overwriting it without dropping would leak a `Waker` reference.

                // Clone the incoming waker before touching the cell. `Waker::clone`
                // runs arbitrary vtable code, so it must not observe the cell in the
                // transiently-uninitialized window between the read and the write below.
                let new_waker = waker.clone();

                // Extract the previous waker in a tight scope so the
                // `&mut MaybeUninit<Waker>` borrow is released before we drop it below:
                // a waker's drop runs arbitrary vtable code that must not observe an
                // active borrow of shared state.
                // Ref: docs/callback-safety.md, "No callbacks under borrows of shared state".
                let previous_waker = {
                    // SAFETY: The only other potential references to the field are other
                    // short-lived references in this type, which cannot exist at the moment
                    // because the type is single-threaded and does not let any references
                    // escape.
                    let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
                    // SAFETY: UnsafeCell pointer is never null.
                    let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                    // We extract the previous waker and immediately overwrite the cell with
                    // the new one, keeping the field initialized and the state consistent for
                    // any reentrant poll triggered while the previous waker is dropped below.
                    // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker
                    // in there.
                    let previous_waker = unsafe { awaiter_cell.assume_init_read() };
                    awaiter_cell.write(new_waker);
                    previous_waker
                };

                // Release the previous registration exactly once, now that the borrow is gone.
                drop(previous_waker);

                None
            }
            EVENT_DISCONNECTED => {
                // There is no result coming, ever! This is the end.
                Some(Err(Disconnected))
            }
            // Defensive: state machine guarantees this is unreachable.
            state => {
                unreachable!("unreachable LocalEvent state on poll: {state}");
            }
        }
    }

    /// Marks the event as having been disconnected early from the sender side.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    #[inline]
    pub(crate) fn sender_dropped_without_set(&self) -> Result<(), Disconnected> {
        let previous_state = self.state.get();

        // We can immediately set this because this is a single-threaded event, so there cannot
        // be any race condition causing us issues with the receiver seeing this too early.
        self.state.set(EVENT_DISCONNECTED);

        match previous_state {
            EVENT_BOUND => {
                // There was nobody listening via the receiver - our work here is done.
                // The receiver still exists, so it will clean up.
                Ok(())
            }
            EVENT_AWAITING => {
                // There was someone listening via the receiver. We need to notify
                // the awaiter that they can come back for another check now.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                // We extract the waker and consider the field uninitialized again.
                // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
                let waker = unsafe { awaiter_cell.assume_init_read() };

                // Come and get it.
                waker.wake();

                // The receiver is the last endpoint remaining, so it will clean up.
                Ok(())
            }
            EVENT_DISCONNECTED => {
                // The sender is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            // Defensive: state machine guarantees this is unreachable.
            _ => {
                unreachable!("unreachable LocalEvent state on sender disconnect: {previous_state}");
            }
        }
    }

    /// Checks whether the event has been set (either with a value or with a disconnect signal).
    #[must_use]
    pub(crate) fn is_set(&self) -> bool {
        matches!(self.state.get(), EVENT_SET | EVENT_DISCONNECTED)
    }

    /// Attempts to obtain the value from the event, if one has been sent, while indicating
    /// that no further polls will be performed by the receiver.
    ///
    /// Returns `Ok(None)` if the sender has not yet sent a value. In this case, the sender will
    /// eventually clean up the event.
    ///
    /// Returns `Ok(Some(value))` if the sender sender has already sent a value.
    /// Returns `Err` if the sender has already disconnected without sending a value.
    /// In both of these cases, the receiver must clean up the event now.
    #[inline]
    pub(crate) fn final_poll(&self) -> Result<Option<T>, Disconnected> {
        // If we are still awaiting, the receiver (the caller) owns the stored waker and must
        // destroy it before disconnecting. We revert to EVENT_BOUND *before* dropping the waker
        // so that a reentrant sender drop triggered by the waker's destructor observes a live,
        // non-terminal event and defers cleanup, rather than deallocating the storage out from
        // under this still-executing call (which holds a strongly-protected `&self`). Only once
        // the waker is gone do we read the resulting state and transition to EVENT_DISCONNECTED.
        //
        // This mirrors the sync `Event::final_poll`, which reverts AWAITING -> BOUND before
        // destroying the awaiter, and follows docs/callback-safety.md ("No callbacks under
        // borrows of shared state"; symmetric handoff vs. cleanup ordering).
        if self.state.get() == EVENT_AWAITING {
            self.state.set(EVENT_BOUND);

            // SAFETY: The only other potential references to the field are other short-lived
            // references in this type, which cannot exist at the moment because
            // the type is single-threaded and does not let any references escape.
            let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
            // SAFETY: UnsafeCell pointer is never null.
            let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

            // We drop the waker and consider the field uninitialized again.
            // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
            unsafe {
                awaiter_cell.assume_init_drop();
            }
        }

        // Re-read the state: a reentrant sender drop during the waker's destructor above may
        // have advanced us from EVENT_BOUND to EVENT_DISCONNECTED. In this single-threaded
        // design the reentrant sender is the only actor that could have changed the state while
        // we were dropping the waker.
        let previous_state = self.state.get();

        // We can immediately set this because this is a single-threaded event, so there cannot
        // be any race condition causing us issues with the receiver seeing this too early.
        self.state.set(EVENT_DISCONNECTED);

        match previous_state {
            EVENT_BOUND => {
                // The sender had not yet set any value (nor did a reentrant sender drop
                // disconnect while we dropped the waker). It will clean up the event later.
                Ok(None)
            }
            EVENT_SET => {
                // The sender has already set a value but we disconnected before we received it.
                // We need to clean up the value and then later clean up the event, as well.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let value_cell_maybe = unsafe { self.value.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let value_cell = unsafe { value_cell_maybe.unwrap_unchecked() };

                // We extract the value and consider the cell uninitialized.
                //
                // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
                let value = unsafe { value_cell.assume_init_read() };

                // The receiver will clean up.
                Ok(Some(value))
            }
            EVENT_DISCONNECTED => {
                // The receiver is the last endpoint remaining, so it will clean up. This also
                // covers the case where a reentrant sender drop disconnected while we dropped
                // the waker above: the sender observed EVENT_BOUND and deferred cleanup to us.
                Err(Disconnected)
            }
            // Defensive: state machine guarantees this is unreachable, since we already handled
            // and cleared EVENT_AWAITING above.
            _ => {
                unreachable!(
                    "unreachable LocalEvent state on receiver disconnect: {previous_state}"
                );
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
#[expect(clippy::missing_fields_in_debug, reason = "phantoms are boring")]
impl<T: 'static> fmt::Debug for LocalEvent<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct(type_name::<Self>());

        debug.field("state", &self.state);
        debug.field("awaiter", &self.awaiter);
        debug.field("value", &self.value);

        #[cfg(debug_assertions)]
        {
            debug.field("backtrace", &self.backtrace);
        }

        debug.finish()
    }
}

#[cfg(test)]
#[expect(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::mem;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{self, Poll, RawWaker, RawWakerVTable};

    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::{ReentrantWakerData, with_watchdog};

    use super::*;
    use crate::IntoValueError;

    assert_not_impl_any!(LocalEvent<i32>: Send, Sync);

    assert_impl_all!(LocalEvent<u32>: UnwindSafe, RefUnwindSafe);

    #[test]
    fn boxed_send_receive() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_send_receive_unit() {
        let (sender, receiver) = LocalEvent::<()>::boxed();
        let mut receiver = Box::pin(receiver);

        sender.send(());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn boxed_send_receive_u128() {
        let (sender, receiver) = LocalEvent::<u128>::boxed();
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_send_receive_array() {
        let (sender, receiver) = LocalEvent::<[u128; 4]>::boxed();
        let mut receiver = Box::pin(receiver);

        sender.send([42, 43, 44, 45]);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok([42, 43, 44, 45]))));
    }

    #[test]
    fn boxed_receive_send_receive() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        sender.send(42);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_drop_send() {
        let (sender, _) = LocalEvent::<i32>::boxed();

        sender.send(42);
    }

    #[test]
    fn boxed_drop_receive() {
        let (_, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn boxed_receive_drop_receive() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn boxed_receive_drop_send() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);

        sender.send(42);
    }

    #[test]
    fn boxed_receive_drop_drop_receiver_first() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn boxed_receive_drop_drop_sender_first() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn boxed_drop_drop_receiver_first() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn boxed_drop_drop_sender_first() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn boxed_is_ready() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        assert!(!receiver.is_ready());

        sender.send(42);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_drop_is_ready() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        assert!(!receiver.is_ready());

        drop(sender);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn boxed_into_value() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(42)));
    }

    #[test]
    fn boxed_drop_into_value() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        drop(sender);

        assert!(matches!(
            receiver.into_value(),
            Err(IntoValueError::Disconnected)
        ));
    }

    #[test]
    #[should_panic]
    fn boxed_panic_poll_after_completion() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        // Should panic - invalid to access receiver after it completes.
        _ = receiver.as_mut().poll(&mut cx);
    }

    #[test]
    #[should_panic]
    fn boxed_panic_is_ready_after_completion() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        // Should panic - invalid to access receiver after it completes.
        _ = receiver.is_ready();
    }

    #[test]
    fn placed_send_receive() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_receive_send_receive() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        sender.send(42);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_drop_send() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, _) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        sender.send(42);
    }

    #[test]
    fn placed_drop_receive() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (_, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_receive_drop_receive() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_receive_drop_send() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);

        sender.send(42);
    }

    #[test]
    fn placed_receive_drop_drop_receiver_first() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_receive_drop_drop_sender_first() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_drop_drop_receiver_first() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_drop_drop_sender_first() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_is_ready() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        assert!(!receiver.is_ready());

        sender.send(42);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_drop_is_ready() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        assert!(!receiver.is_ready());

        drop(sender);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_into_value() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(42)));
    }

    #[test]
    fn placed_drop_into_value() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        drop(sender);

        assert!(matches!(
            receiver.into_value(),
            Err(IntoValueError::Disconnected)
        ));
    }

    #[test]
    #[should_panic]
    fn placed_panic_poll_after_completion() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        // Should panic - invalid to access receiver after it completes.
        _ = receiver.as_mut().poll(&mut cx);
    }

    #[test]
    #[should_panic]
    fn placed_panic_is_ready_after_completion() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        // Should panic - invalid to access receiver after it completes.
        _ = receiver.is_ready();
    }

    #[test]
    fn boxed_double_poll_replaces_waker() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        // First poll transitions BOUND → AWAITING.
        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        // Second poll enters the EVENT_AWAITING branch (replaces waker).
        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        sender.send(42);

        // Third poll picks up the value.
        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_repoll_releases_previous_waker() {
        // A re-poll in the EVENT_AWAITING state must release the previously
        // registered waker exactly once when it replaces it. We observe this via
        // an `Arc`-backed waker whose strong count reflects the number of live
        // clones: a leaked registration would make the count grow with each re-poll.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::Wake;

        struct CountingWake {
            woken: AtomicUsize,
        }

        impl Wake for CountingWake {
            fn wake(self: Arc<Self>) {
                self.woken.fetch_add(1, Ordering::Relaxed);
            }

            fn wake_by_ref(self: &Arc<Self>) {
                self.woken.fetch_add(1, Ordering::Relaxed);
            }
        }

        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let counter = Arc::new(CountingWake {
            woken: AtomicUsize::new(0),
        });
        let waker = Waker::from(Arc::clone(&counter));

        // Baseline: `counter` plus our local `waker`.
        assert_eq!(Arc::strong_count(&counter), 2);

        let mut cx = task::Context::from_waker(&waker);

        // First poll transitions BOUND → AWAITING and stores one clone.
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
        assert_eq!(Arc::strong_count(&counter), 3);

        // Each re-poll in the AWAITING state must drop the previous clone before
        // storing the new one, so the count stays put.
        for _ in 0..5 {
            assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
            assert_eq!(
                Arc::strong_count(&counter),
                3,
                "re-poll must release the previously registered waker",
            );
        }

        // Completion consumes and drops the stored waker, releasing its clone and
        // waking exactly the one registration that survived the replacements.
        sender.send(42);
        assert_eq!(Arc::strong_count(&counter), 2);
        assert_eq!(counter.woken.load(Ordering::Relaxed), 1);

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));
        assert_eq!(Arc::strong_count(&counter), 2);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_no_awaiter() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let _endpoints = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let backtrace = unsafe { place.inner.assume_init_ref() }.awaiter_backtrace();

        assert!(backtrace.is_none());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_with_awaiter() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (_sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        let backtrace = unsafe { place.inner.assume_init_ref() }.awaiter_backtrace();

        assert!(backtrace.is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_after_sender_drop() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(sender);

        let backtrace = unsafe { place.inner.assume_init_ref() }.awaiter_backtrace();

        assert!(backtrace.is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_after_receiver_drop() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
        let (_sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(receiver);

        let backtrace = unsafe { place.inner.assume_init_ref() }.awaiter_backtrace();

        assert!(backtrace.is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_outlives_event() {
        let backtrace = {
            let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());
            let (_sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

            let mut cx = task::Context::from_waker(Waker::noop());
            let mut receiver = Box::pin(receiver);
            _ = receiver.as_mut().poll(&mut cx);

            unsafe { place.inner.assume_init_ref() }
                .awaiter_backtrace()
                .expect("the event has been awaited")
        };

        // The event storage is gone but the snapshot remains readable.
        _ = backtrace.status();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn released_event_releases_backtrace() {
        let mut place = Box::pin(EmbeddedLocalEvent::<i32>::new());

        {
            let (_sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

            let mut cx = task::Context::from_waker(Waker::noop());
            let mut receiver = Box::pin(receiver);
            _ = receiver.as_mut().poll(&mut cx);
        }

        // The event has been released but its storage is still ours to inspect. Releasing an
        // event releases its backtrace, because the storage may be reused without dropping it.
        let event = unsafe { place.inner.assume_init_ref() };

        assert!(event.awaiter_backtrace().is_none());
    }

    // Regression test for the synchronous reentrancy hazard in `set`. A waker
    // fired by the sender's `send` that synchronously polls the receiver must
    // observe a terminal state (SET) and read out the value, not see the
    // intermediate state from the +1 collapse. This also exercises the
    // callback-safety contract that the awaiter cell is drained before the
    // wake callback runs.
    #[test]
    #[cfg_attr(miri, ignore)] // Custom raw waker is not Miri-compatible.
    fn boxed_send_with_reentrant_waker_observes_set() {
        type ObservedResult = Poll<Result<i32, Disconnected>>;

        with_watchdog(|| {
            let (sender, receiver) = LocalEvent::<i32>::boxed();
            let receiver_holder: Rc<RefCell<Option<Pin<Box<_>>>>> =
                Rc::new(RefCell::new(Some(Box::pin(receiver))));
            let receiver_for_waker = Rc::clone(&receiver_holder);

            let reentrant_observed: Rc<RefCell<Option<ObservedResult>>> =
                Rc::new(RefCell::new(None));
            let observed_for_waker = Rc::clone(&reentrant_observed);

            let waker_data = ReentrantWakerData::new(move || {
                // Synchronously poll the receiver from inside the waker.
                // The receiver should observe EVENT_SET and return Ready(Ok(42)).
                let mut holder = receiver_for_waker.borrow_mut();
                let receiver = holder.as_mut().expect("receiver still held");
                let noop = Waker::noop();
                let mut cx = task::Context::from_waker(noop);
                let result = receiver.as_mut().poll(&mut cx);
                *observed_for_waker.borrow_mut() = Some(result);
            });
            // SAFETY: `waker_data` outlives the waker and the test is single-threaded.
            let waker = unsafe { waker_data.waker() };

            // First poll transitions BOUND -> AWAITING and stores the
            // reentrant waker.
            {
                let mut holder = receiver_holder.borrow_mut();
                let receiver = holder.as_mut().expect("receiver still held");
                let mut cx = task::Context::from_waker(&waker);
                assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
            }

            // Send a value. This calls `set` which transitions AWAITING -> SET
            // and then invokes the waker, which must observe the SET state
            // and consume the value reentrantly.
            sender.send(42);

            assert!(waker_data.was_woken());
            let observed = reentrant_observed.borrow_mut().take();
            assert!(
                matches!(observed, Some(Poll::Ready(Ok(42)))),
                "reentrant poll should observe SET and read the value",
            );

            // The receiver was consumed reentrantly; the receiver_holder still
            // owns the Pin<Box> shell, drop it to release.
            drop(receiver_holder.borrow_mut().take());
        });
    }

    // Regression test for the cancellation reentrancy hazard in `final_poll`.
    // A stored waker whose destructor reentrantly drops the sender must not
    // cause the event storage to be deallocated while `final_poll` is still
    // executing. `final_poll` must revert the AWAITING state to a non-terminal
    // state before dropping the waker, so the reentrant sender drop observes a
    // live event and defers cleanup instead of freeing storage out from under
    // the active operation. Ref: docs/callback-safety.md; mirrors the sync
    // `Event::final_poll`. Runs under Miri so the protected-pointer
    // deallocation is caught if the ordering regresses.
    #[test]
    fn boxed_receiver_cancel_with_sender_dropping_waker_preserves_storage() {
        // Owns the sender behind a refcounted waker. When the last waker
        // reference is dropped, `Drop` drops the sender, reentrantly entering
        // `sender_dropped_without_set` while `final_poll` is on the stack.
        struct DropSenderOnWakerDrop {
            sender: Option<BoxedLocalSender<i32>>,
        }

        impl Drop for DropSenderOnWakerDrop {
            fn drop(&mut self) {
                drop(self.sender.take());
            }
        }

        unsafe fn clone_raw(data: *const ()) -> RawWaker {
            let rc = unsafe { Rc::from_raw(data.cast::<DropSenderOnWakerDrop>()) };
            let clone = Rc::clone(&rc);
            // Keep the original reference alive; we only wanted to bump the count.
            mem::forget(rc);
            RawWaker::new(Rc::into_raw(clone).cast(), &VTABLE)
        }
        unsafe fn wake_raw(data: *const ()) {
            unsafe { drop_raw(data) }
        }
        unsafe fn wake_by_ref_raw(_data: *const ()) {}
        unsafe fn drop_raw(data: *const ()) {
            unsafe { drop(Rc::from_raw(data.cast::<DropSenderOnWakerDrop>())) }
        }
        static VTABLE: RawWakerVTable =
            RawWakerVTable::new(clone_raw, wake_raw, wake_by_ref_raw, drop_raw);

        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let data = Rc::new(DropSenderOnWakerDrop {
            sender: Some(sender),
        });
        let waker = unsafe { Waker::from_raw(RawWaker::new(Rc::into_raw(data).cast(), &VTABLE)) };

        // First poll transitions BOUND -> AWAITING and stores a clone of the
        // waker inside the event.
        let mut cx = task::Context::from_waker(&waker);
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // Drop our local waker so only the event's stored clone remains; the
        // sender is still owned behind that clone.
        drop(waker);

        // Dropping the receiver cancels the wait: `final_poll` drops the stored
        // waker, whose destructor drops the sender. The event storage must
        // survive until `final_poll` returns.
        drop(receiver);
    }
}
