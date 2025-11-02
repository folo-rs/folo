use std::alloc::{Layout, alloc, dealloc};
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::{Cell, UnsafeCell};
use std::future::Future;
use std::hint::spin_loop;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{ManuallyDrop, MaybeUninit, offset_of};
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::atomic::{self, AtomicU8};
use std::task::{self, Poll, Waker};

#[cfg(debug_assertions)]
use parking_lot::Mutex;

#[cfg(debug_assertions)]
use crate::{BacktraceType, capture_backtrace};
use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, EVENT_SIGNALING,
    ReflectiveTSend, Sealed,
};

/// Coordinates delivery of a `T` at most once from a sender to a receiver on any thread.
#[derive(Debug)]
pub struct Event<T>
where
    T: Send,
{
    /// The logical state of the event; see constants in `state.rs`.
    state: AtomicU8,

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
    T: Send,
{
    /// In-place initializes a new instance in the `BOUND` state.
    ///
    /// This is for internal use only and is wrapped by public methods that also
    /// wire up the sender and receiver after doing the initialization. An event
    /// without a sender and receiver is invalid.
    fn new_in_inner(place: &mut MaybeUninit<Self>) -> NonNull<Self> {
        // The key here is that we can skip initializing the MaybeUninit fields because
        // they start uninitialized by design and the UnsafeCell wrapper is transparent,
        // only affecting accesses and not the contents.
        let base_ptr = place.as_mut_ptr();

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

        // SAFETY: This came from a reference so guaranteed non-null.
        unsafe { NonNull::new_unchecked(base_ptr) }
    }

    /// Heap-allocates a new instance and returns the endpoints.
    ///
    /// The memory used is released when both endpoints are dropped.
    ///
    /// For more efficiency, consider using [`new_in`][Self::new_in], which allows you to
    /// initialize the event in preallocated storage as part of a larger structure.
    #[must_use]
    pub fn boxed() -> (Sender<BoxedRef<T>>, Receiver<BoxedRef<T>>) {
        let (sender_event, receiver_event) = BoxedRef::new_pair();

        (Sender::new(sender_event), Receiver::new(receiver_event))
    }

    /// Initializes the event in-place, returning the endpoints.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that:
    ///
    /// * The referenced place remains pinned and valid for writes for the entire lifetime of
    ///   the sender and receiver returned by this function.
    /// * The referenced place is not already in use by another instance of the event.
    #[must_use]
    pub unsafe fn placed(
        place: Pin<&mut MaybeUninit<Self>>,
    ) -> (Sender<PtrRef<T>>, Receiver<PtrRef<T>>) {
        // SAFETY: Nothing is getting moved, we just temporarily unwrap the Pin wrapper.
        let event = Self::new_in_inner(unsafe { place.get_unchecked_mut() });

        (
            Sender::new(PtrRef { event }),
            Receiver::new(PtrRef { event }),
        )
    }

    /// Uses the provided closure to inspect the backtrace of the current awaiter,
    /// if there is an awaiter and if backtrace capturing is enabled.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure receives `None` if no one is awaiting the event.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiter(&self, f: impl FnOnce(Option<&Backtrace>)) {
        let backtrace = self.backtrace.lock();
        f(backtrace.as_ref());
    }

    /// Sets the value of the event and notifies the awaiter, if there is one.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    fn set(&self, result: T) -> Result<(), Disconnected> {
        let value_cell = self.value.get();

        // We can start by setting the value - this has to happen no matter what.
        // Everything else we do here is just to get the awaiter to come pick it up.
        //
        // SAFETY: It is valid for the sender to write here because we know that nobody else will
        // be accessing this field at this time. This is guaranteed by:
        // * There is only one sender and it is !Sync, so it cannot be used in parallel.
        // * The receiver will only access this field in the "Set" state, which can only be entered
        //   from later on in this method.
        unsafe {
            value_cell.write(MaybeUninit::new(result));
        }

        // A "set" operation is always a state increment. See `state.rs`.
        // We use `Release` ordering for the write because we are
        // releasing the synchronization block of `value`.
        let previous_state = self.state.fetch_add(1, atomic::Ordering::Release);

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
                let awaiter_cell = unsafe {
                    self.awaiter
                        .get()
                        .as_mut()
                        .expect("UnsafeCell pointer is never null")
                };

                // We extract the waker and consider the field uninitialized again.
                // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
                let waker = unsafe { awaiter_cell.assume_init_read() };

                // Before we send the wake signal we must transition into the `EVENT_SET` state
                // so that the receiver can directly pick up the result when it comes back.
                //
                // We use Release ordering because we are releasing the synchronization block of
                // the `awaiter`. Note that `value` was already released by `fetch_add()` above.
                self.state.store(EVENT_SET, atomic::Ordering::Release);

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
                    self.destroy_value();
                }

                // Before it is safe to destroy the event, we need to synchronize with whatever
                // writes the receiver may have done into its state (e.g. it may have removed
                // its waker before it marked the event as disconnected).
                atomic::fence(atomic::Ordering::Acquire);

                Err(Disconnected)
            }
            _ => {
                unreachable!("unreachable Event state on set: {previous_state}");
            }
        }
    }

    /// We are intended to be polled via `Future::poll`, so we have an equivalent signature here.
    ///
    /// If `Some` is returned, the caller is the last remaining endpoint and responsible
    /// for cleaning up the event.
    fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
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
    fn poll_bound(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        // The sender has not yet set any value, so we will have to wait.

        // SAFETY: The only other potential references to the field are other short-lived
        // references in this type, which cannot exist at the moment because the receiver
        // is !Sync so cannot be used in parallel, while the sender is only allowed to
        // access this field in states that explicitly allow it, which we can only be
        // entered by the receiver in this method.
        let awaiter_cell = unsafe {
            self.awaiter
                .get()
                .as_mut()
                .expect("UnsafeCell pointer is never null")
        };

        awaiter_cell.write(waker.clone());

        // The sender is concurrently racing us to either EVENT_SET or EVENT_DISCONNECTED.
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
    fn poll_set(&self) -> T {
        // The sender has delivered a value and we can complete the event.
        // We know that the sender will have gone away by this point.

        // SAFETY: The sender is gone - there is nobody else who might be touching
        // the event anymore, we are essentially in a single-threaded mode now.
        let value_cell = unsafe {
            self.value
                .get()
                .as_mut()
                .expect("UnsafeCell pointer is never null")
        };

        // We extract the value and consider the cell uninitialized.
        //
        // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
        unsafe { value_cell.assume_init_read() }
    }

    /// `poll()` impl for `EVENT_AWAITING` state.
    ///
    /// Assumes acquired synchronization block for `awaiter`.
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
            Err(state) => {
                unreachable!(
                    "unreachable OnceEvent state on poll state transition that followed EVENT_AWAITING: {state}"
                );
            }
        }
    }

    /// `poll()` impl for `EVENT_SIGNALING` state.
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

    /// Drops the current awaiter.
    ///
    /// # Safety
    ///
    /// Assumes acquired synchronization block for `awaiter`.
    /// Assumes there is a value in `awaiter`.
    unsafe fn destroy_awaiter(&self) {
        // SAFETY: Forwarding guarantees from the caller.
        let awaiter_cell = unsafe {
            self.awaiter
                .get()
                .as_mut()
                .expect("UnsafeCell pointer is never null")
        };

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

    /// Marks the event as having been disconnected early from the sender side.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    fn sender_dropped_without_set(&self) -> Result<(), Disconnected> {
        // We first need to switch into the SIGNALING state, which acquires exclusive access
        // of the awaiter field, so we can send the wake signal if there is an awaiter.
        // Only after that can we transition into the DISCONNECTED state (because that must
        // be our last action - the receiver may clean up the event at any point after we do that).

        let previous_state = self.state.swap(EVENT_SIGNALING, atomic::Ordering::Relaxed);

        match previous_state {
            EVENT_BOUND => {
                // There was nobody polling via the receiver - our work here is done.
                self.state
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
                let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
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

                // We are all done here.
                self.state
                    .store(EVENT_DISCONNECTED, atomic::Ordering::Release);

                // The receiver is the last endpoint remaining, so it will clean up.
                Ok(())
            }
            EVENT_DISCONNECTED => {
                // The sender is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!("unreachable Event state on sender disconnect: {previous_state}");
            }
        }
    }

    /// Checks whether the event has been set (either with a value or with a disconnect signal).
    fn is_set(&self) -> bool {
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
    fn final_poll(&self) -> Result<Option<T>, Disconnected> {
        #[cfg(debug_assertions)]
        self.backtrace.lock().replace(capture_backtrace());

        // The receiver (who is calling this) may still own the waker if the waker has not
        // been used. If this is the case, we need to destroy the waker before proceeding.
        // This is only the case if we are in the AWAITING state - in all other states, we
        // either do not have a waker or the sender will destroy it.
        // We implement this check by attempting to revert ourselves back to the BOUND state.
        // If successful, we know we were in AWAITING and can destroy the waker.
        if self
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
                self.destroy_awaiter();
            }
        }

        // We use Release because we are releasing the synchronization block of the event.
        let previous_state = self
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

                let value = self.poll_set();

                // The receiver will clean up.
                Ok(Some(value))
            }
            EVENT_SIGNALING => {
                // We had started listening for the value and the sender is currently in the
                // middle of signaling us that it has set the value or disconnected.
                // We need to wait for the sender to finish its work first so we can identify
                // which one of us needs to clean up (the sender is going to overwrite
                // the DISCONNECTED state that we wrote because we wrote it during signaling).
                match self.poll_signaling() {
                    Ok(value) => Ok(Some(value)),
                    Err(Disconnected) => Err(Disconnected),
                }
            }
            EVENT_DISCONNECTED => {
                // The receiver is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!(
                    "unreachable OnceEvent state on receiver disconnect: {previous_state}"
                );
            }
        }
    }
}

