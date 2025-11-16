#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::hint::spin_loop;
use std::marker::PhantomPinned;
use std::mem::{MaybeUninit, offset_of};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicU8};
use std::task::Waker;

#[cfg(debug_assertions)]
use parking_lot::Mutex;

#[cfg(debug_assertions)]
use crate::{BacktraceType, capture_backtrace};
use crate::{
    BoxedReceiver, BoxedRef, BoxedSender, Disconnected, EVENT_AWAITING, EVENT_BOUND,
    EVENT_DISCONNECTED, EVENT_SET, EVENT_SIGNALING, EmbeddedEvent, PtrRef, RawReceiver, RawSender,
    ReceiverCore, SenderCore,
};

/// Coordinates delivery of a `T` at most once from a sender to a receiver on any thread.
pub struct Event<T>
where
    T: Send + 'static,
{
    /// The logical state of the event; see constants in `state.rs`.
    pub(crate) state: AtomicU8,

    /// If `state` is [`EVENT_AWAITING`] or [`EVENT_SIGNALING`], this field is initialized with the
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
    backtrace: Mutex<Option<BacktraceType>>,

    // It is invalid to move this type once it has been pinned because
    // the sender and receiver connect via raw pointers to the event.
    _requires_pinning: PhantomPinned,
}

impl<T> Event<T>
where
    T: Send + 'static,
{
    /// In-place initializes a new instance in the `BOUND` state.
    ///
    /// This is for internal use only and is wrapped by public methods that also
    /// wire up the sender and receiver after doing the initialization. An event
    /// without a sender and receiver is invalid.
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    pub(crate) fn new_in_inner(place: &mut UnsafeCell<MaybeUninit<Self>>) {
        // The key here is that we can skip initializing the MaybeUninit fields because
        // they start uninitialized by design and the UnsafeCell wrapper is transparent,
        // only affecting accesses and not the contents.
        let base_ptr = place.get_mut().as_mut_ptr();

        // SAFETY: We are making a pointer to a known field at a compiler-guaranteed offset.
        let state_ptr = unsafe { base_ptr.byte_add(offset_of!(Self, state)) }.cast::<AtomicU8>();

        // SAFETY: This is the matching field of the type we are initializing, so valid for writes.
        unsafe {
            state_ptr.write(AtomicU8::new(EVENT_BOUND));
        }

        #[cfg(debug_assertions)]
        {
            // SAFETY: We are making a pointer to a known field at a compiler-guaranteed offset.
            let backtrace_ptr = unsafe { base_ptr.byte_add(offset_of!(Self, backtrace)) }
                .cast::<Mutex<Option<BacktraceType>>>();

            // SAFETY: This is the matching field of the type we are initializing, so valid for writes.
            unsafe {
                backtrace_ptr.write(Mutex::new(None));
            }
        }
    }

    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub(crate) fn boxed_core() -> (SenderCore<BoxedRef<T>, T>, ReceiverCore<BoxedRef<T>, T>) {
        let (sender_event_ref, receiver_event_ref) = BoxedRef::new_pair();

        (
            SenderCore::new(sender_event_ref),
            ReceiverCore::new(receiver_event_ref),
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
    /// use events_once::Event;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let (sender, receiver) = Event::<String>::boxed();
    ///
    /// sender.send("Hello, world!".to_string());
    ///
    /// let message = receiver.await.unwrap();
    /// assert_eq!(message, "Hello, world!");
    /// # }
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub fn boxed() -> (BoxedSender<T>, BoxedReceiver<T>) {
        let (sender_core, receiver_core) = Self::boxed_core();

        (
            BoxedSender::new(sender_core),
            BoxedReceiver::new(receiver_core),
        )
    }

    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub(crate) unsafe fn placed_core(
        place: Pin<&mut UnsafeCell<MaybeUninit<Self>>>,
    ) -> (SenderCore<PtrRef<T>, T>, ReceiverCore<PtrRef<T>, T>) {
        // SAFETY: Nothing is getting moved, we just temporarily unwrap the Pin wrapper.
        let place_mut = unsafe { place.get_unchecked_mut() };

        Self::new_in_inner(place_mut);

        // We cast away the MaybeUninit wrapper because it is now initialized.
        let event = NonNull::from(place_mut).cast::<UnsafeCell<Self>>();

        (
            SenderCore::new(PtrRef::new(event)),
            ReceiverCore::new(PtrRef::new(event)),
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
    /// use events_once::{EmbeddedEvent, Event};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut place = Box::pin(EmbeddedEvent::<String>::new());
    ///
    /// // SAFETY: We promise that `place` lives longer than the endpoints.
    /// let (sender, receiver) = unsafe { Event::placed(place.as_mut()) };
    ///
    /// sender.send("Hello from embedded event!".to_string());
    ///
    /// let message = receiver.await.unwrap();
    /// assert_eq!(message, "Hello from embedded event!");
    /// # }
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub unsafe fn placed(place: Pin<&mut EmbeddedEvent<T>>) -> (RawSender<T>, RawReceiver<T>) {
        // SAFETY: Not moving anything, just breaking through the public wrapper API.
        let place = unsafe { place.map_unchecked_mut(|container| &mut container.inner) };

        // SAFETY: Forwarding safety guarantees from the caller.
        let (sender_core, receiver_core) = unsafe { Self::placed_core(place) };

        (RawSender::new(sender_core), RawReceiver::new(receiver_core))
    }

    /// Uses the provided closure to inspect the backtrace of the current awaiter,
    /// if there is an awaiter and if backtrace capturing is enabled.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure receives `None` if no one is awaiting the event.
    #[cfg(debug_assertions)]
    pub(crate) fn inspect_awaiter(&self, f: impl FnOnce(Option<&Backtrace>)) {
        let backtrace = self.backtrace.lock();
        f(backtrace.as_ref());
    }

    /// Sets the value of the event and notifies the receiver's awaiter, if there is one.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    pub(crate) fn set(event_cell: &UnsafeCell<Self>, value: T) -> Result<(), Disconnected> {
        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method.
        let event_maybe = unsafe { event_cell.get().as_ref() };
        // SAFETY: UnsafeCell pointer is never null.
        let event = unsafe { event_maybe.unwrap_unchecked() };

        let value_cell = event.value.get();

        // We can start by setting the value - this has to happen no matter what.
        // Everything else we do here is just to get the awaiter to come pick it up.
        //
        // SAFETY: It is valid for the sender to write here because we know that nobody else will
        // be accessing this field at this time. This is guaranteed by:
        // * There is only one sender and it is !Sync, so it cannot be used in parallel.
        // * The receiver will only access this field in the "Set" state, which can only be entered
        //   from later on in this method.
        unsafe {
            value_cell.write(MaybeUninit::new(value));
        }

        // A "set" operation is always a state increment. See `state.rs`.
        // We use `Release` ordering for the write because we are
        // releasing the synchronization block of `value`.
        //
        // It is legal to enter SET state here, permitting dealloc, even though we still
        // hold an &event reference here, which ordinarily means it is still illegal to
        // deallocate the object. However, the dangling reference in this method is an
        // `UnsafeCell` which is allowed to dangle, so we are fine even if the object is
        // immediately destroyed after this line.
        let previous_state = event.state.fetch_add(1, atomic::Ordering::Release);

        match previous_state {
            EVENT_BOUND => {
                // Current state: EVENT_SET
                // There was nobody polling via the receiver - our work here is done.
                // The receiver was still connected, so it will clean up the event.
                Ok(())
            }
            EVENT_AWAITING => {
                // Current state: EVENT_SIGNALING
                // There was someone polling via the receiver. We need to
                // notify the awaiter that they can come back for the value now.

                // We need to acquire the synchronization block for the `awaiter`.
                atomic::fence(atomic::Ordering::Acquire);

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // we are in the `EVENT_SIGNALING` state that acts as a mutex to block access
                // to the `awaiter` field.
                let awaiter_cell_maybe = unsafe { event.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                // We extract the waker and consider the field uninitialized again.
                // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
                let waker = unsafe { awaiter_cell.assume_init_read() };

                // Before we send the wake signal we must transition into the `EVENT_SET` state
                // so that the receiver can directly pick up the result when it comes back.
                //
                // We use Release ordering because we are releasing the synchronization block of
                // the `awaiter`. Note that `value` was already released by `fetch_add()` above.
                //
                // It is legal to enter SET state here, permitting dealloc, even though we still
                // hold an &event reference here, which ordinarily means it is still illegal to
                // deallocate the object. However, the dangling reference in this method is an
                // `UnsafeCell` which is allowed to dangle, so we are fine even if the object is
                // immediately destroyed after this line.
                event.state.store(EVENT_SET, atomic::Ordering::Release);

                // Come and get it.
                //
                // As the event is multithreaded, the receiver may already have returned to us
                // before we send this wake signal - that is fine. If that happens, this signal
                // is simply a no-op.
                waker.wake();

                // The receiver was still connected, so it will clean up the event.
                Ok(())
            }
            EVENT_DISCONNECTED => {
                // The receiver has already been dropped, so we need to clean up the event.
                // We have to first drop the value that we inserted into the event, though.

                // SAFETY: The receiver is gone - there is nobody else who might be touching
                // the event anymore, we are essentially in a single-threaded mode now.
                // We also just inserted the value, so it must still be there because we never
                // entered a state where the receiver had the permission to extract the value.
                unsafe {
                    event.destroy_value();
                }

                // Before it is safe to destroy the event, we need to synchronize with whatever
                // writes the receiver may have done into its state (e.g. it may have removed
                // its waker before it marked the event as disconnected).
                atomic::fence(atomic::Ordering::Acquire);

                // The sender (the caller) needs to clean up the event.
                Err(Disconnected)
            }
            _ => {
                unreachable!("unreachable Event state on set: {previous_state}");
            }
        }
    }

    /// Marks the event as having been disconnected early from the sender side.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    pub(crate) fn sender_dropped_without_set(
        event_cell: &UnsafeCell<Self>,
    ) -> Result<(), Disconnected> {
        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method.
        let event_maybe = unsafe { event_cell.get().as_ref() };
        // SAFETY: UnsafeCell pointer is never null.
        let event = unsafe { event_maybe.unwrap_unchecked() };

        // We first need to switch into the SIGNALING state, which acquires exclusive access
        // of the awaiter field, so we can send the wake signal if there is an awaiter.
        // Only after that can we transition into the DISCONNECTED state (because that must
        // be our last action - the receiver may clean up the event at any point after we do that).

        let previous_state = event.state.swap(EVENT_SIGNALING, atomic::Ordering::Relaxed);

        match previous_state {
            EVENT_BOUND => {
                // There was nobody polling via the receiver - our work here is done.

                // It is legal to set DISCONNECTED here, permitting dealloc, even though we still
                // hold an &event reference here, which ordinarily means it is still illegal to
                // deallocate the object. However, the dangling reference in this method is an
                // `UnsafeCell` which is allowed to dangle, so we are fine even if the object is
                // immediately destroyed after this line.
                event
                    .state
                    .store(EVENT_DISCONNECTED, atomic::Ordering::Release);

                // The receiver is the last endpoint remaining, so it will clean up.
                Ok(())
            }
            EVENT_AWAITING => {
                // There is an awaiter that we need to wake up.

                // We need to acquire the synchronization block for the `awaiter`.
                atomic::fence(atomic::Ordering::Acquire);

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // we are in the `EVENT_SIGNALING` state that acts as a mutex to block access
                // to the `awaiter` field.
                let awaiter_cell_maybe = unsafe { event.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                // We extract the waker and consider the field uninitialized again.
                // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
                let waker = unsafe { awaiter_cell.assume_init_read() };

                // Come and get it.
                //
                // As the event is multithreaded, the receiver may already have returned to us
                // before we send this wake signal - that is fine. If that happens, this signal
                // is simply a no-op.
                waker.wake();

                // It is legal to set DISCONNECTED here, permitting dealloc, even though we still
                // hold an &event reference here, which ordinarily means it is still illegal to
                // deallocate the object. However, the dangling reference in this method is an
                // `UnsafeCell` which is allowed to dangle, so we are fine even if the object is
                // immediately destroyed after this line.
                event
                    .state
                    .store(EVENT_DISCONNECTED, atomic::Ordering::Release);

                // The receiver is the last endpoint remaining, so it will clean up.
                Ok(())
            }
            EVENT_DISCONNECTED => {
                // We are the last endpoint remaining, so we will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!("unreachable Event state on sender disconnect: {previous_state}");
            }
        }
    }

    /// We are intended to be polled via `Future::poll`, so we have an equivalent signature here.
    ///
    /// If `Some` is returned, the caller is the last remaining endpoint and responsible
    /// for cleaning up the event.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    pub(crate) fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        #[cfg(debug_assertions)]
        self.backtrace.lock().replace(capture_backtrace());

        // We use Acquire because we are (depending on the state) acquiring the synchronization
        // block for `value` and/or `awaiter`.
        match self.state.load(atomic::Ordering::Acquire) {
            EVENT_BOUND => self.poll_bound(waker),
            EVENT_SET => Some(Ok(self.poll_set())),
            EVENT_AWAITING => self.poll_awaiting(waker),
            EVENT_SIGNALING => Some(self.poll_signaling()),
            EVENT_DISCONNECTED => {
                // There is no result coming, ever! This is the end.
                Some(Err(Disconnected))
            }
            state => {
                unreachable!("unreachable Event state on poll: {state}");
            }
        }
    }

    /// `poll()` impl for `EVENT_BOUND` state.
    ///
    /// Assumes acquired synchronization block for `awaiter`.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn poll_bound(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        // The sender has not yet set any value, so we will have to wait.

        // SAFETY: The only other potential references to the field are other short-lived
        // references in this type, which cannot exist at the moment because the receiver
        // is !Sync so cannot be used in parallel, while the sender is only allowed to
        // access this field in states that explicitly allow it, which we can only be
        // entered by the receiver in this method.
        let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
        // SAFETY: UnsafeCell pointer is never null.
        let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

        awaiter_cell.write(waker.clone());

        // The sender is concurrently racing us to either EVENT_SET or EVENT_DISCONNECTED
        // or EVENT_SIGNALING. Note that it is legal for the sender to enter EVENT_SIGNALING
        // at any time - it does not require there to be an awaiter present.
        //
        // We use Release ordering on success because we are releasing the synchronization
        // block for `awaiter`.
        // We use Acquire ordering in failure because on state transition, we acquire the
        // synchronization block for `awaiter` and `value`.
        match self.state.compare_exchange(
            EVENT_BOUND,
            EVENT_AWAITING,
            atomic::Ordering::Release,
            atomic::Ordering::Acquire,
        ) {
            Ok(_) => {
                // We successfully transitioned to the EVENT_AWAITING state.
                // The sender will wake us up when it sets the value.
                None
            }
            Err(EVENT_SET) => {
                // The sender has set the value while we were doing our thing.
                // We know that the sender will have gone away by this point.
                // We need to clean up our awaiter and pick up the value.

                // SAFETY: The sender is gone - there is nobody else who might be touching
                // the event anymore, we are essentially in a single-threaded mode now.
                // We also just set the value there previously and the event
                // has undergone a transition that could not have affected this value.
                unsafe {
                    self.destroy_awaiter();
                }

                Some(Ok(self.poll_set()))
            }
            Err(EVENT_SIGNALING) => {
                // The sender is in the middle of a state transition. It is using the awaiter,
                // so we cannot touch it any more. We really cannot do anything here
                // except wait for the sender to complete its state transition into
                // EVENT_SET or EVENT_DISCONNECTED.
                Some(self.poll_signaling())
            }
            Err(EVENT_DISCONNECTED) => {
                // The sender was dropped without setting the event.
                // We need to clean up our awaiter and return an error.

                // SAFETY: The sender is gone - there is nobody else who might be touching
                // the event anymore, we are essentially in a single-threaded mode now.
                // We also just set the value there previously and the event
                // has undergone a transition that could not have affected this value.
                unsafe {
                    self.destroy_awaiter();
                }

                Some(Err(Disconnected))
            }
            Err(state) => {
                unreachable!(
                    "unreachable Event state on poll state transition that followed EVENT_BOUND: {state}"
                );
            }
        }
    }

    /// `poll()` impl for `EVENT_SET` state.
    ///
    /// Assumes acquired synchronization block for `value`.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn poll_set(&self) -> T {
        // The sender has delivered a value and we can complete the event.
        // We know that the sender will have gone away by this point.

        // SAFETY: The sender is gone - there is nobody else who might be touching
        // the event anymore, we are essentially in a single-threaded mode now.
        let value_cell_maybe = unsafe { self.value.get().as_mut() };
        // SAFETY: UnsafeCell pointer is never null.
        let value_cell = unsafe { value_cell_maybe.unwrap_unchecked() };

        // We extract the value and consider the cell uninitialized.
        //
        // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
        unsafe { value_cell.assume_init_read() }
    }

    /// `poll()` impl for `EVENT_AWAITING` state.
    ///
    /// Assumes acquired synchronization block for `awaiter`.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn poll_awaiting(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        // We are re-polling after previously starting a wait. This is fine
        // and we just need to clean up the previous waker, replacing it with
        // a new one.

        // The danger here is that the sender may be, at the same time, taking the old
        // waiter and using it. We cannot touch the `awaiter` field right now, as we do
        // not have exclusive access. Instead, we must first transition back into the
        // `EVENT_BOUND` state, then replace the awaiter, then transition back into the
        // `EVENT_AWAITING` state. This way the sender will not do anything invalid.
        // Each state transition is, of course, a potential race that we need to care for.

        // We use Relaxed on both success and failure because we do not yet change
        // externally visible state, merely continue to use our already acquired `awaiter`
        // field that purely sender-side transitions cannot acquire on their own.
        // For the transitions where we do care about synchronization, we use fences.
        match self.state.compare_exchange(
            EVENT_AWAITING,
            EVENT_BOUND,
            atomic::Ordering::Relaxed,
            atomic::Ordering::Relaxed,
        ) {
            Ok(_) => {
                // We have successfully transitioned into EVENT_BOUND.
                // We must destroy the old awaiter and then act as if we were never in
                // EVENT_AWAITING in the first place, going through the EVENT_BOUND path.

                // We are about to touch the `awaiter`, so we need to acquire its synchronization
                // block - the load above was relaxed, so we may not yet have access to the awaiter.
                atomic::fence(atomic::Ordering::Acquire);

                // SAFETY: We have entered the `EVENT_BOUND` state which makes it invalid
                // for the sender to touch `awaiter`. The receiver is !Sync, so we have
                // the only reference here.
                // We also just came from `EVENT_AWAITING` which guarantees there is an
                // awaiter in the cell as we are the only one allowed to touch the field.
                unsafe {
                    self.destroy_awaiter();
                }

                // Now we go back into the normal `EVENT_BOUND` path.
                self.poll_bound(waker)
            }
            Err(EVENT_SET) => {
                // The sender has transitioned the event into EVENT_SET while we were
                // doing our thing. This is fine - we can destroy the old awaiter and
                // simply continue as if we were never in EVENT_AWAITING, going through
                // the EVENT_SET path instead.

                // We are about to touch the `awaiter`, so we need to acquire its synchronization
                // block - the load above was relaxed, so we may not yet have access to the awaiter.
                atomic::fence(atomic::Ordering::Acquire);

                // SAFETY: We have entered the `EVENT_SET` state which makes it invalid
                // for the sender to touch `awaiter`. The receiver is !Sync, so we have
                // the only reference here.
                // We also just came from `EVENT_AWAITING` which guarantees there is an
                // awaiter in the cell as we are the only one allowed to touch the field.
                unsafe {
                    self.destroy_awaiter();
                }

                // Now we go back into the normal `EVENT_SET` path.
                Some(Ok(self.poll_set()))
            }
            Err(EVENT_SIGNALING) => {
                // The sender is in the middle of a state transition. It is using the old
                // waker, so we cannot touch it any more. We really cannot do anything here
                // except wait for the sender to complete its state transition into
                // EVENT_SET or EVENT_DISCONNECTED.
                Some(self.poll_signaling())
            }
            Err(EVENT_DISCONNECTED) => {
                // The sender was dropped without setting the event, while we were awaiting.
                // This already consumed the waker, so all we need to do is handle the disconnect.

                // As the last surviving party, we are responsible for cleanup.
                // We must also ensure that we observe any writes done by the sender
                // before we destroy the event, so we synchronize here.
                atomic::fence(atomic::Ordering::Acquire);

                Some(Err(Disconnected))
            }
            Err(state) => {
                unreachable!(
                    "unreachable Event state on poll state transition that followed EVENT_AWAITING: {state}"
                );
            }
        }
    }

    /// `poll()` impl for `EVENT_SIGNALING` state.
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    fn poll_signaling(&self) -> Result<T, Disconnected> {
        // This is pretty much a mutex - we are locked out of touching the event state until
        // the sender completes its state transition into either EVENT_SET or EVENT_DISCONNECTED.

        let state = loop {
            let state = self.state.load(atomic::Ordering::Relaxed);

            if state != EVENT_SIGNALING {
                break state;
            }

            // The sender-side transition is just a few instructions,
            // which should be near-instantaneous so we just spin.
            spin_loop();
        };

        // The store that brings us out of the SIGNALING state has Release semantics,
        // so we must have an Acquire fence here to ensure we observe all its effects.
        atomic::fence(atomic::Ordering::Acquire);

        // After the sender-side transition, we continue, knowing now that the event must
        // either by in SET or DISCONNECTED state, as those are the only valid sender-side
        // transitions from SIGNALING.
        match state {
            EVENT_SET => {
                let value = self.poll_set();

                // The receiver will clean up.
                Ok(value)
            }
            EVENT_DISCONNECTED => {
                // The receiver is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!(
                    "unreachable OnceEvent post-signaling state on receiver disconnect: {state}"
                )
            }
        }
    }

    #[must_use]
    pub(crate) fn is_set(&self) -> bool {
        // We use Relaxed ordering because this is independent of any other data.
        // If something wishes to actually obtain the value from the event, that
        // logic will perform its own synchronization.
        matches!(
            self.state.load(atomic::Ordering::Relaxed),
            EVENT_SET | EVENT_SIGNALING | EVENT_DISCONNECTED
        )
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
    #[cfg_attr(test, mutants::skip)] // Critical - mutation can cause UB, timeouts and hailstorms.
    pub(crate) fn final_poll(event_cell: &UnsafeCell<Self>) -> Result<Option<T>, Disconnected> {
        // SAFETY: We only ever create shared references to the event, so no aliasing conflicts.
        // The event lives until both sender and receiver are dropped or inert, so we know it must
        // still exist because something was able to call this method.
        let event_maybe = unsafe { event_cell.get().as_ref() };
        // SAFETY: UnsafeCell pointer is never null.
        let event = unsafe { event_maybe.unwrap_unchecked() };

        #[cfg(debug_assertions)]
        event.backtrace.lock().replace(capture_backtrace());

        // The receiver (who is calling this) may still own the waker if the waker has not
        // been used. If this is the case, we need to destroy the waker before proceeding.
        // This is only the case if we are in the AWAITING state - in all other states, we
        // either do not have a waker or the sender will destroy it.
        // We implement this check by attempting to revert ourselves back to the BOUND state.
        // If successful, we know we were in AWAITING and can destroy the waker.
        if event
            .state
            .compare_exchange(
                EVENT_AWAITING,
                EVENT_BOUND,
                atomic::Ordering::Acquire,
                atomic::Ordering::Relaxed,
            )
            .is_ok()
        {
            // SAFETY: The `awaiter` is guaranteed to be present because we just came from
            // `EVENT_AWAITING`. The sender will only touch the awaiter in `EVENT_SIGNALING`,
            // which never entered. Therefore, we are the only one who can touch the awaiter now.
            unsafe {
                event.destroy_awaiter();
            }
        }

        // It is legal to set DISCONNECTED here, permitting dealloc, even though we still
        // hold an &event reference here, which ordinarily means it is still illegal to
        // deallocate the object. However, the dangling reference in this method is an
        // `UnsafeCell` which is allowed to dangle, so we are fine even if the object is
        // immediately destroyed after this line.
        //
        // We use Release because we are releasing the synchronization block of the event.
        let previous_state = event
            .state
            .swap(EVENT_DISCONNECTED, atomic::Ordering::Release);

        match previous_state {
            EVENT_BOUND => {
                // The sender had not yet set any value. It will clean up the event later.
                Ok(None)
            }
            EVENT_SET => {
                // The sender has already set a value but we disconnected before we received it.
                // We need to clean up the value and then later clean up the event, as well.

                // We need to acquire the synchronization block for the `value`.
                atomic::fence(atomic::Ordering::Acquire);

                let value = event.poll_set();

                // The receiver (the caller) will clean up.
                Ok(Some(value))
            }
            EVENT_SIGNALING => {
                // We had started listening for the value and the sender is currently in the
                // middle of signaling us that it has set the value or disconnected.
                // We need to wait for the sender to finish its work first so we can identify
                // which one of us needs to clean up (the sender is going to overwrite
                // the DISCONNECTED state that we wrote because we wrote it during signaling).
                match event.poll_signaling() {
                    // The receiver (the caller) will clean up.
                    Ok(value) => Ok(Some(value)),
                    // The receiver (the caller) is the last endpoint remaining, so it will clean up.
                    Err(Disconnected) => Err(Disconnected),
                }
            }
            EVENT_DISCONNECTED => {
                // We need to ensure we see any writes the sender made before it disconnected.
                atomic::fence(atomic::Ordering::Acquire);

                // The receiver (the caller) is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!("unreachable Event state on receiver disconnect: {previous_state}");
            }
        }
    }

    /// Drops the current awaiter.
    ///
    /// # Safety
    ///
    /// Assumes acquired synchronization block for `awaiter`.
    /// Assumes there is a value in `awaiter`.
    unsafe fn destroy_awaiter(&self) {
        // SAFETY: Forwarding guarantees from the caller.
        let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
        // SAFETY: UnsafeCell pointer is never null.
        let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

        // SAFETY: Forwarding guarantees from the caller.
        unsafe {
            awaiter_cell.assume_init_drop();
        }
    }

    /// Drops the current value.
    ///
    /// # Safety
    ///
    /// Assumes acquired synchronization block for `value`.
    /// Assumes there is a value in `value`.
    unsafe fn destroy_value(&self) {
        // SAFETY: Forwarding guarantees from the caller.
        let value_cell_maybe = unsafe { self.value.get().as_mut() };
        // SAFETY: UnsafeCell pointer is never null.
        let value_cell = unsafe { value_cell_maybe.unwrap_unchecked() };

        // SAFETY: Forwarding guarantees from the caller.
        unsafe {
            value_cell.assume_init_drop();
        }
    }
}

