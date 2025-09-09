//! Single-threaded one-time events.
//!
//! This module provides single-threaded event types that have lower overhead
//! but cannot be shared across threads.

use std::alloc::{Layout, alloc, dealloc};
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::cell::{Cell, UnsafeCell};
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{ManuallyDrop, MaybeUninit, offset_of};
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::task;
use std::task::Waker;

#[cfg(debug_assertions)]
use crate::{BacktraceType, capture_backtrace};
use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, EVENT_UNBOUND,
    ReflectiveT, Sealed,
};

/// A one-time event that can send and receive a value of type `T` on a single thread.
///
/// The event can only be used once - after binding a sender and receiver,
/// subsequent bind calls will panic (or return [`None`] for the checked variants).
///
/// Similarly, sending a value will consume the sender, preventing further use.
/// You need to create a new event instance to create another sender-receiver pair.
///
/// To reuse event resources for many operations and avoid constantly recreating events, use
/// [`LocalOnceEventPool`][crate::LocalOnceEventPool].
///
/// For an event that can send the value across threads, see [`OnceEvent`][crate::OnceEvent].
///
/// # Example
///
/// ```rust
/// use events::LocalOnceEvent;
///
/// let event = LocalOnceEvent::<String>::new();
/// let (sender, receiver) = event.bind_by_ref();
///
/// sender.send("Hello".to_string());
/// let message = futures::executor::block_on(receiver).unwrap();
/// assert_eq!(message, "Hello");
/// ```
#[derive(Debug)]
pub struct LocalOnceEvent<T> {
    /// The logical state of the event; see `event_state.rs`.
    state: Cell<u8>,

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
    backtrace: RefCell<Option<BacktraceType>>,

    // Everything to do with this event is single-threaded,
    // even if T is thread-mobile or thread-safe.
    _single_threaded: PhantomData<*const ()>,

    // It is invalid to move this type once it has been pinned.
    _requires_pinning: PhantomPinned,
}