// SAFETY: We are a synchronization primitive, so we do our own synchronization.
unsafe impl<T: Send> Sync for Event<T> {}

/// Enables a sender or receiver to reference the event that connects them.
///
/// This is a sealed trait and exists for internal use only. User code never needs to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait EventRef<T>: Deref<Target = Event<T>> + ReflectiveTSend + RefPrivate<T> + Sealed
where
    T: Send,
{
}

trait RefPrivate<T>
where
    T: Send,
{
    /// Releases the event, asserting that the last endpoint has been dropped
    /// and nothing will access the event after this call.
    fn release_event(&self);
}

/// References an event stored anywhere, via raw pointer.
///
/// Only used in type names. Instances are created internally by [`Event`].
#[derive(Debug)]
pub struct PtrRef<T>
where
    T: Send,
{
    event: NonNull<Event<T>>,
}

impl<T> Sealed for PtrRef<T> where T: Send {}
impl<T> EventRef<T> for PtrRef<T> where T: Send {}
impl<T> RefPrivate<T> for PtrRef<T>
where
    T: Send,
{
    fn release_event(&self) {}
}
impl<T> Deref for PtrRef<T>
where
    T: Send,
{
    type Target = Event<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}
impl<T: Send> ReflectiveTSend for PtrRef<T> {
    type T = T;
}
// SAFETY: This is only used with the thread-safe event (the event is Sync).
unsafe impl<T> Send for PtrRef<T> where T: Send {}