// SAFETY: We are a synchronization primitive, so we do our own synchronization.
unsafe impl<T: Send + 'static> Sync for Event<T> {}

#[expect(clippy::missing_fields_in_debug, reason = "phantoms are boring")]
impl<T: Send + 'static> fmt::Debug for Event<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Event");

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
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    reason = "test code, be concise"
)]
mod tests {
    use std::pin::pin;
    use std::sync::{Arc, Barrier};
    use std::task::Poll;
    use std::{task, thread};

    use spin_on::spin_on;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::IntoValueError;

    assert_impl_all!(Event<u32>: Send, Sync);

    #[test]
    fn boxed_send_receive() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_send_receive_unit() {
        let (sender, receiver) = Event::<()>::boxed();
        let mut receiver = pin!(receiver);

        sender.send(());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn boxed_send_receive_u128() {
        let (sender, receiver) = Event::<u128>::boxed();
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_send_receive_array() {
        let (sender, receiver) = Event::<[u128; 4]>::boxed();
        let mut receiver = pin!(receiver);

        sender.send([42, 43, 44, 45]);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok([42, 43, 44, 45]))));
    }

    #[test]
    fn boxed_receive_send_receive() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        sender.send(42);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_drop_send() {
        let (sender, _) = Event::<i32>::boxed();

        sender.send(42);
    }

    #[test]
    fn boxed_drop_receive() {
        let (_, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn boxed_receive_drop_receive() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn boxed_receive_drop_send() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);

        sender.send(42);
    }

    #[test]
    fn boxed_receive_drop_drop_receiver_first() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn boxed_receive_drop_drop_sender_first() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn boxed_drop_drop_receiver_first() {
        let (sender, receiver) = Event::<i32>::boxed();

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn boxed_drop_drop_sender_first() {
        let (sender, receiver) = Event::<i32>::boxed();

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn boxed_is_ready() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

        assert!(!receiver.is_ready());

        sender.send(42);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_drop_is_ready() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

        assert!(!receiver.is_ready());

        drop(sender);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn boxed_into_value() {
        let (sender, receiver) = Event::<i32>::boxed();

        let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(42)));
    }

    #[test]
    fn boxed_drop_into_value() {
        let (sender, receiver) = Event::<i32>::boxed();

        drop(sender);

        assert!(matches!(
            receiver.into_value(),
            Err(IntoValueError::Disconnected)
        ));
    }

    #[test]
    #[should_panic]
    fn boxed_panic_poll_after_completion() {
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

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
        let (sender, receiver) = Event::<i32>::boxed();
        let mut receiver = pin!(receiver);

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
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_receive_send_receive() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        sender.send(42);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_drop_send() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, _) = unsafe { Event::<i32>::placed(place.as_mut()) };

        sender.send(42);
    }

    #[test]
    fn placed_drop_receive() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (_, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_receive_drop_receive() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_receive_drop_send() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);

        sender.send(42);
    }

    #[test]
    fn placed_receive_drop_drop_receiver_first() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_receive_drop_drop_sender_first() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_drop_drop_receiver_first() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_drop_drop_sender_first() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_is_ready() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        assert!(!receiver.is_ready());

        sender.send(42);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_drop_is_ready() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        assert!(!receiver.is_ready());

        drop(sender);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_into_value() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let Err(IntoValueError::Pending(receiver)) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(42)));
    }

    #[test]
    fn placed_drop_into_value() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        drop(sender);

        assert!(matches!(
            receiver.into_value(),
            Err(IntoValueError::Disconnected)
        ));
    }

    #[test]
    #[should_panic]
    fn placed_panic_poll_after_completion() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

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
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

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
    fn boxed_send_receive_mt() {
        let (sender, receiver) = Event::<i32>::boxed();

        thread::spawn(move || {
            sender.send(42);
        })
        .join()
        .unwrap();

        thread::spawn(move || {
            let mut receiver = pin!(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn boxed_receive_send_receive_mt() {
        let (sender, receiver) = Event::<i32>::boxed();

        let first_poll_completed = Arc::new(Barrier::new(2));
        let first_poll_completed_clone = Arc::clone(&first_poll_completed);

        let send_thread = thread::spawn(move || {
            first_poll_completed.wait();

            sender.send(42);
        });

        let receive_thread = thread::spawn(move || {
            let mut receiver = pin!(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            first_poll_completed_clone.wait();

            // We do not know how many polls this will take, so we switch into real async.
            spin_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn boxed_send_receive_unbiased_mt() {
        let (sender, receiver) = Event::<i32>::boxed();

        let receive_thread = thread::spawn(move || {
            spin_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn boxed_drop_receive_unbiased_mt() {
        let (sender, receiver) = Event::<i32>::boxed();

        let receive_thread = thread::spawn(move || {
            spin_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Err(Disconnected)));
            });
        });

        let send_thread = thread::spawn(move || {
            drop(sender);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn boxed_drop_send_unbiased_mt() {
        let (sender, receiver) = Event::<i32>::boxed();

        let receive_thread = thread::spawn(move || {
            drop(receiver);
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn placed_send_receive_mt() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        thread::spawn(move || {
            sender.send(42);
        })
        .join()
        .unwrap();

        thread::spawn(move || {
            let mut receiver = pin!(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn placed_send_receive_reused_mt() {
        const ITERATIONS: usize = 123;

        let mut place = Box::pin(EmbeddedEvent::<i32>::new());

        for _ in 0..ITERATIONS {
            let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

            thread::spawn(move || {
                sender.send(42);
            })
            .join()
            .unwrap();

            thread::spawn(move || {
                let mut receiver = pin!(receiver);
                let mut cx = task::Context::from_waker(Waker::noop());

                let poll_result = receiver.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Ready(Ok(42))));
            })
            .join()
            .unwrap();
        }
    }

    #[test]
    fn placed_receive_send_receive_mt() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let first_poll_completed = Arc::new(Barrier::new(2));
        let first_poll_completed_clone = Arc::clone(&first_poll_completed);

        let send_thread = thread::spawn(move || {
            first_poll_completed.wait();

            sender.send(42);
        });

        let receive_thread = thread::spawn(move || {
            let mut receiver = pin!(receiver);
            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            first_poll_completed_clone.wait();

            // We do not know how many polls this will take, so we switch into real async.
            spin_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn placed_send_receive_unbiased_mt() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            spin_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn placed_drop_receive_unbiased_mt() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            spin_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Err(Disconnected)));
            });
        });

        let send_thread = thread::spawn(move || {
            drop(sender);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn placed_drop_send_unbiased_mt() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            drop(receiver);
        });

        let send_thread = thread::spawn(move || {
            sender.send(42);
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_no_awaiter() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let _endpoints = unsafe { Event::<i32>::placed(place.as_mut()) };

        let mut called = false;
        unsafe { place.inner.get().as_ref().unwrap().assume_init_ref() }.inspect_awaiter(
            |backtrace| {
                called = true;
                assert!(backtrace.is_none());
            },
        );

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_with_awaiter() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (_sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = pin!(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        let mut called = false;
        unsafe { place.inner.get().as_ref().unwrap().assume_init_ref() }.inspect_awaiter(
            |backtrace| {
                called = true;
                assert!(backtrace.is_some());
            },
        );

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_after_sender_drop() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = pin!(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(sender);

        let mut called = false;
        unsafe { place.inner.get().as_ref().unwrap().assume_init_ref() }.inspect_awaiter(
            |backtrace| {
                called = true;
                assert!(backtrace.is_some());
            },
        );

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_after_receiver_drop() {
        let mut place = Box::pin(EmbeddedEvent::<i32>::new());
        let (_sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(receiver);

        let mut called = false;
        unsafe { place.inner.get().as_ref().unwrap().assume_init_ref() }.inspect_awaiter(
            |backtrace| {
                called = true;
                assert!(backtrace.is_some());
            },
        );

        assert!(called);
    }
}