impl<T> LocalOnceEvent<T> {
    /// Creates a new single-threaded event.
    ///
    /// The event must be bound to a sender-receiver pair to be used.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// ```
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self {
            state: Cell::new(EVENT_UNBOUND),
            awaiter: UnsafeCell::new(MaybeUninit::uninit()),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            #[cfg(debug_assertions)]
            backtrace: RefCell::new(None),
            _single_threaded: PhantomData,
            _requires_pinning: PhantomPinned,
        }
    }

    /// In-place initializes a new single-threaded event that starts in the bound state.
    ///
    /// This is for internal use only - memory-managed events start in the bound state.
    pub(crate) fn new_in_place_bound(storage: &mut MaybeUninit<Self>) {
        // The key here is that we can skip initializing the MaybeUninit fields because
        // they start uninitialized by design and the UnsafeCell wrapper is transparent,
        // only affecting accesses and not the contents.
        let base_ptr = storage.as_mut_ptr();

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

    /// Creates a new single-threaded event with automatically managed heap storage,
    /// returning both the sender and receiver for this event.
    ///
    /// The memory used by the event is released when both endpoints are dropped.
    /// This is similar to `oneshot::channel()` but for single-threaded use cases.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let (sender, receiver) = LocalOnceEvent::<String>::new_managed();
    ///
    /// sender.send("Hello from the managed event!".to_string());
    /// let message = receiver.await.unwrap();
    /// assert_eq!(message, "Hello from the managed event!");
    /// # });
    /// ```
    #[must_use]
    #[inline]
    pub fn new_managed() -> (
        LocalOnceSender<ManagedLocalEvent<T>>,
        LocalOnceReceiver<ManagedLocalEvent<T>>,
    ) {
        let (sender_event, receiver_event) = ManagedLocalEvent::new_pair();

        (
            LocalOnceSender::new(sender_event),
            LocalOnceReceiver::new(receiver_event),
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// let (sender, receiver) = event.bind_by_ref();
    /// ```
    #[must_use]
    #[inline]
    pub fn bind_by_ref(
        &self,
    ) -> (
        LocalOnceSender<RefLocalEvent<'_, T>>,
        LocalOnceReceiver<RefLocalEvent<'_, T>>,
    ) {
        self.bind_by_ref_checked()
            .expect("LocalOnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by a shared reference to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// let endpoints = event.bind_by_ref_checked().unwrap();
    /// let endpoints2 = event.bind_by_ref_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    #[inline]
    pub fn bind_by_ref_checked(
        &self,
    ) -> Option<(
        LocalOnceSender<RefLocalEvent<'_, T>>,
        LocalOnceReceiver<RefLocalEvent<'_, T>>,
    )> {
        if self.state.get() != EVENT_UNBOUND {
            return None;
        }

        self.state.set(EVENT_BOUND);

        Some((
            LocalOnceSender::new(RefLocalEvent { event: self }),
            LocalOnceReceiver::new(RefLocalEvent { event: self }),
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
    /// use events::LocalOnceEvent;
    ///
    /// let event = LocalOnceEvent::<i32>::new();
    /// // We know this is the first and only binding of this event
    /// // SAFETY: We know this is the first and only binding of this event
    /// let (sender, receiver) = unsafe { event.bind_by_ref_unchecked() };
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_ref_unchecked(
        &self,
    ) -> (
        LocalOnceSender<RefLocalEvent<'_, T>>,
        LocalOnceReceiver<RefLocalEvent<'_, T>>,
    ) {
        self.state.set(EVENT_BOUND);

        (
            LocalOnceSender::new(RefLocalEvent { event: self }),
            LocalOnceReceiver::new(RefLocalEvent { event: self }),
        )
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Rc` to the event.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] when called.
    ///
    /// # Panics
    ///
    /// Panics if the event has already been bound to a sender-receiver pair.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let (sender, receiver) = event.bind_by_rc();
    /// ```
    #[must_use]
    #[inline]
    pub fn bind_by_rc(
        self: &Rc<Self>,
    ) -> (
        LocalOnceSender<RcLocalEvent<T>>,
        LocalOnceReceiver<RcLocalEvent<T>>,
    ) {
        self.bind_by_rc_checked()
            .expect("LocalOnceEvent has already been bound")
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Rc` to the event.
    ///
    /// Returns [`None`] if the event has already been bound to a sender-receiver pair.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] when called.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let endpoints = event.bind_by_rc_checked().unwrap();
    /// let endpoints2 = event.bind_by_rc_checked(); // Returns None
    /// assert!(endpoints2.is_none());
    /// ```
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "caller is expected to destructure and never to use this type"
    )]
    #[inline]
    pub fn bind_by_rc_checked(
        self: &Rc<Self>,
    ) -> Option<(
        LocalOnceSender<RcLocalEvent<T>>,
        LocalOnceReceiver<RcLocalEvent<T>>,
    )> {
        if self.state.get() != EVENT_UNBOUND {
            return None;
        }

        self.state.set(EVENT_BOUND);

        Some((
            LocalOnceSender::new(RcLocalEvent {
                event: Rc::clone(self),
            }),
            LocalOnceReceiver::new(RcLocalEvent {
                event: Rc::clone(self),
            }),
        ))
    }

    /// Returns both the sender and receiver for this event,
    /// connected by an `Rc` to the event.
    ///
    /// This method assumes the event is not already bound and skips the check for performance.
    ///
    /// This method requires the event to be wrapped in an [`Rc`] when called.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the event is not already bound.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// // We know this is the first and only binding of this event
    /// // SAFETY: We know this is the first and only binding of this event
    /// let (sender, receiver) = unsafe { event.bind_by_rc_unchecked() };
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_rc_unchecked(
        self: &Rc<Self>,
    ) -> (
        LocalOnceSender<RcLocalEvent<T>>,
        LocalOnceReceiver<RcLocalEvent<T>>,
    ) {
        self.state.set(EVENT_BOUND);

        (
            LocalOnceSender::new(RcLocalEvent {
                event: Rc::clone(self),
            }),
            LocalOnceReceiver::new(RcLocalEvent {
                event: Rc::clone(self),
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
    /// use events::LocalOnceEvent;
    ///
    /// let mut event = Box::pin(LocalOnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver, see below.
    /// let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver).unwrap();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before event
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_ptr(
        self: Pin<&Self>,
    ) -> (
        LocalOnceSender<PtrLocalEvent<T>>,
        LocalOnceReceiver<PtrLocalEvent<T>>,
    ) {
        // SAFETY: Caller has guaranteed event lifetime management
        unsafe { self.bind_by_ptr_checked() }.expect("LocalOnceEvent has already been bound")
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
    /// use events::LocalOnceEvent;
    ///
    /// let mut event = Box::pin(LocalOnceEvent::<i32>::new());
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
    ) -> Option<(
        LocalOnceSender<PtrLocalEvent<T>>,
        LocalOnceReceiver<PtrLocalEvent<T>>,
    )> {
        if self.state.get() != EVENT_UNBOUND {
            return None;
        }

        self.state.set(EVENT_BOUND);

        let event_ptr = NonNull::from(self.get_ref());

        Some((
            LocalOnceSender::new(PtrLocalEvent { event: event_ptr }),
            LocalOnceReceiver::new(PtrLocalEvent { event: event_ptr }),
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
    /// use events::LocalOnceEvent;
    ///
    /// let mut event = Box::pin(LocalOnceEvent::<i32>::new());
    /// // SAFETY: We ensure the event outlives the sender and receiver
    /// let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr_unchecked() };
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn bind_by_ptr_unchecked(
        self: Pin<&Self>,
    ) -> (
        LocalOnceSender<PtrLocalEvent<T>>,
        LocalOnceReceiver<PtrLocalEvent<T>>,
    ) {
        self.state.set(EVENT_BOUND);

        let event_ptr = NonNull::from(self.get_ref());

        (
            LocalOnceSender::new(PtrLocalEvent { event: event_ptr }),
            LocalOnceReceiver::new(PtrLocalEvent { event: event_ptr }),
        )
    }

    /// Initializes the event in-place at a pinned location and returns both the sender and
    /// receiver for this event, connected by a raw pointer to the event.
    ///
    /// This method is useful for high-performance scenarios where you want to avoid heap
    /// allocation and have precise control over memory layout.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The place the event is stored remains valid for the entire lifetime
    ///   of the sender and receiver.
    /// - The sender and receiver are dropped before the event is dropped.
    /// - The event is eventually dropped by its owner.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    /// use std::pin::pin;
    ///
    /// use events::LocalOnceEvent;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let mut event_storage = pin!(MaybeUninit::uninit());
    ///
    /// // SAFETY: We keep the event alive until sender/receiver are done
    /// let (sender, receiver) =
    ///     unsafe { LocalOnceEvent::<i32>::new_in_place_by_ptr(event_storage.as_mut()) };
    ///
    /// sender.send(42);
    /// let value = receiver.await.unwrap();
    /// assert_eq!(value, 42);
    ///
    /// // Both sender and receiver are dropped here, before we drop the event
    ///
    /// // SAFETY: We initialized it above and have dropped both sender and receiver
    /// unsafe {
    ///     event_storage.get_unchecked_mut().assume_init_drop();
    /// }
    /// # });
    /// ```
    #[must_use]
    #[inline]
    pub unsafe fn new_in_place_by_ptr(
        place: Pin<&mut MaybeUninit<Self>>,
    ) -> (
        LocalOnceSender<PtrLocalEvent<T>>,
        LocalOnceReceiver<PtrLocalEvent<T>>,
    ) {
        // SAFETY: We are not moving anything - in fact, there is nothing in there to move yet.
        let place = unsafe { place.get_unchecked_mut() };

        Self::new_in_place_bound(place);

        // We cast away the MaybeUninit here because it is now initialized.
        let event_ptr = NonNull::from(place).cast();

        (
            LocalOnceSender::new(PtrLocalEvent { event: event_ptr }),
            LocalOnceReceiver::new(PtrLocalEvent { event: event_ptr }),
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
        let backtrace = self.backtrace.borrow();
        f(backtrace.as_ref());
    }

    /// Sets the value of the event and notifies the awaiter, if there is one.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
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

        // A "set" operation is always a state increment. See `event_state.rs`.
        let previous_state = self.state.get();
        self.state.set(previous_state.wrapping_add(1));

        match previous_state {
            EVENT_BOUND => {
                // Current state: EVENT_SET
                // There was nobody listening via the receiver - our work here is done.
                // The receiver is still connected, so it will clean up.
                Ok(())
            }
            EVENT_AWAITING => {
                // Current state: EVENT_SIGNALING
                // There was someone listening via the receiver. We need to
                // notify the awaiter that they can come back for the value now.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell = unsafe {
                    self.awaiter
                        .get()
                        .as_mut()
                        .expect("UnsafeCell pointer is never null")
                };

                // We extract the waker and consider the field uninitialized again.
                // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
                let waker = unsafe { awaiter_cell.assume_init_read() };

                // Before sending the wake signal we must transition into `EVENT_SET` state, so
                // as soon as it wakes up it can grab the result. Granted, as this specific type
                // is a single-threaded signal, the order of operations does not actually matter.
                self.state.set(EVENT_SET);

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
                let value_cell = unsafe {
                    self.value
                        .get()
                        .as_mut()
                        .expect("UnsafeCell pointer is never null")
                };

                // We drop the value and consider the cell uninitialized.
                //
                // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
                unsafe {
                    value_cell.assume_init_drop();
                }

                Err(Disconnected)
            }
            _ => {
                unreachable!("unreachable LocalOnceEvent state on set: {previous_state}");
            }
        }
    }

    /// We are intended to be polled via `Future::poll`, so we have an equivalent signature here.
    ///
    /// If `Some` is returned, the caller is the last remaining endpoint and responsible
    /// for cleaning up the event.
    pub(crate) fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
        #[cfg(debug_assertions)]
        self.backtrace.replace(Some(capture_backtrace()));

        match self.state.get() {
            EVENT_BOUND => {
                // The sender has not yet set any value, so we will have to wait.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell = unsafe {
                    self.awaiter
                        .get()
                        .as_mut()
                        .expect("UnsafeCell pointer is never null")
                };

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
                let value_cell = unsafe {
                    self.value
                        .get()
                        .as_ref()
                        .expect("UnsafeCell pointer is never null")
                };

                // We extract the value and consider the cell uninitialized.
                //
                // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
                let value = unsafe { value_cell.assume_init_read() };

                Some(Ok(value))
            }
            EVENT_AWAITING => {
                // We are re-polling after previously starting a wait. This is fine
                // and we just need to clean up the previous waker, replacing it with
                // a new one.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell = unsafe {
                    self.awaiter
                        .get()
                        .as_mut()
                        .expect("UnsafeCell pointer is never null")
                };

                awaiter_cell.write(waker.clone());
                None
            }
            EVENT_DISCONNECTED => {
                // There is no result coming, ever! This is the end.
                Some(Err(Disconnected))
            }
            state => {
                unreachable!("unreachable LocalOnceEvent state on poll: {state}");
            }
        }
    }

    /// Marks the event as having been disconnected early from the sender side.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
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
                waker.wake();

                // The receiver is the last endpoint remaining, so it will clean up.
                Ok(())
            }
            EVENT_DISCONNECTED => {
                // The sender is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!(
                    "unreachable LocalOnceEvent state on sender disconnect: {previous_state}"
                );
            }
        }
    }

    /// Checks whether the event has been set (either with a value or with a disconnect signal).
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
    pub(crate) fn final_poll(&self) -> Result<Option<T>, Disconnected> {
        let previous_state = self.state.get();

        // We can immediately set this because this is a single-threaded event, so there cannot
        // be any race condition causing us issues with the receiver seeing this too early.
        self.state.set(EVENT_DISCONNECTED);

        match previous_state {
            EVENT_BOUND => {
                // The sender had not yet set any value. It will clean up the event later.
                Ok(None)
            }
            EVENT_SET => {
                // The sender has already set a value but we disconnected before we received it.
                // We need to clean up the value and then later clean up the event, as well.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let value_cell = unsafe {
                    self.value
                        .get()
                        .as_mut()
                        .expect("UnsafeCell pointer is never null")
                };

                // We extract the value and consider the cell uninitialized.
                //
                // SAFETY: We were in EVENT_SET which guarantees there is a value in there.
                let value = unsafe { value_cell.assume_init_read() };

                // The receiver will clean up.
                Ok(Some(value))
            }
            EVENT_AWAITING => {
                // We had previously started listening for the value but have not received one,
                // so the sender is the last endpoint remaining. We need to make sure we clean
                // up our old waker first, though, as the sender knows nothing about the waker
                // being present.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell = unsafe {
                    self.awaiter
                        .get()
                        .as_mut()
                        .expect("UnsafeCell pointer is never null")
                };

                // We drop the waker and consider the field uninitialized again.
                // SAFETY: We were in EVENT_AWAITING which guarantees there is a waker in there.
                unsafe {
                    awaiter_cell.assume_init_drop();
                }

                // The sender will clean up the event later.
                Ok(None)
            }
            EVENT_DISCONNECTED => {
                // The receiver is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!(
                    "unreachable LocalOnceEvent state on receiver disconnect: {previous_state}"
                );
            }
        }
    }
}

impl<T> Default for LocalOnceEvent<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Enables a sender or receiver to reference the event that connects them.
///
/// This is a sealed trait and exists for internal use only. You never need to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait LocalEventRef<T>:
    Deref<Target = LocalOnceEvent<T>> + ReflectiveT + LocalEventRefPrivate<T> + Sealed
{
}

trait LocalEventRefPrivate<T> {
    /// Releases the event, asserting that the last endpoint is being dropped.
    ///
    /// Depending on the resource management model of the reference, this may be a no-op.
    /// For example, if the event is managed via `Rc`, the reference count will simply be
    /// decremented to zero when the implementing type is dropped.
    ///
    /// This is primarily intended as a signal for manual resource management models.
    fn release_event(&self);
}

/// An event referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Copy, Debug)]
pub struct RefLocalEvent<'a, T> {
    event: &'a LocalOnceEvent<T>,
}

impl<T> Sealed for RefLocalEvent<'_, T> {}
impl<T> LocalEventRefPrivate<T> for RefLocalEvent<'_, T> {
    fn release_event(&self) {}
}
impl<T> LocalEventRef<T> for RefLocalEvent<'_, T> {}
impl<T> Deref for RefLocalEvent<'_, T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        self.event
    }
}
impl<T> Clone for RefLocalEvent<'_, T> {
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
impl<T> ReflectiveT for RefLocalEvent<'_, T> {
    type T = T;
}