/// References an event stored on the heap.
///
/// Only used in type names. Instances are created internally by [`Event`].
#[derive(Debug)]
pub struct BoxedRef<T>
where
    T: Send,
{
    event: NonNull<Event<T>>,
}

impl<T> BoxedRef<T>
where
    T: Send,
{
    fn new_pair() -> (Self, Self) {
        // SAFETY: The layout is correct for the type we are using - all is well.
        let event = NonNull::new(unsafe { alloc(Self::layout()) })
            .expect("memory allocation failed - fatal error")
            .cast();

        // SAFETY: MaybeUninit is a transparent wrapper, so the layout matches.
        // This is the only reference, so we have exclusive access rights.
        let event_as_maybe_uninit = unsafe { event.cast::<MaybeUninit<Event<T>>>().as_mut() };

        Event::new_in_inner(event_as_maybe_uninit);

        (Self { event }, Self { event })
    }

    const fn layout() -> Layout {
        Layout::new::<Event<T>>()
    }
}

impl<T> RefPrivate<T> for BoxedRef<T>
where
    T: Send,
{
    fn release_event(&self) {
        // The caller tells us that they are the last endpoint, so nothing else can possibly
        // be accessing the event any more. We can safely release the memory.

        // SAFETY: Still the same type - all is well. We rely on the event state machine
        // to ensure that there is no double-release happening.
        unsafe {
            dealloc(self.event.as_ptr().cast(), Self::layout());
        }
    }
}

