//! Thread-safe one-time events.
//!
//! This module provides thread-safe event types that can be shared across threads
//! and used for cross-thread communication.

#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::{Cell, UnsafeCell};
use std::future::Future;
use std::hint::spin_loop;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
#[cfg(debug_assertions)]
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicU8};
use std::task;
use std::task::Waker;

#[cfg(debug_assertions)]
use crate::{BacktraceType, ERR_POISONED_LOCK, capture_backtrace};
use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, EVENT_SIGNALING,
    EVENT_UNBOUND, ReflectiveTSend, Sealed, WithTwoOwners,
};

/// A one-time event that can send and receive a value of type `T`, potentially across threads.
///
/// The event can only be used once - after binding a sender and receiver,
/// subsequent bind calls will panic (or return [`None`] for the checked variants).
///
/// Similarly, sending a value will consume the sender, preventing further use.
/// You need to create a new event instance to create another sender-receiver pair.
///
/// To reuse event resources for many operations and avoid constantly recreating events, use
/// [`OnceEventPool`][crate::OnceEventPool].
///
/// For single-threaded usage, see [`OnceEvent`][crate::OnceEvent] which has
/// lower overhead.
///
/// # Example
///
/// ```rust
/// use events::OnceEvent;
/// # use futures::executor::block_on;
///
/// # block_on(async {
/// let event = OnceEvent::<String>::new();
/// let (sender, receiver) = event.bind_by_ref();
///
/// sender.send("Hello".to_string());
/// let message = receiver.await.unwrap();
/// assert_eq!(message, "Hello");
/// # });
/// ```
#[derive(Debug)]
pub struct OnceEvent<T>
where
    T: Send,
{
    /// The logical state of the event; see `event_state.rs`.
    state: AtomicU8,

    /// If `state` is `EVENT_AWAITING` or `EVENT_RESOLVING`, this field is initialized with the
    /// waker of whoever most recently awaited the receiver. In other states, this field is not
    /// initialized.
    ///
    /// We use `MaybeUninit` to minimize the storage and avoid an `Option` or enum overhead,
    /// as we already track the presence via `state`.
    ///
    /// We use `UnsafeCell` because we are a synchronization primitive and
    /// do our own synchronization.
    awaiter: UnsafeCell<MaybeUninit<Waker>>,

    /// If `state` is `EVENT_SET`, this field is initialized with the value that was sent by
    /// the sender. In other states, this field is not initialized.
    ///
    /// We use `MaybeUninit` to minimize the storage and avoid an `Option` or enum overhead,
    /// as we already track the presence via `state`.
    ///
    /// We use `UnsafeCell` because we are a synchronization primitive and
    /// do our own synchronization.
    value: UnsafeCell<MaybeUninit<T>>,

    // In debug builds, we save the backtrace of the most recent awaiter here. This will not be
    // cleared merely by exiting the `Awaiting` state, as even in the `Set` state there is value
    // in retaining the backtrace for debugging purposes.
    #[cfg(debug_assertions)]
    backtrace: Mutex<Option<BacktraceType>>,

    // It is invalid to move this type once it has been pinned.
    _requires_pinning: PhantomPinned,
}