/// An event referenced via `Rc` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Debug)]
pub struct RcLocalEvent<T> {
    event: Rc<LocalOnceEvent<T>>,
}

impl<T> Sealed for RcLocalEvent<T> {}
impl<T> LocalEventRefPrivate<T> for RcLocalEvent<T> {
    fn release_event(&self) {}
}
impl<T> LocalEventRef<T> for RcLocalEvent<T> {}
impl<T> Deref for RcLocalEvent<T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}
impl<T> Clone for RcLocalEvent<T> {
    fn clone(&self) -> Self {
        Self {
            event: Rc::clone(&self.event),
        }
    }
}
impl<T> ReflectiveT for RcLocalEvent<T> {
    type T = T;
}

/// An event referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Copy, Debug)]
pub struct PtrLocalEvent<T> {
    event: NonNull<LocalOnceEvent<T>>,
}

impl<T> Sealed for PtrLocalEvent<T> {}
impl<T> LocalEventRefPrivate<T> for PtrLocalEvent<T> {
    fn release_event(&self) {}
}
impl<T> LocalEventRef<T> for PtrLocalEvent<T> {}
impl<T> Deref for PtrLocalEvent<T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}
impl<T> Clone for PtrLocalEvent<T> {
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
impl<T> ReflectiveT for PtrLocalEvent<T> {
    type T = T;
}

/// An event stored on the heap, with automatically managed storage.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Debug)]
pub struct ManagedLocalEvent<T> {
    event: NonNull<LocalOnceEvent<T>>,
}

impl<T> ManagedLocalEvent<T> {
    fn new_pair() -> (Self, Self) {
        // SAFETY: The layout is correct for the type we are using - all is well.
        let event = NonNull::new(unsafe { alloc(Self::layout()) })
            .expect("memory allocation failed - fatal error")
            .cast();

        // SAFETY: MaybeUninit is a transparent wrapper, so the layout matches.
        // This is the only reference, so we have exclusive access rights.
        let event_as_maybe_uninit =
            unsafe { event.cast::<MaybeUninit<LocalOnceEvent<T>>>().as_mut() };

        LocalOnceEvent::new_in_place_bound(event_as_maybe_uninit);

        (Self { event }, Self { event })
    }