impl<T> Sealed for BoxedRef<T> where T: Send {}
impl<T> EventRef<T> for BoxedRef<T> where T: Send {}

impl<T> Deref for BoxedRef<T>
where
    T: Send,
{
    type Target = Event<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        unsafe { self.event.as_ref() }
    }
}
impl<T> ReflectiveTSend for BoxedRef<T>
where
    T: Send,
{
    type T = T;
}
// SAFETY: This is only used with the thread-safe event (the event is Sync).
unsafe impl<T> Send for BoxedRef<T> where T: Send {}

/// Delivers a single value to the receiver connected to the same event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `Sender<BoxedRef<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
#[derive(Debug)]
pub struct Sender<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    event_ref: E,

    // We are not compatible with concurrent sender use from multiple threads.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E> Sender<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn new(event_ref: E) -> Self {
        Self {
            event_ref,
            _not_sync: PhantomData,
        }
    }

    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    pub fn send(self, value: E::T) {
        // The drop logic is different before/after set(), so we switch to manual drop here.
        let mut this = ManuallyDrop::new(self);

        if this.event_ref.set(value) == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            this.event_ref.release_event();
        }

        // SAFETY: The field contains a valid object of the right type. We avoid a double-drop
        // via ManuallyDrop above. We consume `self` so nothing further can happen.
        unsafe {
            ptr::drop_in_place(&raw mut this.event_ref);
        }
    }
}

impl<E> Drop for Sender<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn drop(&mut self) {
        if self.event_ref.sender_dropped_without_set() == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            self.event_ref.release_event();
        }
    }
}

/// Receives a single value from the sender connected to the same event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `Receiver<BoxedRef<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
#[derive(Debug)]
pub struct Receiver<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    // This is `None` if the receiver has already been polled to completion. We need to guard
    // against that because the event will be cleaned up after the first poll that signals "ready".
    event_ref: Option<E>,

    // We are not compatible with concurrent receiver use from multiple threads.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E> Receiver<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn new(event_ref: E) -> Self {
        Self {
            event_ref: Some(event_ref),
            _not_sync: PhantomData,
        }
    }

    /// Checks whether a value is ready to be received.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    pub fn is_ready(&self) -> bool {
        let Some(event_ref) = &self.event_ref else {
            panic!("receiver queried after completion");
        };

        event_ref.is_set()
    }

    /// Consumes the receiver and transforms it into the received value, if the value is available.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available value without blocking. It returns `Some(value)` if a value has
    /// already been sent, or `None` if no value is currently available.
    ///
    /// # Panics
    ///
    /// Panics if the value has already been received via `Future::poll()`.
    pub fn into_value(self) -> Result<Result<E::T, Disconnected>, Self> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("OnceReceiver polled after completion");

        // Check the current state directly to decide what to do
        let current_state = event_ref.state.load(atomic::Ordering::Acquire);

        match current_state {
            EVENT_BOUND | EVENT_AWAITING | EVENT_SIGNALING => {
                // No value available yet - return the receiver
                Err(self)
            }
            EVENT_SET | EVENT_DISCONNECTED => {
                // Value available or disconnected - consume self and let final_poll decide
                let mut this = ManuallyDrop::new(self);
                let event_ref = this.event_ref.take().unwrap();

                match event_ref.final_poll() {
                    Ok(Some(value)) => {
                        event_ref.release_event();
                        Ok(Ok(value))
                    }
                    Ok(None) => {
                        // This shouldn't happen - final_poll should return Some(value) or Err(Disconnected)
                        unreachable!("final_poll returned None")
                    }
                    Err(Disconnected) => {
                        event_ref.release_event();
                        Ok(Err(Disconnected))
                    }
                }
            }
            _ => {
                unreachable!("Invalid event state: {}", current_state)
            }
        }
    }
}

