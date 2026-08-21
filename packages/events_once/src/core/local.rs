use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{MaybeUninit, offset_of};
use std::panic::{RefUnwindSafe, UnwindSafe};
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
///
/// # Reentrancy
///
/// Completing or cancelling an event runs the awaiting task's waker, either waking it or dropping
/// it. Such a callback may operate on the endpoints of the event it belongs to: it may poll the
/// receiver to completion, and it may drop an endpoint - including the last one, which releases
/// the event storage.
///
/// A poll that registers the task for later notification runs the waker's clone operation, which
/// is likewise user-supplied code. Such a callback may operate on the sender endpoint of the event
/// being polled: it may send a value and it may drop the sender. The poll observes the resulting
/// state.
pub struct LocalEvent<T> {
    /// The logical state of the event; see constants in `state.rs`.
    pub(crate) state: Cell<u8>,

    /// Holds the waker of whoever most recently awaited the receiver. The `state` field is what
    /// records whether this field is initialized; see `state.rs`.
    ///
    /// We use `MaybeUninit` to minimize the storage and avoid an `Option` or enum overhead,
    /// as we already track the presence via `state`.
    ///
    /// We use `UnsafeCell` because we are a synchronization primitive and
    /// do our own synchronization of reads/writes.
    awaiter: UnsafeCell<MaybeUninit<Waker>>,