    const fn layout() -> Layout {
        Layout::new::<LocalOnceEvent<T>>()
    }
}

impl<T> LocalEventRefPrivate<T> for ManagedLocalEvent<T> {
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

impl<T> Sealed for ManagedLocalEvent<T> {}
impl<T> LocalEventRef<T> for ManagedLocalEvent<T> {}
impl<T> Deref for ManagedLocalEvent<T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        unsafe { self.event.as_ref() }
    }
}
impl<T> Clone for ManagedLocalEvent<T> {
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
impl<T> ReflectiveT for ManagedLocalEvent<T> {
    type T = T;
}

/// A sender that can send a single value through a single-threaded event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `LocalOnceSender<ArcEvent<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
#[derive(Debug)]
pub struct LocalOnceSender<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    event_ref: E,
}

impl<E> LocalOnceSender<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    fn new(event_ref: E) -> Self {
        Self { event_ref }
    }

    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::LocalOnceEvent;
    ///
    /// let event = Rc::new(LocalOnceEvent::<i32>::new());
    /// let (sender, _receiver) = event.bind_by_rc();
    /// sender.send(42);
    /// ```
    #[inline]
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

impl<E> Drop for LocalOnceSender<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    #[inline]
    fn drop(&mut self) {
        if self.event_ref.sender_dropped_without_set() == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            self.event_ref.release_event();
        }
    }
}