impl<E> Future for Receiver<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    type Output = Result<E::T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("OnceReceiver polled after completion");

        let inner_poll_result = event_ref.poll(cx.waker());

        // If the poll returns `Some`, we need to clean up the event.
        if inner_poll_result.is_some() {
            // We have a slight problem here, though, because if we release the event here and
            // the memory is freed, what happens if someone foolishly tries to poll again? That
            // could lead to a memory safety violation if we did it naively. Therefore, we have
            // to guard against double polling via an `Option`.
            event_ref.release_event();

            // SAFETY: We are not moving anything, merely updating a field.
            let this = unsafe { self.get_unchecked_mut() };
            this.event_ref = None;
        }

        inner_poll_result.map_or_else(|| Poll::Pending, Poll::Ready)
    }
}

impl<E> Drop for Receiver<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn drop(&mut self) {
        if let Some(event_ref) = self.event_ref.take() {
            match event_ref.final_poll() {
                Ok(None) => {
                    // Nothing for us to do - the sender was still connected and had not
                    // sent any value, so it will perform the cleanup on its own.
                }
                _ => {
                    // The sender has already disconnected, so we need to clean up the event.
                    event_ref.release_event();
                }
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
mod tests {
    use std::pin::pin;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use futures::executor::block_on;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(Event<u32>: Send, Sync);

    assert_impl_all!(BoxedRef<u32>: Send);
    assert_not_impl_any!(BoxedRef<u32>: Sync);

    assert_impl_all!(PtrRef<u32>: Send);
    assert_not_impl_any!(PtrRef<u32>: Sync);

    assert_impl_all!(Sender<BoxedRef<u32>>: Send);
    assert_not_impl_any!(Sender<BoxedRef<u32>>: Sync);

    assert_impl_all!(Receiver<BoxedRef<u32>>: Send);
    assert_not_impl_any!(Receiver<BoxedRef<u32>>: Sync);

    assert_impl_all!(Sender<PtrRef<u32>>: Send);
    assert_not_impl_any!(Sender<PtrRef<u32>>: Sync);

    assert_impl_all!(Receiver<PtrRef<u32>>: Send);
    assert_not_impl_any!(Receiver<PtrRef<u32>>: Sync);

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

        let Err(receiver) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(Ok(42))));
    }

    #[test]
    fn boxed_drop_into_value() {
        let (sender, receiver) = Event::<i32>::boxed();

        drop(sender);

        assert!(matches!(receiver.into_value(), Ok(Err(Disconnected))));
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_receive_send_receive() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, _) = unsafe { Event::<i32>::placed(place.as_mut()) };

        sender.send(42);
    }

    #[test]
    fn placed_drop_receive() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (_, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_receive_drop_receive() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_drop_drop_sender_first() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_is_ready() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let Err(receiver) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(Ok(42))));
    }

    #[test]
    fn placed_drop_into_value() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        drop(sender);

        assert!(matches!(receiver.into_value(), Ok(Err(Disconnected))));
    }

    #[test]
    #[should_panic]
    fn placed_panic_poll_after_completion() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
            block_on(async {
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
            block_on(async {
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
            block_on(async {
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
    fn placed_receive_send_receive_mt() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
            block_on(async {
                let result = &mut receiver.await;
                assert!(matches!(result, Ok(42)));
            });
        });

        send_thread.join().unwrap();
        receive_thread.join().unwrap();
    }

    #[test]
    fn placed_send_receive_unbiased_mt() {
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            block_on(async {
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
        let (sender, receiver) = unsafe { Event::<i32>::placed(place.as_mut()) };

        let receive_thread = thread::spawn(move || {
            block_on(async {
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
        let mut place = Box::pin(MaybeUninit::<Event<i32>>::uninit());
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
}