impl<T> OnceEvent<T>
where
    T: Send,
{
    /// Creates a new thread-safe event.
    ///
    /// The event must be bound to a sender-receiver pair to be used.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// ```
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(EVENT_UNBOUND),
            awaiter: UnsafeCell::new(MaybeUninit::uninit()),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            #[cfg(debug_assertions)]
            backtrace: Mutex::new(None),
            _requires_pinning: PhantomPinned,
        }
    }

    /// Creates a new thread-safe event that starts in the bound state.
    ///
    /// This is for internal use only - pooled events start in the bound state.
    #[must_use]
    pub(crate) fn new_bound() -> Self {
        Self {
            state: AtomicU8::new(EVENT_BOUND),
            awaiter: UnsafeCell::new(MaybeUninit::uninit()),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            #[cfg(debug_assertions)]
            backtrace: Mutex::new(None),
            _requires_pinning: PhantomPinned,
        }
    }

    /// Creates a new heap-allocated thread-safe event, returning both the sender and receiver
    /// for this event.
    ///
    /// The memory used by the event is automatically released when both endpoints are dropped.
    #[must_use]
    #[inline]
    pub fn new_heap() -> (OnceSender<HeapEvent<T>>, OnceReceiver<HeapEvent<T>>) {
        let (sender_event, receiver_event) = HeapEvent::new_pair();

        (
            OnceSender::new(sender_event),
            OnceReceiver::new(receiver_event),
        )
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a shared reference to the event.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// let (sender, receiver) = event.bind_by_ref();
    /// ```
    #[must_use]
    #[inline]
    pub fn bind_by_ref(&self) -> (OnceSender<RefEvent<'_, T>>, OnceReceiver<RefEvent<'_, T>>) {
        self.bind_by_ref_checked()
            .expect("OnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a shared reference to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// let endpoints = event.bind_by_ref_checked().unwrap();
    /// let endpoints2 = event.bind_by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    #[must_use]
    #[inline]
    pub fn bind_by_ref_checked(
        &self,
    ) -> Option<(OnceSender<RefEvent<'_, T>>, OnceReceiver<RefEvent<'_, T>>)> {
        // We use Relaxed because this does not affect any other state of the event.
        if self
            .state
            .compare_exchange(
                EVENT_UNBOUND,
                EVENT_BOUND,
                atomic::Ordering::Relaxed,
                atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            return None;
        }

        Some((
            OnceSender::new(RefEvent { event: self }),
            OnceReceiver::new(RefEvent { event: self }),
        ))
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a shared reference to the event.
    ///
    /// This method assumes the event is not already bound and skips the check for performance.
    ///
    /// # Safety
    ///
    /// The caller must guarantee the event is not already bound.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let event = OnceEvent::<i32>::new();
    /// // We know this is the first and only binding of this event
    /// let (sender, receiver) = event.bind_by_ref_unchecked();
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_ref_unchecked(
        &self,
    ) -> (OnceSender<RefEvent<'_, T>>, OnceReceiver<RefEvent<'_, T>>) {
        // We use Relaxed because this does not affect any other state of the event.
        self.state.store(EVENT_BOUND, atomic::Ordering::Relaxed);

        (
            OnceSender::new(RefEvent { event: self }),
            OnceReceiver::new(RefEvent { event: self }),
        )
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Arc` to the event.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] when called.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// let (sender, receiver) = event.bind_by_arc();
    /// ```
    #[must_use]
    #[inline]
    pub fn bind_by_arc(self: &Arc<Self>) -> (OnceSender<ArcEvent<T>>, OnceReceiver<ArcEvent<T>>) {
        self.bind_by_arc_checked()
            .expect("OnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Arc` to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] when called.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// let endpoints = event.bind_by_arc_checked().unwrap();
    /// let endpoints2 = event.bind_by_arc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    #[inline]
    pub fn bind_by_arc_checked(
        self: &Arc<Self>,
    ) -> Option<(OnceSender<ArcEvent<T>>, OnceReceiver<ArcEvent<T>>)> {
        // We use Relaxed because this does not affect any other state of the event.
        if self
            .state
            .compare_exchange(
                EVENT_UNBOUND,
                EVENT_BOUND,
                atomic::Ordering::Relaxed,
                atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            return None;
        }

        Some((
            OnceSender::new(ArcEvent {
                event: Arc::clone(self),
            }),
            OnceReceiver::new(ArcEvent {
                event: Arc::clone(self),
            }),
        ))
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Arc` to the event.
    ///
    /// This method assumes the event is not already bound and skips the check for performance.
    ///
    /// This method requires the event to be wrapped in an [`Arc`] when called.
    ///
    /// # Safety
    ///
    /// The caller must guarantee the event is not already bound.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// // We know this is the first and only binding of this event
    /// let (sender, receiver) = event.bind_by_arc_unchecked();
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_arc_unchecked(
        self: &Arc<Self>,
    ) -> (OnceSender<ArcEvent<T>>, OnceReceiver<ArcEvent<T>>) {
        // We use Relaxed because this does not affect any other state of the event.
        self.state.store(EVENT_BOUND, atomic::Ordering::Relaxed);

        (
            OnceSender::new(ArcEvent {
                event: Arc::clone(self),
            }),
            OnceReceiver::new(ArcEvent {
                event: Arc::clone(self),
            }),
        )
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a raw pointer to the event.
    ///
    /// This method requires the event to be pinned when called.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The event remains alive and pinned for the entire lifetime of the sender and receiver.
    /// - The sender and receiver are dropped before the event is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let mut event = Box::pin(OnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event is pinned and outlives the sender and receiver
    /// let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
    ///
    /// sender.send(42);
    /// let value = receiver.await.unwrap();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// # });
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_ptr(
        self: Pin<&Self>,
    ) -> (OnceSender<PtrEvent<T>>, OnceReceiver<PtrEvent<T>>) {
        // SAFETY: Caller has guaranteed event lifetime management
        unsafe { self.bind_by_ptr_checked() }.expect("OnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a raw pointer to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// This method requires the event to be pinned when called.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The event remains alive and pinned for the entire lifetime of the sender and receiver.
    /// - The sender and receiver are dropped before the event is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::OnceEvent;
    ///
    /// let mut event = Box::pin(OnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let endpoints = unsafe { event.as_ref().bind_by_ptr_checked() }.unwrap();
    /// let endpoints2 = unsafe { event.as_ref().bind_by_ptr_checked() }; // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    #[inline]
    pub unsafe fn bind_by_ptr_checked(
        self: Pin<&Self>,
    ) -> Option<(OnceSender<PtrEvent<T>>, OnceReceiver<PtrEvent<T>>)> {
        // We use Relaxed because this does not affect any other state of the event.
        if self
            .state
            .compare_exchange(
                EVENT_UNBOUND,
                EVENT_BOUND,
                atomic::Ordering::Relaxed,
                atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            return None;
        }

        let event_ptr = NonNull::from(self.get_ref());

        Some((
            OnceSender::new(PtrEvent { event: event_ptr }),
            OnceReceiver::new(PtrEvent { event: event_ptr }),
        ))
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a raw pointer to the event.
    ///
    /// This method assumes the event is not already bound and skips the check for performance.
    ///
    /// This method requires the event to be pinned when called.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The event is not already bound.
    /// - The event remains alive and pinned for the entire lifetime of the sender and receiver.
    /// - The sender and receiver are dropped before the event is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEvent;
    ///
    /// let mut event = Box::pin(OnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr_unchecked() };
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_ptr_unchecked(
        self: Pin<&Self>,
    ) -> (OnceSender<PtrEvent<T>>, OnceReceiver<PtrEvent<T>>) {
        // We use Relaxed because this does not affect any other state of the event.
        self.state.store(EVENT_BOUND, atomic::Ordering::Relaxed);

        let event_ptr = NonNull::from(self.get_ref());

        (
            OnceSender::new(PtrEvent { event: event_ptr }),
            OnceReceiver::new(PtrEvent { event: event_ptr }),
        )
    }

    /// Initializes the event in-place at a pinned location and returns both the sender and
    /// receiver for this event, connected by a raw pointer to the event.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The place the event is stored remains valid for the entire lifetime
    ///   of the sender and receiver.
    /// - The sender and receiver are dropped before the event is dropped.
    /// - The event is eventually dropped by its owner.
    #[must_use]
    #[inline]
    pub unsafe fn new_in_place_by_ptr(
        place: Pin<&mut MaybeUninit<Self>>,
    ) -> (OnceSender<PtrEvent<T>>, OnceReceiver<PtrEvent<T>>) {
        // SAFETY: We are not moving anything - in fact, there is nothing in there to move yet.
        let place = unsafe { place.get_unchecked_mut() };

        let event = place.write(Self::new_bound());

        let event_ptr = NonNull::from(event);

        (
            OnceSender::new(PtrEvent { event: event_ptr }),
            OnceReceiver::new(PtrEvent { event: event_ptr }),
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
        let backtrace = self.backtrace.lock().expect(ERR_POISONED_LOCK);
        f(backtrace.as_ref());
    }

    pub(crate) fn set(&self, result: T) {
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

        // A "set" operation is always a state increment. See `event_state.rs`.
        // We use `Release` ordering for the write because we are
        // releasing the synchronization block of `value`.
        let previous_state = self.state.fetch_add(1, atomic::Ordering::Release);

        match previous_state {
            EVENT_BOUND => {
                // Current state: EVENT_SET
                // There was nobody listening via the receiver - our work here is done.
            }
            EVENT_AWAITING => {
                // Current state: EVENT_SIGNALING
                // There was someone listening via the receiver. We need to
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
            }
            _ => {
                unreachable!("unreachable OnceEvent state on set: {previous_state}");
            }
        }
    }

    // We are intended to be polled via Future::poll, so we have an equivalent signature here.
    pub(crate) fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        #[cfg(debug_assertions)]
        self.backtrace
            .lock()
            .expect(ERR_POISONED_LOCK)
            .replace(capture_backtrace());

        // We use Acquire because we are (depending on the state) acquiring the synchronization
        // block for `value` and/or `awaiter`.
        match self.state.load(atomic::Ordering::Acquire) {
            EVENT_BOUND => self.poll_bound(waker),
            EVENT_SET => Some(Ok(self.poll_set())),
            EVENT_AWAITING => self.poll_awaiting(waker),
            EVENT_SIGNALING => self.poll_signaling(waker),
            EVENT_DISCONNECTED => {
                // There is no result coming, ever! This is the end.
                Some(Err(Disconnected))
            }
            state => {
                unreachable!("unreachable OnceEvent state on poll: {state}");
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
                    "unreachable OnceEvent state on poll state transition that followed EVENT_BOUND: {state}"
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

                self.poll_signaling(waker)
            }
            Err(state) => {
                unreachable!(
                    "unreachable OnceEvent state on poll state transition that followed EVENT_AWAITING: {state}"
                );
            }
        }
    }

    /// `poll()` impl for `EVENT_SIGNALING` state.
    fn poll_signaling(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        // This is pretty much a mutex - we are locked out of touching the event state until
        // the sender completes its state transition into either EVENT_SET or EVENT_DISCONNECTED.

        while self.state.load(atomic::Ordering::Relaxed) == EVENT_SIGNALING {
            // The sender-side transition is just a few instructions,
            // which should be near-instantaneous so we just spin.
            spin_loop();
        }

        // After the sender-side transition, we go back to the start and repeat the entire poll.
        self.poll(waker)
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

    pub(crate) fn sender_dropped_without_set(&self) {
        // We use Relaxed because we use fences to synchronize below.
        let previous_state = self
            .state
            .swap(EVENT_DISCONNECTED, atomic::Ordering::Relaxed);

        match previous_state {
            EVENT_BOUND => {
                // There was nobody listening via the receiver - our work here is done.
            }
            EVENT_AWAITING => {
                // There was someone listening via the receiver. We need to
                // notify the awaiter that they can come back for the disconnect signal now.

                // We need to acquire the synchronization block for the `awaiter`.
                atomic::fence(atomic::Ordering::Acquire);

                // Note that we do not need to go into the `EVENT_SIGNALING` state here
                // because the receiver will never set a new awaiter in the `EVENT_DISCONNECTED`
                // state.

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

                // Come and get it.
                //
                // As the event is multithreaded, the receiver may already have returned to us
                // before we send this wake signal - that is fine. If that happens, this signal
                // is simply a no-op.
                waker.wake();
            }
            _ => {
                unreachable!("unreachable OnceEvent state on disconnect: {previous_state}");
            }
        }
    }
}

impl<T> Default for OnceEvent<T>
where
    T: Send,
{
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: We are a synchronization primitive, so we do our own synchronization.
unsafe impl<T: Send> Sync for OnceEvent<T> {}

/// Enables a sender or receiver to reference the event that connects them.
///
/// This is a sealed trait and exists for internal use only. You never need to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait EventRef<T>: Deref<Target = OnceEvent<T>> + ReflectiveTSend + Sealed
where
    T: Send,
{
}

/// An event referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEvent`].
#[derive(Copy, Debug)]
pub struct RefEvent<'a, T>
where
    T: Send,
{
    event: &'a OnceEvent<T>,
}

impl<T> Sealed for RefEvent<'_, T> where T: Send {}
impl<T> EventRef<T> for RefEvent<'_, T> where T: Send {}
impl<T> Deref for RefEvent<'_, T>
where
    T: Send,
{
    type Target = OnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        self.event
    }
}
impl<T> Clone for RefEvent<'_, T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
impl<T: Send> ReflectiveTSend for RefEvent<'_, T> {
    type T = T;
}

/// An event referenced via `Arc` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEvent`].
#[derive(Debug)]
pub struct ArcEvent<T>
where
    T: Send,
{
    event: Arc<OnceEvent<T>>,
}

impl<T> Sealed for ArcEvent<T> where T: Send {}
impl<T> EventRef<T> for ArcEvent<T> where T: Send {}
impl<T> Deref for ArcEvent<T>
where
    T: Send,
{
    type Target = OnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}
impl<T> Clone for ArcEvent<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self {
            event: Arc::clone(&self.event),
        }
    }
}
impl<T: Send> ReflectiveTSend for ArcEvent<T> {
    type T = T;
}

/// An event referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`OnceEvent`].
#[derive(Copy, Debug)]
pub struct PtrEvent<T>
where
    T: Send,
{
    event: NonNull<OnceEvent<T>>,
}

impl<T> Sealed for PtrEvent<T> where T: Send {}
impl<T> EventRef<T> for PtrEvent<T> where T: Send {}
impl<T> Deref for PtrEvent<T>
where
    T: Send,
{
    type Target = OnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}
impl<T> Clone for PtrEvent<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
impl<T: Send> ReflectiveTSend for PtrEvent<T> {
    type T = T;
}
// SAFETY: This is only used with the thread-safe event (the event is Sync).
unsafe impl<T> Send for PtrEvent<T> where T: Send {}

/// An event stored on the heap, with automatically managed storage.
///
/// Only used in type names. Instances are created internally by [`OnceEvent`].
#[derive(Debug)]
pub struct HeapEvent<T>
where
    T: Send,
{
    event: NonNull<WithTwoOwners<OnceEvent<T>>>,
}

impl<T> HeapEvent<T>
where
    T: Send,
{
    fn new_pair() -> (Self, Self) {
        let event = NonNull::new(Box::into_raw(Box::new(WithTwoOwners::new(
            OnceEvent::new_bound(),
        ))))
        .expect("freshly allocated Box cannot be null");

        (Self { event }, Self { event })
    }
}

impl<T> Sealed for HeapEvent<T> where T: Send {}
impl<T> EventRef<T> for HeapEvent<T> where T: Send {}
impl<T> Deref for HeapEvent<T>
where
    T: Send,
{
    type Target = OnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        unsafe { self.event.as_ref() }
    }
}
impl<T> Clone for HeapEvent<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
impl<T> ReflectiveTSend for HeapEvent<T>
where
    T: Send,
{
    type T = T;
}
impl<T> Drop for HeapEvent<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // On drop, we need to coordinate with the `LocalWithTwoOwners` to ensure proper cleanup.
        // The last of either the sender or receiver will clean up the event.

        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        let event_wrapper = unsafe { self.event.as_ref() };

        if event_wrapper.release_one() {
            // This was the last reference - free the memory now.

            // SAFETY: Yes, it really is the type we are claiming it to be - we made it!
            drop(unsafe { Box::from_raw(self.event.as_ptr()) });
        }
    }
}

