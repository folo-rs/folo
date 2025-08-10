//! Single-threaded one-time events.
//!
//! This module provides single-threaded event types that have lower overhead
//! but cannot be shared across threads.

#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::cell::{Cell, UnsafeCell};
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task;
use std::task::Waker;

#[cfg(debug_assertions)]
use crate::{BacktraceType, capture_backtrace};
use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, EVENT_UNBOUND,
    LocalWithTwoOwners, ReflectiveT, Sealed,
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

    /// Creates a new single-threaded event that starts in the bound state.
    ///
    /// This is for internal use only - pooled events start in the bound state.
    #[must_use]
    pub(crate) fn new_bound() -> Self {
        Self {
            state: Cell::new(EVENT_BOUND),
            awaiter: UnsafeCell::new(MaybeUninit::uninit()),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            #[cfg(debug_assertions)]
            backtrace: RefCell::new(None),
            _single_threaded: PhantomData,
            _requires_pinning: PhantomPinned,
        }
    }

    /// Creates a new heap-allocated single-threaded event, returning both the sender and receiver
    /// for this event.
    ///
    /// The memory used by the event is automatically released when both endpoints are dropped.
    #[must_use]
    #[inline]
    pub fn new_heap() -> (
        LocalOnceSender<HeapLocalEvent<T>>,
        LocalOnceReceiver<HeapLocalEvent<T>>,
    ) {
        let (sender_event, receiver_event) = HeapLocalEvent::new_pair();

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
    /// let (sender, receiver) = event.bind_by_ref_unchecked();
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
    /// let (sender, receiver) = event.bind_by_rc_unchecked();
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
    ) -> (
        LocalOnceSender<PtrLocalEvent<T>>,
        LocalOnceReceiver<PtrLocalEvent<T>>,
    ) {
        // SAFETY: We are not moving anything - in fact, there is nothing in there to move yet.
        let place = unsafe { place.get_unchecked_mut() };

        let event = place.write(Self::new_bound());

        let event_ptr = NonNull::from(event);

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

    pub(crate) fn set(&self, result: T) {
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
            }
            _ => {
                unreachable!("unreachable LocalOnceEvent state on set: {previous_state}");
            }
        }
    }

    /// We are intended to be polled via `Future::poll`, so we have an equivalent signature here.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been consumed.
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

    pub(crate) fn sender_dropped_without_set(&self) {
        let previous_state = self.state.get();

        // We can immediately set this because this is a single-threaded event, so there cannot
        // be any race condition causing us issues with the receiver seeing this too early.
        self.state.set(EVENT_DISCONNECTED);

        match previous_state {
            EVENT_BOUND => {
                // There was nobody listening via the receiver - our work here is done.
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
            }
            _ => {
                unreachable!("unreachable LocalOnceEvent state on disconnect: {previous_state}");
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
pub trait LocalEventRef<T>: Deref<Target = LocalOnceEvent<T>> + ReflectiveT + Sealed {}

/// An event referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEvent`].
#[derive(Copy, Debug)]
pub struct RefLocalEvent<'a, T> {
    event: &'a LocalOnceEvent<T>,
}

impl<T> Sealed for RefLocalEvent<'_, T> {}
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
pub struct HeapLocalEvent<T> {
    event: NonNull<LocalWithTwoOwners<LocalOnceEvent<T>>>,
}

impl<T> HeapLocalEvent<T> {
    fn new_pair() -> (Self, Self) {
        let event = NonNull::new(Box::into_raw(Box::new(LocalWithTwoOwners::new(
            LocalOnceEvent::new_bound(),
        ))))
        .expect("freshly allocated Box cannot be null");

        (Self { event }, Self { event })
    }
}

impl<T> Sealed for HeapLocalEvent<T> {}
impl<T> LocalEventRef<T> for HeapLocalEvent<T> {}
impl<T> Deref for HeapLocalEvent<T> {
    type Target = LocalOnceEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        unsafe { self.event.as_ref() }
    }
}
impl<T> Clone for HeapLocalEvent<T> {
    fn clone(&self) -> Self {
        Self { event: self.event }
    }
}
impl<T> ReflectiveT for HeapLocalEvent<T> {
    type T = T;
}
impl<T> Drop for HeapLocalEvent<T> {
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
        // Once we call set(), we no longer need to call the drop logic.
        let this = ManuallyDrop::new(self);

        this.event_ref.set(value);
    }
}

impl<E> Drop for LocalOnceSender<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    #[inline]
    fn drop(&mut self) {
        self.event_ref.sender_dropped_without_set();
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
    event_ref: E,
}

impl<E> LocalOnceReceiver<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
{
    fn new(event_ref: E) -> Self {
        Self { event_ref }
    }
}

impl<E> Future for LocalOnceReceiver<E>
where
    E: LocalEventRef<<E as ReflectiveT>::T>,
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

            sender.send(123);
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