/// A receiver that can receive a single value through a single-threaded event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `LocalOnceReceiver<ArcEvent<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
#[derive(Debug)]
pub struct LocalOnceReceiver<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    // This is `None` if the receiver has already been polled to completion. We need to guard
    // against that because the event will already be cleaned up after the first poll completes.
    event_ref: Option<E>,
}

impl<E> LocalOnceReceiver<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    fn new(event_ref: E) -> Self {
        Self {
            event_ref: Some(event_ref),
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
    ///
    /// # Examples
    ///
    /// ## Basic usage with immediate value
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let (sender, receiver) = LocalOnceEvent::<String>::new_managed();
    /// sender.send("Hello".to_string());
    ///
    /// // Value is immediately available
    /// let value = receiver.into_value();
    /// assert_eq!(value, Some("Hello".to_string()));
    /// ```
    ///
    /// ## No value available
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let (_sender, receiver) = LocalOnceEvent::<i32>::new_managed();
    ///
    /// // No value sent yet
    /// let value = receiver.into_value();
    /// assert_eq!(value, None);
    /// ```
    ///
    /// ## Sender disconnected without sending
    ///
    /// ```rust
    /// use events::LocalOnceEvent;
    ///
    /// let (sender, receiver) = LocalOnceEvent::<String>::new_managed();
    /// drop(sender); // Disconnect without sending
    ///
    /// let value = receiver.into_value();
    /// assert_eq!(value, None);
    /// ```
    pub fn into_value(self) -> Option<E::T> {
        // This fn is a drop() implementation of sorts, so no need to run regular drop().
        let mut this = ManuallyDrop::new(self);

        let event_ref = this
            .event_ref
            .take()
            .expect("LocalOnceReceiver polled after completion");

        match event_ref.final_poll() {
            Ok(Some(value)) => {
                // The sender has disconnected and sent a value, so we need to clean up.
                event_ref.release_event();
                Some(value)
            }
            Ok(None) => {
                // Nothing for us to do - the sender was still connected and had not
                // sent any value, so it will perform the cleanup on its own.
                None
            }
            Err(Disconnected) => {
                // The sender has already disconnected, so we need to clean up the event.
                event_ref.release_event();
                None
            }
        }
    }
}

impl<E> Future for LocalOnceReceiver<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    type Output = Result<E::T, Disconnected>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("LocalOnceReceiver polled after completion");

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
            // This makes `drop()` a no-op.
            this.event_ref = None;
        }

        inner_poll_result.map_or_else(|| task::Poll::Pending, task::Poll::Ready)
    }
}