/// A sender that can send a single value through a thread-safe event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `OnceSender<ArcEvent<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
#[derive(Debug)]
pub struct OnceSender<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    event_ref: E,

    // We do not expect use cases that require Sync, so we suppress it to leave
    // design flexibility for future changes.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E> OnceSender<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn new(event_ref: E) -> Self {
        Self {
            event_ref,
            _not_sync: PhantomData,
        }
    }

    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEvent;
    ///
    /// let event = Arc::new(OnceEvent::<i32>::new());
    /// let (sender, _receiver) = event.bind_by_arc();
    /// sender.send(42);
    /// ```
    #[inline]
    pub fn send(self, value: E::T) {
        // Once we call set(), we no longer need to call the drop logic.
        let this = ManuallyDrop::new(self);

        this.event_ref.set(value);
    }
}

impl<E> Drop for OnceSender<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    #[inline]
    fn drop(&mut self) {
        self.event_ref.sender_dropped_without_set();
    }
}

/// A receiver that can receive a single value through a thread-safe event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `OnceReceiver<ArcEvent<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
#[derive(Debug)]
pub struct OnceReceiver<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    event_ref: E,

    // We do not expect use cases that require Sync, so we suppress it to leave
    // design flexibility for future changes.
    _not_sync: PhantomData<Cell<()>>,
}