    /// Holds the value that was sent by the sender. The `state` field is what records whether
    /// this field is initialized; see `state.rs`.
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
// inference to mark LocalEvent as !RefUnwindSafe, and a payload that is itself
// !UnwindSafe would additionally make it !UnwindSafe. The event is unwind-safe
// regardless of the payload because every mutation is panic-atomic: the payload
// and the waker are moved into and out of their cells in single moves, the state
// that records which of those cells is initialized is published by a single store
// ordered after the move, and the user-controlled code that can unwind (payload,
// waker and destructor code) only runs where state and storage agree. A caught
// panic therefore leaves the event in a state that the remaining endpoint can
// complete or clean up, which the unwinding-mid-poll regression tests below
// exercise.
impl<T: 'static> UnwindSafe for LocalEvent<T> {}
impl<T: 'static> RefUnwindSafe for LocalEvent<T> {}

impl<T: 'static> LocalEvent<T> {
    /// In-place initializes a new instance in the `BOUND` state.
    ///
    /// This is for internal use only and is wrapped by public methods that also
    /// wire up the sender and receiver after doing the initialization. An event
    /// without a sender and receiver is invalid.
    pub(crate) fn new_in_inner(place: &mut UnsafeCell<MaybeUninit<Self>>) {
        // We can skip initializing the MaybeUninit fields because they start uninitialized
        // by design and the UnsafeCell wrapper is transparent, only affecting accesses and
        // not the contents.
        let base_ptr = place.get_mut().as_mut_ptr();

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
        place: Pin<&mut UnsafeCell<MaybeUninit<Self>>>,
    ) -> (
        LocalSenderCore<PtrLocalRef<T>, T>,
        LocalReceiverCore<PtrLocalRef<T>, T>,
    ) {
        // SAFETY: Nothing is getting moved, we just temporarily unwrap the Pin wrapper.
        let place_mut = unsafe { place.get_unchecked_mut() };

        Self::new_in_inner(place_mut);

        // We cast away the MaybeUninit wrapper because it is now initialized.
        let event = NonNull::from_mut(place_mut).cast::<UnsafeCell<Self>>();

        // SAFETY: The event was just initialized into this storage, the caller of this function
        // guaranteed that the storage stays valid and pinned at this address for the entire
        // lifetime of the endpoints, and the endpoints only ever reach the event through the
        // shared references that the reference policy hands out.
        let sender_event = unsafe { PtrLocalRef::new(event) };

        // SAFETY: Same as above - the second endpoint references the same initialized event in
        // the same caller-guaranteed storage.
        let receiver_event = unsafe { PtrLocalRef::new(event) };

        (
            LocalSenderCore::new(sender_event),
            LocalReceiverCore::new(receiver_event),
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
    /// // SAFETY: `place` is a freshly created container that no other event is using, it stays
    /// // alive and writable until both endpoints below are consumed, and `Box::pin` keeps the
    /// // event at a stable address for that entire time.
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
    pub(crate) fn clear_awaiter_backtrace(event_cell: &UnsafeCell<Self>) {
        // SAFETY: The cell's pointer is always valid and points to an initialized event because
        // the caller is still an endpoint of it. We only ever create shared references to the
        // event through this cell, so no exclusive reference can alias this one.
        let event = unsafe { &*event_cell.get() };

        let mut backtrace = event.backtrace.borrow_mut();

        *backtrace = None;
    }

    /// Sets the value of the event and notifies the awaiter, if there is one.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    ///
    /// The event arrives as an `&UnsafeCell<Self>` rather than as `&self` because the wake
    /// callback performed here is free to release the event storage. A `&self` argument stays
    /// strongly protected by the aliasing model for the whole call, which makes such a release
    /// undefined behavior, whereas references derived from an `UnsafeCell` carry no protector and
    /// may dangle. The event is therefore not touched after the callback runs.
    /// Ref: docs/callback-safety.md.
    #[inline]
    pub(crate) fn set(event_cell: &UnsafeCell<Self>, value: T) -> Result<(), Disconnected> {
        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method.
        let event = unsafe { &*event_cell.get() };

        let value_cell = event.value.get();

        // We can start by setting the value - this has to happen no matter what.
        // Everything else we do here is just to get the awaiter to come pick it up.
        //
        // SAFETY: It is valid for the sender to write here because we know that nobody else will
        // be accessing this field at this time. This is guaranteed by:
        // * There is only one sender and it is single-threaded, so it cannot be used in parallel.
        // * The receiver will only access this field in the "Set" state, which can only be entered
        //   from later on in this method.
        unsafe {
            value_cell.write(MaybeUninit::new(value));
        }

        // We publish the terminal state with a single store, which also collapses the two state
        // writes that the thread-safe variant needs on the awaiting branch (`EVENT_SIGNALING`
        // then `EVENT_SET`). Only the thread-safe variant needs that intermediate; see
        // `state.rs`. In the disconnected branch the state we store here is a don't-care because
        // the event is about to be released, and no external observer can witness it.
        let previous_state = event.state.replace(EVENT_SET);

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
                    let awaiter_cell_maybe = unsafe { event.awaiter.get().as_mut() };
                    // SAFETY: UnsafeCell pointer is never null.
                    let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                    // We extract the waker and consider the field uninitialized again.
                    // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker
                    // in there.
                    unsafe { awaiter_cell.assume_init_read() }
                };

                // Come and get it. The event is in a terminal state, so the woken receiver may
                // reentrantly complete and release the event storage. We touch nothing in the
                // event after this point.
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
                let value_cell_maybe = unsafe { event.value.get().as_mut() };
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
                unreachable!(
                    "unreachable {} state on set: {previous_state}",
                    type_name::<Self>()
                );
            }
        }
    }

    /// We are intended to be polled via `Future::poll`, so we have a similar signature here,
    /// with `None` equating to `Poll::Pending`.
    ///
    /// If `Some` result is returned, the caller is the last remaining endpoint and responsible
    /// for cleaning up the event. It must do so immediately, without running any user-controlled
    /// code in between and without any further state-based cleanup, because the event may be in
    /// the transient state described on [`poll_set()`][Self::poll_set].
    #[inline]
    #[must_use]
    pub(crate) fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        #[cfg(debug_assertions)]
        self.backtrace.replace(Some(capture_backtrace()));

        match self.state.get() {
            EVENT_BOUND => self.poll_bound(waker),
            EVENT_SET => Some(Ok(self.poll_set())),
            EVENT_AWAITING => self.poll_awaiting(waker),
            EVENT_DISCONNECTED => {
                // There is no result coming, ever! This is the end.
                Some(Err(Disconnected))
            }
            // Defensive: state machine guarantees this is unreachable.
            state => {
                unreachable!("unreachable {} state on poll: {state}", type_name::<Self>());
            }
        }
    }

    /// `poll()` impl for the `EVENT_BOUND` state.
    #[must_use]
    fn poll_bound(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        // The sender has not yet set any value, so we will have to wait.

        // `Waker::clone` runs arbitrary user code, which is free to operate on the sender endpoint
        // of this same event and thereby move the event into a terminal state. We therefore clone
        // before touching the event and consult the state again afterwards. This mirrors the
        // compare-exchange in the sync `Event::poll_bound`: reentrancy hands the single-threaded
        // variant the same interleavings that concurrency hands the thread-safe one.
        // Ref: docs/callback-safety.md.
        let new_waker = waker.clone();

        match self.state.get() {
            EVENT_BOUND => {
                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                awaiter_cell.write(new_waker);

                // The sender will wake us up when it has set the value.
                self.state.set(EVENT_AWAITING);
                None
            }
            EVENT_SET => {
                // A reentrant sender delivered the value while we were cloning the waker. Nobody
                // would ever consume a registration made now, so we release the clone instead.
                // Releasing it runs user code, which must not see the event mid-extraction: the
                // value comes out only once no callback can run, so an unwinding destructor leaves
                // `EVENT_SET` still backed by an initialized value for the receiver to clean up.
                // Ref: docs/callback-safety.md.
                drop(new_waker);

                Some(Ok(self.poll_set()))
            }
            EVENT_DISCONNECTED => {
                // A reentrant sender drop disconnected while we were cloning the waker, so there
                // is no result coming, ever. We release the clone that nobody would consume.
                drop(new_waker);

                Some(Err(Disconnected))
            }
            // Defensive: only the receiver enters EVENT_AWAITING and it is right here, so the
            // state machine guarantees this is unreachable.
            state => {
                unreachable!(
                    "unreachable {} state on poll of bound event: {state}",
                    type_name::<Self>()
                );
            }
        }
    }

    /// `poll()` impl for the `EVENT_SET` state.
    ///
    /// Extracts the payload and leaves the value cell uninitialized while `state` still says
    /// `EVENT_SET`. That is the window which `state.rs` ("Field initialization") requires the
    /// caller to close, so two obligations follow: every user-controlled callback (waker clone,
    /// drop or wake) must have finished before entry, and the caller must release the event on
    /// return without any further state-driven cleanup. Otherwise a callback that unwinds into
    /// receiver cleanup reaches `final_poll()`, which trusts `EVENT_SET` and would read the
    /// moved-out payload a second time. Ref: docs/callback-safety.md.
    #[must_use]
    fn poll_set(&self) -> T {
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
        unsafe { value_cell.assume_init_read() }
    }

    /// `poll()` impl for the `EVENT_AWAITING` state.
    #[must_use]
    fn poll_awaiting(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        // We are re-polling after previously starting a wait. This is fine
        // and we just need to clean up the previous waker, replacing it with
        // a new one. The previous registration must be released exactly once;
        // overwriting it without dropping would leak a `Waker` reference.

        // Clone the incoming waker before touching the cell. `Waker::clone` runs arbitrary user
        // code, so it must not observe the cell in the transiently-uninitialized window between
        // the read and the write below. That code is also free to operate on the sender endpoint
        // of this same event, which moves the event into a terminal state and takes the
        // previously registered waker along the way, so we consult the state again afterwards.
        // Ref: docs/callback-safety.md.
        let new_waker = waker.clone();

        match self.state.get() {
            EVENT_AWAITING => {
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
                // A reentrant sender that completes the event from this destructor takes the
                // registration we just made and wakes it, so the caller is still told to come
                // back even though we report being pending. We touch nothing in the event after
                // this point.
                drop(previous_waker);

                None
            }
            EVENT_SET => {
                // A reentrant sender delivered the value while we were cloning the waker, taking
                // the previous registration with it. See the equivalent arm of `poll_bound()` for
                // why the unused clone is released before the value comes out.
                drop(new_waker);

                Some(Ok(self.poll_set()))
            }
            EVENT_DISCONNECTED => {
                // A reentrant sender drop disconnected while we were cloning the waker, taking
                // the previous registration with it. There is no result coming, ever.
                drop(new_waker);

                Some(Err(Disconnected))
            }
            // Defensive: only the receiver leaves EVENT_AWAITING for a non-terminal state and it
            // is right here, so the state machine guarantees this is unreachable.
            state => {
                unreachable!(
                    "unreachable {} state on poll of awaited event: {state}",
                    type_name::<Self>()
                );
            }
        }
    }

    /// Marks the event as having been disconnected early from the sender side.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    ///
    /// See [`set()`][Self::set] for why the event arrives as an `&UnsafeCell<Self>`.
    #[inline]
    pub(crate) fn sender_dropped_without_set(
        event_cell: &UnsafeCell<Self>,
    ) -> Result<(), Disconnected> {
        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method.
        let event = unsafe { &*event_cell.get() };

        let previous_state = event.state.get();

        // We can immediately set this because this is a single-threaded event, so there cannot
        // be any race condition causing us issues with the receiver seeing this too early.
        event.state.set(EVENT_DISCONNECTED);

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
                let awaiter_cell_maybe = unsafe { event.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                // We extract the waker and consider the field uninitialized again.
                // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
                let waker = unsafe { awaiter_cell.assume_init_read() };

                // Come and get it. The event is in a terminal state, so the woken receiver may
                // reentrantly complete and release the event storage. We touch nothing in the
                // event after this point.
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
                unreachable!(
                    "unreachable {} state on sender disconnect: {previous_state}",
                    type_name::<Self>()
                );
            }
        }
    }

    /// Whether the event has reached a terminal state, meaning `EVENT_SET` or
    /// `EVENT_DISCONNECTED` (see `state.rs`). A terminal state is what makes an outcome - a value
    /// or a disconnect - immediately retrievable.
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
    ///
    /// See [`set()`][Self::set] for why the event arrives as an `&UnsafeCell<Self>`; here it is
    /// the waker's destructor rather than `wake()` that may release the event storage.
    #[inline]
    pub(crate) fn final_poll(event_cell: &UnsafeCell<Self>) -> Result<Option<T>, Disconnected> {
        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method.
        let event = unsafe { &*event_cell.get() };

        // If we are still awaiting, the receiver (the caller) owns the stored waker and must
        // destroy it before disconnecting. We revert to EVENT_BOUND *before* dropping the waker
        // so that a reentrant sender drop triggered by the waker's destructor observes a live,
        // non-terminal event and defers cleanup, rather than deallocating the storage we are
        // about to read from again. Only once the waker is gone do we read the resulting state
        // and transition to EVENT_DISCONNECTED.
        //
        // This mirrors the sync `Event::final_poll`, which reverts AWAITING -> BOUND before
        // destroying the awaiter, and follows docs/callback-safety.md ("No callbacks under
        // borrows of shared state"; symmetric handoff vs. cleanup ordering).
        if event.state.get() == EVENT_AWAITING {
            event.state.set(EVENT_BOUND);

            // SAFETY: The only other potential references to the field are other short-lived
            // references in this type, which cannot exist at the moment because
            // the type is single-threaded and does not let any references escape.
            let awaiter_cell_maybe = unsafe { event.awaiter.get().as_mut() };
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
        let previous_state = event.state.get();

        // We can immediately set this because this is a single-threaded event, so there cannot
        // be any race condition causing us issues with the receiver seeing this too early.
        event.state.set(EVENT_DISCONNECTED);

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
                let value_cell_maybe = unsafe { event.value.get().as_mut() };
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
                    "unreachable {} state on receiver disconnect: {previous_state}",
                    type_name::<Self>()
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::mem;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::task::{self, Poll};

    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::{
        DropOnWakerRelease, ReentrantWakerData, assert_panics_with, clone_action_waker,
        clone_action_waker_panicking_on_clone_release, drop_waker, with_watchdog,
    };

    use super::*;
    use crate::IntoValueError;

    assert_not_impl_any!(LocalEvent<i32>: Send, Sync);

    // The payload is one that is itself neither `UnwindSafe` nor `RefUnwindSafe`, so the
    // assertion can only pass if the event supplies both regardless of what it carries.
    assert_impl_all!(LocalEvent<Rc<RefCell<u32>>>: UnwindSafe, RefUnwindSafe);

    /// Places an event into freshly allocated pinned storage, returning the storage together
    /// with the endpoints, so that every test does not have to repeat the placement proof.
    ///
    /// The storage comes first in the returned tuple, which makes it outlive the endpoints
    /// bound alongside it.
    fn placed<T: 'static>() -> (
        Pin<Box<EmbeddedLocalEvent<T>>>,
        RawLocalSender<T>,
        RawLocalReceiver<T>,
    ) {
        let mut place = Box::pin(EmbeddedLocalEvent::<T>::new());

        // SAFETY: The container was created right here, so no other event is using it. It is
        // returned to the caller alongside the endpoints and is dropped after them, so it stays
        // alive and writable for their entire lifetime, and `Box::pin` keeps the event at a
        // stable address for that time.
        let (sender, receiver) = unsafe { LocalEvent::placed(place.as_mut()) };

        (place, sender, receiver)
    }

    /// Reads the event out of the storage it was placed into, so a test can inspect state that
    /// the endpoints do not expose.
    ///
    /// The only such state is the diagnostic backtrace, which exists in debug builds only.
    #[cfg(debug_assertions)]
    fn placed_event<T: 'static>(place: &EmbeddedLocalEvent<T>) -> &LocalEvent<T> {
        // SAFETY: The container is only ever accessed through shared references, matching the
        // access that the endpoints make, and the pointer of an `UnsafeCell` is never null.
        let event = unsafe { &*place.inner.get() };

        // SAFETY: The caller obtained this container from `placed()`, which initialized an event
        // into it. Releasing an event does not deinitialize the storage.
        unsafe { event.assume_init_ref() }
    }

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
        let (_place, sender, receiver) = placed::<i32>();
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_receive_send_receive() {
        let (_place, sender, receiver) = placed::<i32>();
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
        let (_place, sender, _) = placed::<i32>();

        sender.send(42);
    }

    #[test]
    fn placed_drop_receive() {
        let (_place, _, receiver) = placed::<i32>();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_receive_drop_receive() {
        let (_place, sender, receiver) = placed::<i32>();
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
        let (_place, sender, receiver) = placed::<i32>();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);

        sender.send(42);
    }

    #[test]
    fn placed_receive_drop_drop_receiver_first() {
        let (_place, sender, receiver) = placed::<i32>();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_receive_drop_drop_sender_first() {
        let (_place, sender, receiver) = placed::<i32>();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_drop_drop_receiver_first() {
        let (_place, sender, receiver) = placed::<i32>();

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_drop_drop_sender_first() {
        let (_place, sender, receiver) = placed::<i32>();

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_is_ready() {
        let (_place, sender, receiver) = placed::<i32>();
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
        let (_place, sender, receiver) = placed::<i32>();
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
        let (_place, sender, receiver) = placed::<i32>();

        let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(42)));
    }

    #[test]
    fn placed_drop_into_value() {
        let (_place, sender, receiver) = placed::<i32>();

        drop(sender);

        assert!(matches!(
            receiver.into_value(),
            Err(IntoValueError::Disconnected)
        ));
    }

    #[test]
    #[should_panic]
    fn placed_panic_poll_after_completion() {
        let (_place, sender, receiver) = placed::<i32>();
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
        let (_place, sender, receiver) = placed::<i32>();
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
        let (place, _sender, _receiver) = placed::<i32>();

        let backtrace = placed_event(&place).awaiter_backtrace();

        assert!(backtrace.is_none());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_with_awaiter() {
        let (place, _sender, receiver) = placed::<i32>();

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        let backtrace = placed_event(&place).awaiter_backtrace();

        assert!(backtrace.is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_after_sender_drop() {
        let (place, sender, receiver) = placed::<i32>();

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(sender);

        let backtrace = placed_event(&place).awaiter_backtrace();

        assert!(backtrace.is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_after_receiver_drop() {
        let (place, _sender, receiver) = placed::<i32>();

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(receiver);

        let backtrace = placed_event(&place).awaiter_backtrace();

        assert!(backtrace.is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn awaiter_backtrace_outlives_event() {
        let backtrace = {
            let (place, _sender, receiver) = placed::<i32>();

            let mut cx = task::Context::from_waker(Waker::noop());
            let mut receiver = Box::pin(receiver);
            _ = receiver.as_mut().poll(&mut cx);

            placed_event(&place)
                .awaiter_backtrace()
                .expect("the event has been awaited")
        };

        // The event storage is gone but the snapshot remains readable.
        _ = backtrace.status();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn released_event_releases_backtrace() {
        let (place, sender, receiver) = placed::<i32>();

        {
            // Both endpoints go out of scope at the end of this block, which releases the event
            // while its storage stays ours.
            let _sender = sender;
            let mut cx = task::Context::from_waker(Waker::noop());
            let mut receiver = Box::pin(receiver);
            _ = receiver.as_mut().poll(&mut cx);
        }

        // The event has been released but its storage is still ours to inspect. Releasing an
        // event releases its backtrace, because the storage may be reused without dropping it.
        let event = placed_event(&place);

        assert!(event.awaiter_backtrace().is_none());
    }

    // Regression test for the synchronous reentrancy hazard in `set`. The wake callback that
    // `set` fires may poll the receiver synchronously, and it must find the event fully
    // published: the value stored, the terminal state committed and the awaiter slot emptied.
    // The local variant reaches that state with a single state write and never publishes the
    // signaling state that the thread-safe variant uses (see `state.rs`), so the callback must
    // observe `EVENT_SET` and read out the value. Ref: docs/callback-safety.md.
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

    // Parity counterpart of the `set` case above. A waker fired by the sender
    // drop that synchronously polls the receiver must likewise observe a
    // terminal state, here DISCONNECTED.
    #[test]
    #[cfg_attr(miri, ignore)] // Custom raw waker is not Miri-compatible.
    fn boxed_sender_drop_with_reentrant_waker_observes_disconnected() {
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

            // Dropping the sender transitions AWAITING -> DISCONNECTED and then
            // invokes the waker, which must observe the terminal state.
            drop(sender);

            assert!(waker_data.was_woken());
            let observed = reentrant_observed.borrow_mut().take();
            assert!(
                matches!(observed, Some(Poll::Ready(Err(Disconnected)))),
                "reentrant poll should observe DISCONNECTED",
            );

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
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let (data, sender_dropped) = DropOnWakerRelease::new(sender);
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let waker = unsafe { drop_waker(data) };

        // First poll transitions BOUND -> AWAITING and stores a clone of the
        // waker inside the event.
        let mut cx = task::Context::from_waker(&waker);
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // Drop our local waker so only the event's stored clone remains; the
        // sender is still owned behind that clone.
        drop(waker);
        assert!(!sender_dropped.get());

        // Dropping the receiver cancels the wait: `final_poll` drops the stored
        // waker, whose destructor drops the sender. The event storage must
        // survive until `final_poll` returns.
        drop(receiver);

        assert!(sender_dropped.get(), "{REENTRANCY_REQUIRED}");
    }

    // Regression test for the reentrancy hazard in `set`. The wake callback
    // fired by the sender is free to drop the receiver, which completes the
    // event and releases its storage while `set` is still on the stack. `set`
    // must therefore reach the event through an `UnsafeCell` and must not touch
    // it after waking. Ref: docs/callback-safety.md. Runs under Miri so the
    // protected-pointer deallocation is caught if the shape regresses.
    #[test]
    fn boxed_send_with_reentrant_receiver_drop_releases_storage() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        let (data, receiver_dropped) = DropOnWakerRelease::new(Box::pin(receiver));
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let waker = unsafe { drop_waker(Arc::clone(&data)) };

        // First poll transitions BOUND -> AWAITING and stores a clone of the
        // waker inside the event.
        data.with_value(|receiver| {
            let mut cx = task::Context::from_waker(&waker);
            assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
        });

        // Leave the event's stored clone as the only reference, so waking it
        // runs the reentrant drop.
        drop(waker);
        drop(data);
        assert!(!receiver_dropped.get());

        // `set` stores the value, transitions AWAITING -> SET and wakes, which
        // drops the receiver, which consumes the value and frees the event.
        sender.send(42);

        assert!(receiver_dropped.get(), "{REENTRANCY_REQUIRED}");
    }

    // Regression test for the reentrancy hazard in `sender_dropped_without_set`.
    // Identical in shape to the `set` case above, except that the receiver
    // observes DISCONNECTED instead of SET.
    #[test]
    fn boxed_sender_drop_with_reentrant_receiver_drop_releases_storage() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        let (data, receiver_dropped) = DropOnWakerRelease::new(Box::pin(receiver));
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let waker = unsafe { drop_waker(Arc::clone(&data)) };

        data.with_value(|receiver| {
            let mut cx = task::Context::from_waker(&waker);
            assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));
        });

        drop(waker);
        drop(data);
        assert!(!receiver_dropped.get());

        // `sender_dropped_without_set` transitions AWAITING -> DISCONNECTED and
        // wakes, which drops the receiver, which frees the event.
        drop(sender);

        assert!(receiver_dropped.get(), "{REENTRANCY_REQUIRED}");
    }

    /// Explains a failure to reach the reentrant drop that a regression test exists to exercise.
    /// Without it the test would pass no matter how the code under test behaved.
    const REENTRANCY_REQUIRED: &str =
        "the event must have held a waker clone whose drop reentered the operation under test";

    /// Explains a failure to reach the reentrant clone that a regression test exists to exercise.
    /// Without it the test would pass no matter how the code under test behaved.
    const WAKER_CLONE_REQUIRED: &str =
        "the poll must have cloned the waker it was given, which is what re-enters the sender";

    // Regression tests for the reentrancy hazard in `poll`. Registering an awaiter clones the
    // waker, which is user code that may operate on the sender endpoint of the same event and
    // move it into a terminal state. The poll must observe the state the clone left behind
    // instead of overwriting it with EVENT_AWAITING, which would strand the receiver and lose
    // the result. Ref: docs/callback-safety.md.
    #[test]
    fn boxed_poll_with_reentrant_send_during_waker_clone_observes_set() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || sender.send(42)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_poll_with_reentrant_sender_drop_during_waker_clone_observes_disconnected() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || drop(sender)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    // The same hazard on the re-poll path, where the reentrant sender additionally consumes the
    // previously registered waker on its way to the terminal state.
    #[test]
    fn boxed_repoll_with_reentrant_send_during_waker_clone_observes_set() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        // First poll transitions BOUND -> AWAITING and registers a waker for the sender to take.
        let mut cx = task::Context::from_waker(Waker::noop());
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || sender.send(42)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_repoll_with_reentrant_sender_drop_during_waker_clone_observes_disconnected() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) = unsafe { clone_action_waker(move || drop(sender)) };

        let mut cx = task::Context::from_waker(&waker);
        let poll_result = receiver.as_mut().poll(&mut cx);

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    // The re-poll path releases the registration made by the previous poll, which is user code
    // that may drop the sender and thereby complete the event from inside the re-poll. The
    // replacement registration is written before that destructor runs, so the completion finds it
    // and wakes it. Reporting pending therefore loses no wakeup.
    // Ref: docs/callback-safety.md.
    #[test]
    #[cfg_attr(miri, ignore)] // Custom raw waker is not Miri-compatible.
    fn boxed_repoll_with_reentrant_sender_drop_during_previous_waker_release_wakes_new_waker() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let (data, sender_dropped) = DropOnWakerRelease::new(sender);
        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let previous_waker = unsafe { drop_waker(data) };

        // First poll transitions BOUND -> AWAITING and registers a clone of `previous_waker`.
        let mut cx = task::Context::from_waker(&previous_waker);
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        // Release our own reference so the registration inside the event is the last one and
        // dropping it drops the sender it carries.
        drop(previous_waker);
        assert!(!sender_dropped.get());

        let new_waker_data = ReentrantWakerData::new(|| {});
        // SAFETY: `new_waker_data` outlives the waker and the test is single-threaded.
        let new_waker = unsafe { new_waker_data.waker() };

        let mut cx = task::Context::from_waker(&new_waker);
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        assert!(sender_dropped.get(), "{REENTRANCY_REQUIRED}");
        assert!(
            new_waker_data.was_woken(),
            "the completion must have reached the registration made by this poll"
        );

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Err(Disconnected))
        ));
    }

    /// Counts its own drops, so a test can tell a value that was delivered exactly once apart from
    /// one that was extracted twice or lost.
    struct DropCounter {
        drops: Rc<Cell<usize>>,
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.drops.set(self.drops.get().saturating_add(1));
        }
    }

    // Releasing the waker clone that a reentrant send made useless is user code, so it may unwind.
    // The value must still be inside the event at that moment: a poll that extracts the value
    // first leaves EVENT_SET backed by an uninitialized cell, which the receiver's own cleanup
    // then reads a second time. Ref: docs/callback-safety.md.
    #[test]
    fn boxed_poll_unwinding_during_waker_clone_release_leaves_value_in_event() {
        let drops = Rc::new(Cell::new(0_usize));

        let (sender, receiver) = LocalEvent::<DropCounter>::boxed();
        let mut receiver = Box::pin(receiver);

        let value = DropCounter {
            drops: Rc::clone(&drops),
        };

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) =
            unsafe { clone_action_waker_panicking_on_clone_release(move || sender.send(value)) };

        let mut cx = task::Context::from_waker(&waker);
        assert_panics_with(
            || receiver.as_mut().poll(&mut cx),
            |message| assert!(message.contains("waker clone release")),
        );

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");

        if drops.get() != 0 {
            // The poll extracted the value before running the callback, so the event now claims a
            // value it no longer holds. Letting the receiver clean up would read that cell again,
            // so leak the event instead of escalating the failure into undefined behavior.
            mem::forget(receiver);

            panic!("the value must stay in the event while a callback can still unwind past it");
        }

        // The event still owns the value, so the receiver's cleanup delivers exactly one drop.
        drop(receiver);

        assert_eq!(drops.get(), 1);
    }

    // The same hazard on the re-poll path, which releases the useless clone through the same arm.
    #[test]
    fn boxed_repoll_unwinding_during_waker_clone_release_leaves_value_in_event() {
        let drops = Rc::new(Cell::new(0_usize));

        let (sender, receiver) = LocalEvent::<DropCounter>::boxed();
        let mut receiver = Box::pin(receiver);

        // First poll transitions BOUND -> AWAITING and registers a waker for the sender to take.
        let mut cx = task::Context::from_waker(Waker::noop());
        assert!(matches!(receiver.as_mut().poll(&mut cx), Poll::Pending));

        let value = DropCounter {
            drops: Rc::clone(&drops),
        };

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let (waker, cloned) =
            unsafe { clone_action_waker_panicking_on_clone_release(move || sender.send(value)) };

        let mut cx = task::Context::from_waker(&waker);
        assert_panics_with(
            || receiver.as_mut().poll(&mut cx),
            |message| assert!(message.contains("waker clone release")),
        );

        assert!(cloned.get(), "{WAKER_CLONE_REQUIRED}");

        if drops.get() != 0 {
            mem::forget(receiver);

            panic!("the value must stay in the event while a callback can still unwind past it");
        }

        drop(receiver);

        assert_eq!(drops.get(), 1);
    }
}