impl<E> Drop for LocalOnceReceiver<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
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
mod tests {
    use std::pin::pin;
    use std::rc::Rc;

    use futures::task::noop_waker_ref;
    use static_assertions::assert_not_impl_any;
    use testing::with_watchdog;

    use super::*;
    use crate::Disconnected;

    #[test]
    fn local_event_default_creates_valid_event() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<String>::default();

            let (sender, receiver) = event.bind_by_ref();

            sender.send("test".to_string());

            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "test");
        });
    }

    #[test]
    fn local_event_by_ref_works() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<u64>::new();
            let (sender, receiver) = event.bind_by_ref();

            // Receiver should not be ready before value is sent
            assert!(!receiver.is_ready());

            sender.send(123);

            // Receiver should be ready after value is sent
            assert!(receiver.is_ready());

            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 123);
        });
    }

    #[test]
    #[should_panic]
    fn local_event_by_ref_panics_on_second_call() {
        let event = LocalOnceEvent::<i32>::new();
        let _endpoints = event.bind_by_ref();
        let _endpoints2 = event.bind_by_ref(); // Should panic
    }

    #[test]
    fn local_event_by_ref_checked_returns_none_after_use() {
        let event = LocalOnceEvent::<i32>::new();
        let endpoints1 = event.bind_by_ref_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.bind_by_ref_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn local_event_by_ref_unchecked_works() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<i32>::new();
            // SAFETY: We know this is the first and only binding of this event
            let (sender, receiver) = unsafe { event.bind_by_ref_unchecked() };

            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 42);
        });
    }

    #[test]
    fn local_event_send_succeeds_without_receiver() {
        let event = LocalOnceEvent::<i32>::new();
        let (sender, _receiver) = event.bind_by_ref();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn local_event_works_in_rc() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_ref();

            sender.send("Hello from Rc".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from Rc");
        });
    }

    #[test]
    fn single_threaded_type() {
        assert_not_impl_any!(LocalOnceEvent<i32>: Send, Sync);
    }

    #[test]
    fn local_event_by_rc_basic() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<i32>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send(42);
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), 42);
        });
    }

    #[test]
    fn local_event_by_rc_checked_returns_none_after_use() {
        let event = Rc::new(LocalOnceEvent::<i32>::new());
        let endpoints1 = event.bind_by_rc_checked();
        assert!(endpoints1.is_some());

        let endpoints2 = event.bind_by_rc_checked();
        assert!(endpoints2.is_none());
    }

    #[test]
    fn local_event_by_rc_unchecked_works() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<String>::new());
            // SAFETY: We know this is the first and only binding of this event
            let (sender, receiver) = unsafe { event.bind_by_rc_unchecked() };

            sender.send("Hello from Rc unchecked".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from Rc unchecked");
        });
    }

    #[test]
    #[should_panic]
    fn local_event_by_rc_panics_on_second_call() {
        let event = Rc::new(LocalOnceEvent::<i32>::new());
        let _endpoints = event.bind_by_rc();
        let _endpoints2 = event.bind_by_rc(); // Should panic
    }

    #[test]
    fn local_event_by_rc_string() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_rc();

            sender.send("Hello from Rc".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from Rc");
        });
    }

    #[test]
    fn local_event_by_ptr_basic() {
        with_watchdog(|| {
            let event = Box::pin(LocalOnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver within this test
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };

            sender.send("Hello from pointer".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from pointer");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn local_event_by_ptr_checked_returns_none_after_use() {
        with_watchdog(|| {
            let event = Box::pin(LocalOnceEvent::<String>::new());

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
    fn local_event_by_ptr_unchecked_works() {
        with_watchdog(|| {
            let event = Box::pin(LocalOnceEvent::<String>::new());

            // SAFETY: We ensure the event outlives the sender and receiver
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr_unchecked() };

            sender.send("Hello from pointer unchecked".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from pointer unchecked");
            // sender and receiver are dropped here, before event
        });
    }

    #[test]
    fn local_event_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let event = LocalOnceEvent::<i32>::new();
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
        let event = LocalOnceEvent::<i32>::new();
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
        let event = LocalOnceEvent::<i32>::new();
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
        let event = LocalOnceEvent::<String>::new();
        let (_sender, receiver) = event.bind_by_ref();

        // Start polling to create an awaiter
        let mut context = task::Context::from_waker(noop_waker_ref());
        let mut pinned_receiver = pin!(receiver);

        drop(pinned_receiver.as_mut().poll(&mut context));

        let mut called = false;
        event.inspect_awaiter(|backtrace| {
            called = true;
            // Should have backtrace when someone is awaiting
            assert!(backtrace.is_some());
        });

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_after_completion() {
        let event = LocalOnceEvent::<i32>::new();
        let (sender, receiver) = event.bind_by_ref();

        // Send value to complete the event
        sender.send(42);

        // Drop receiver to ensure no awaiter
        drop(receiver);

        let mut called = false;
        event.inspect_awaiter(|backtrace| {
            called = true;
            assert!(backtrace.is_none());
        });

        assert!(called);
    }

    #[test]
    fn local_event_new_managed_basic() {
        with_watchdog(|| {
            let (sender, receiver) = LocalOnceEvent::<String>::new_managed();

            sender.send("Hello from heap!".to_string());
            let value = futures::executor::block_on(receiver);
            assert_eq!(value.unwrap(), "Hello from heap!");
        });
    }

    #[test]
    fn local_event_new_managed_without_receiver() {
        let (sender, _receiver) = LocalOnceEvent::<i32>::new_managed();

        // Send should still succeed even if we don't have a receiver
        sender.send(42);
    }

    #[test]
    fn local_event_new_managed_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let (sender, receiver) = LocalOnceEvent::<i32>::new_managed();

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
    fn local_event_new_managed_different_types() {
        with_watchdog(|| {
            // Test with different types to ensure generic functionality
            let (sender1, receiver1) = LocalOnceEvent::<u64>::new_managed();
            let (sender2, receiver2) = LocalOnceEvent::<Vec<String>>::new_managed();

            sender1.send(12345);
            sender2.send(vec!["hello".to_string(), "world".to_string()]);

            let value1 = futures::executor::block_on(receiver1);
            let value2 = futures::executor::block_on(receiver2);

            assert_eq!(value1.unwrap(), 12345);
            assert_eq!(
                value2.unwrap(),
                vec!["hello".to_string(), "world".to_string()]
            );
        });
    }

    #[test]
    fn local_event_new_in_place_by_ptr_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let mut event_storage = pin!(MaybeUninit::uninit());

                // SAFETY: We keep the event alive until sender/receiver are done
                let (sender, receiver) = unsafe {
                    LocalOnceEvent::<String>::new_in_place_by_ptr(event_storage.as_mut())
                };

                sender.send("Hello from in-place!".to_string());
                let value = receiver.await.unwrap();
                assert_eq!(value, "Hello from in-place!");

                // SAFETY: We initialized it above and have dropped both sender and receiver.
                let event_storage_ref = unsafe { event_storage.get_unchecked_mut() };

                // SAFETY: We initialized it above and have dropped both sender and receiver.
                unsafe {
                    event_storage_ref.assume_init_drop();
                }
            });
        });
    }

    #[test]
    fn local_event_new_in_place_by_ptr_without_receiver() {
        let mut event_storage = pin!(MaybeUninit::uninit());

        // SAFETY: We keep the event alive until sender/receiver are done
        let (sender, _receiver) =
            unsafe { LocalOnceEvent::<i32>::new_in_place_by_ptr(event_storage.as_mut()) };

        // Send should still succeed even if we don't have a receiver
        sender.send(42);

        // SAFETY: We initialized it above and have dropped both sender and receiver.
        let event_storage_ref = unsafe { event_storage.get_unchecked_mut() };

        // SAFETY: We initialized it above and have dropped both sender and receiver.
        unsafe {
            event_storage_ref.assume_init_drop();
        }
    }

    #[test]
    fn local_event_new_in_place_by_ptr_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let mut event_storage = pin!(MaybeUninit::uninit());

                // SAFETY: We keep the event alive until sender/receiver are done
                let (sender, receiver) =
                    unsafe { LocalOnceEvent::<i32>::new_in_place_by_ptr(event_storage.as_mut()) };

                // Drop the sender without sending anything
                drop(sender);

                // Receiver should get a Disconnected error
                let result = receiver.await;
                assert!(result.is_err());
                assert!(matches!(result, Err(Disconnected)));

                // SAFETY: We initialized it above and have dropped both sender and receiver.
                let event_storage_ref = unsafe { event_storage.get_unchecked_mut() };

                // SAFETY: We initialized it above and have dropped both sender and receiver.
                unsafe {
                    event_storage_ref.assume_init_drop();
                }
            });
        });
    }

    #[test]
    fn local_event_new_in_place_by_ptr_different_types() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let mut event_storage1 = pin!(MaybeUninit::uninit());
                let mut event_storage2 = pin!(MaybeUninit::uninit());

                // SAFETY: We keep the events alive until sender/receiver are done
                let (sender1, receiver1) =
                    unsafe { LocalOnceEvent::<u64>::new_in_place_by_ptr(event_storage1.as_mut()) };
                // SAFETY: We keep the events alive until sender/receiver are done.
                let (sender2, receiver2) = unsafe {
                    LocalOnceEvent::<Vec<i32>>::new_in_place_by_ptr(event_storage2.as_mut())
                };

                sender1.send(98765);
                sender2.send(vec![1, 2, 3, 4, 5]);

                let value1 = receiver1.await.unwrap();
                let value2 = receiver2.await.unwrap();

                assert_eq!(value1, 98765);
                assert_eq!(value2, vec![1, 2, 3, 4, 5]);

                // SAFETY: We initialized them above and have dropped both sender and receiver.
                let event_storage1_ref = unsafe { event_storage1.get_unchecked_mut() };
                // SAFETY: We initialized them above and have dropped both sender and receiver.
                let event_storage2_ref = unsafe { event_storage2.get_unchecked_mut() };

                // SAFETY: We initialized them above and have dropped both sender and receiver.
                unsafe {
                    event_storage1_ref.assume_init_drop();
                }

                // SAFETY: We initialized them above and have dropped both sender and receiver.
                unsafe {
                    event_storage2_ref.assume_init_drop();
                }
            });
        });
    }

    #[test]
    fn local_receiver_into_value_with_sent_value() {
        with_watchdog(|| {
            let (sender, receiver) = LocalOnceEvent::<String>::new_managed();
            sender.send("test value".to_string());

            let result = receiver.into_value();
            assert_eq!(result, Some("test value".to_string()));
        });
    }

    #[test]
    fn local_receiver_into_value_no_value_sent() {
        with_watchdog(|| {
            let (_sender, receiver) = LocalOnceEvent::<i32>::new_managed();

            let result = receiver.into_value();
            assert_eq!(result, None);
        });
    }

    #[test]
    fn local_receiver_into_value_sender_disconnected() {
        with_watchdog(|| {
            let (sender, receiver) = LocalOnceEvent::<String>::new_managed();
            drop(sender); // Disconnect without sending

            let result = receiver.into_value();
            assert_eq!(result, None);
        });
    }

    #[test]
    fn local_receiver_into_value_with_ref_event() {
        with_watchdog(|| {
            let event = LocalOnceEvent::<i32>::new();
            let (sender, receiver) = event.bind_by_ref();
            sender.send(42);

            let result = receiver.into_value();
            assert_eq!(result, Some(42));
        });
    }

    #[test]
    fn local_receiver_into_value_with_rc_event() {
        with_watchdog(|| {
            let event = Rc::new(LocalOnceEvent::<String>::new());
            let (sender, receiver) = event.bind_by_rc();
            sender.send("rc test".to_string());

            let result = receiver.into_value();
            assert_eq!(result, Some("rc test".to_string()));
        });
    }

    #[test]
    fn local_receiver_into_value_with_ptr_event() {
        with_watchdog(|| {
            let event = Box::pin(LocalOnceEvent::<i32>::new());
            // SAFETY: Event is pinned and outlives the sender/receiver.
            let (sender, receiver) = unsafe { event.as_ref().bind_by_ptr() };
            sender.send(999);

            let result = receiver.into_value();
            assert_eq!(result, Some(999));
        });
    }

    #[test]
    #[should_panic(expected = "LocalOnceReceiver polled after completion")]
    fn local_receiver_into_value_panics_after_poll() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let (sender, mut receiver) = LocalOnceEvent::<i32>::new_managed();
                sender.send(42);

                // Poll the receiver first
                let waker = noop_waker_ref();
                let mut context = task::Context::from_waker(waker);
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "testing poll result ignored"
                )]
                let _ = Pin::new(&mut receiver).poll(&mut context);

                // This should panic
                let _ = receiver.into_value();
            });
        });
    }

    #[test]
    fn local_receiver_into_value_multiple_event_types() {
        with_watchdog(|| {
            // Test with different value types
            let (sender1, receiver1) = LocalOnceEvent::<()>::new_managed();
            sender1.send(());
            assert_eq!(receiver1.into_value(), Some(()));

            let (sender2, receiver2) = LocalOnceEvent::<Vec<i32>>::new_managed();
            sender2.send(vec![1, 2, 3]);
            assert_eq!(receiver2.into_value(), Some(vec![1, 2, 3]));

            let (sender3, receiver3) = LocalOnceEvent::<Option<String>>::new_managed();
            sender3.send(Some("nested option".to_string()));
            assert_eq!(
                receiver3.into_value(),
                Some(Some("nested option".to_string()))
            );
        });
    }

    #[test]
    fn thread_safety() {
        // Nothing is Send or Sync - everything is stuck on one thread.
        assert_not_impl_any!(LocalOnceEvent<u32>: Send, Sync);
        assert_not_impl_any!(LocalOnceSender<RefLocalEvent<'static, u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceReceiver<RefLocalEvent<'static, u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceSender<RcLocalEvent<u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceReceiver<RcLocalEvent<u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceSender<PtrLocalEvent<u32>>: Send, Sync);
        assert_not_impl_any!(LocalOnceReceiver<PtrLocalEvent<u32>>: Send, Sync);
    }
}