impl<E> OnceReceiver<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    fn new(event_ref: E) -> Self {
        Self {
            event_ref,
            _not_sync: PhantomData,
        }
    }
}

impl<E> Future for OnceReceiver<E>
where
    E: EventRef<<E as ReflectiveTSend>::T>,
{
    type Output = Result<E::T, Disconnected>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        self.event_ref
            .poll(cx.waker())
            .map_or_else(|| task::Poll::Pending, task::Poll::Ready)
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::thread;

    use futures::task::noop_waker_ref;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::with_watchdog;

    use super::*;
    use crate::Disconnected;

    #[test]
    fn event_default_creates_valid_event() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<String>::default();
                let (sender, receiver) = event.bind_by_ref();
                sender.send("test".to_string());
                let value = receiver.await.unwrap();
                assert_eq!(value, "test");
            });
        });
    }

    #[test]
    fn event_by_ref_works() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<u64>::new();
                let (sender, receiver) = event.bind_by_ref();

                sender.send(123);
                let value = receiver.await.unwrap();
                assert_eq!(value, 123);
            });
        });
    }

    #[test]
    #[should_panic]
    fn event_by_ref_panics_on_second_call() {
        let event = OnceEvent::<i32>::new();
        let _endpoints = event.bind_by_ref();
        let _endpoints2 = event.bind_by_ref(); // Should panic
    }

    #[test]
    fn event_by_ref_checked_returns_none_after_use() {
        let event = OnceEvent::<i32>::new();
        let endpoints1 = event.bind_by_ref_checked();
        assert!(endpoints1.is_some());
        let endpoints2 = event.bind_by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn event_by_ref_unchecked_works() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<i32>::new();
                // SAFETY: We know this is the first and only binding of this event
                let (sender, receiver) = unsafe { event.bind_by_ref_unchecked() };

                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);
            });
        });
    }

    #[test]
    fn event_send_succeeds_without_receiver() {
        let event = OnceEvent::<i32>::new();
        let (sender, _receiver) = event.bind_by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn event_works_in_arc() {
        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_ref();

            sender.send("Hello from Arc".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from Arc");
        });
    }

    #[test]
    fn event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(OnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_ref();

            sender.send("Hello from Rc".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from Rc");
        });
    }

    #[test]
    fn event_by_arc_basic() {
        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_arc();

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_by_arc_checked_returns_none_after_use() {
        let event = Arc::new(OnceEvent::<i32>::new());
        let endpoints1 = event.bind_by_arc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.bind_by_arc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn event_by_arc_unchecked_works() {
        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<String>::new());
            // SAFETY: We know this is the first and only binding of this event
            let (sender, receiver) = unsafe { event.bind_by_arc_unchecked() };

            sender.send("Hello from Arc unchecked".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from Arc unchecked");
        });
    }

    #[test]
    #[should_panic]
    fn event_by_arc_panics_on_second_call() {
        let event = Arc::new(OnceEvent::<i32>::new());
        let _endpoints = event.bind_by_arc();
        let _endpoints2 = event.bind_by_arc(); // Should panic
    }

    #[test]
    fn event_by_arc_cross_thread() {
        with_watchdog(|| {
            let event = Arc::new(OnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_arc();

            let sender_handle = thread::spawn(move || {
                sender.send("Hello from Arc thread!".to_string());
            });

            let receiver_handle = thread::spawn(move || futures::executor::block_on(receiver));

            sender_handle.join().unwrap();
            let message = receiver_handle.join().unwrap().unwrap();
            assert_eq!(message, "Hello from Arc thread!");
        });
    }

    #[test]
    fn event_by_ptr_basic() {
        with_watchdog(|| {
            let event = Box::pin(OnceEvent::<String>::new());

            // SAFETY: We ensure the event is pinned and outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let event = Box::pin(OnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let endpoints = unsafe { event.as_ref().bind_by_ptr_checked() };
            assert!(endpoints.is_some());

            // Second call should return None
            // SAFETY: We ensure the event outlives the endpoints within this test
            let endpoints2 = unsafe { event.as_ref().bind_by_ptr_checked() };
            assert!(endpoints2.is_none());
        });
    }

    #[test]
    fn event_by_ptr_unchecked_works() {
        with_watchdog(|| {
            let event = Box::pin(OnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr_unchecked() };

            sender.send("Hello from pointer unchecked".to_string());
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, "Hello from pointer unchecked");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn event_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = OnceEvent::<i32>::new();
                let (sender, receiver) = event.bind_by_ref();

                // Drop the sender without sending anything
                drop(sender);

                // Receiver should get a Disconnected error
                let result = receiver.await;
                assert!(result.is_err());
                assert!(matches!(result, Err(Disconnected)));
            });
        });
    }

    #[test]
    fn sender_dropped_when_awaiting_signals_disconnected() {
        let event = OnceEvent::<i32>::new();
        let (sender, receiver) = event.bind_by_ref();

        let mut receiver = pin!(receiver);
        let mut context = task::Context::from_waker(noop_waker_ref());
        assert!(matches!(
            receiver.as_mut().poll(&mut context),
            task::Poll::Pending
        ));

        drop(sender);

        let mut context = task::Context::from_waker(noop_waker_ref());
        assert!(matches!(
            receiver.as_mut().poll(&mut context),
            task::Poll::Ready(Err(Disconnected))
        ));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_no_awaiter() {
        let event = OnceEvent::<i32>::new();
        let (_sender, _receiver) = event.bind_by_ref();

        let mut called = false;
        event.inspect_awaiter(|backtrace| {
            called = true;
            assert!(backtrace.is_none());
        });

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_with_awaiter() {
        let event = OnceEvent::<String>::new();
        let (_sender, receiver) = event.bind_by_ref();

        // Start polling to create an awaiter.
        let mut context = task::Context::from_waker(noop_waker_ref());
        let mut pinned_receiver = pin!(receiver);

        drop(pinned_receiver.as_mut().poll(&mut context));

        let mut called = false;
        event.inspect_awaiter(|backtrace| {
            called = true;

            // Should have Some(backtrace) when someone is awaiting.
            assert!(backtrace.is_some());
        });

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_after_completion() {
        let event = OnceEvent::<i32>::new();
        let (sender, receiver) = event.bind_by_ref();

        // Send value to complete the event
        sender.send(42);

        // Drop receiver to ensure no awaiter
        _ = receiver;

        let mut called = false;
        event.inspect_awaiter(|backtrace| {
            called = true;
            assert!(backtrace.is_none());
        });

        assert!(called);
    }

    #[test]
    fn thread_safety() {
        // The event is accessed across threads, so requires Sync as well as Send.
        assert_impl_all!(OnceEvent<u32>: Send, Sync);

        // These are all meant to be consumed ly - they may move between threads but are
        // not shared between threads, so Sync is not expected, only Send.
        assert_impl_all!(OnceSender<RefEvent<'static, u32>>: Send);
        assert_impl_all!(OnceReceiver<RefEvent<'static, u32>>: Send);
        assert_impl_all!(OnceSender<ArcEvent<u32>>: Send);
        assert_impl_all!(OnceReceiver<ArcEvent<u32>>: Send);
        assert_impl_all!(OnceSender<PtrEvent<u32>>: Send);
        assert_impl_all!(OnceReceiver<PtrEvent<u32>>: Send);
        assert_not_impl_any!(OnceSender<RefEvent<'static, u32>>: Sync);
        assert_not_impl_any!(OnceReceiver<RefEvent<'static, u32>>: Sync);
        assert_not_impl_any!(OnceSender<ArcEvent<u32>>: Sync);
        assert_not_impl_any!(OnceReceiver<ArcEvent<u32>>: Sync);
        assert_not_impl_any!(OnceSender<PtrEvent<u32>>: Sync);
        assert_not_impl_any!(OnceReceiver<PtrEvent<u32>>: Sync);
    }
}
