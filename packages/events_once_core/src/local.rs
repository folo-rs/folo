use std::alloc::{Layout, alloc, dealloc};
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{ManuallyDrop, MaybeUninit, offset_of};
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::task::{self, Poll, Waker};

#[cfg(debug_assertions)]
use crate::{BacktraceType, capture_backtrace};
use crate::{
    Disconnected, EVENT_AWAITING, EVENT_BOUND, EVENT_DISCONNECTED, EVENT_SET, ReflectiveT, Sealed,
};

/// Coordinates delivery of a `T` at most once from a sender to a receiver on the same thread.
///
/// This is a low level synchronization primitive intended for building higher-level synchronization
/// primitives, so the API is quite raw and non-ergonomic. Real end-users are expected to use the
/// next level of abstraction instead, such as the ones in the `events` package.
pub struct LocalEvent<T> {
    /// The logical state of the event; see constants in `state.rs`.
    state: Cell<u8>,

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

impl<T> LocalEvent<T> {
    /// In-place initializes a new instance in the `BOUND` state.
    ///
    /// This is for internal use only and is wrapped by public methods that also
    /// wire up the sender and receiver after doing the initialization. An event
    /// without a sender and receiver is invalid.
    fn new_in_inner(place: &mut MaybeUninit<Self>) -> NonNull<Self> {
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

    /// Heap-allocates a new instance and returns the endpoints.
    ///
    /// The memory used is released when both endpoints are dropped.
    ///
    /// For more efficiency, consider using [`new_in`][Self::new_in], which allows you to
    /// initialize the event in preallocated storage as part of a larger structure.
    #[must_use]
    pub fn boxed() -> (
        LocalSender<BoxedLocalRef<T>>,
        LocalReceiver<BoxedLocalRef<T>>,
    ) {
        let (sender_event, receiver_event) = BoxedLocalRef::new_pair();

        (
            LocalSender::new(sender_event),
            LocalReceiver::new(receiver_event),
        )
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
    ) -> (LocalSender<PtrLocalRef<T>>, LocalReceiver<PtrLocalRef<T>>) {
        // SAFETY: Nothing is getting moved, we just temporarily unwrap the Pin wrapper.
        let event = Self::new_in_inner(unsafe { place.get_unchecked_mut() });

        (
            LocalSender::new(PtrLocalRef { event }),
            LocalReceiver::new(PtrLocalRef { event }),
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
    fn set(&self, result: T) -> Result<(), Disconnected> {
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

        let previous_state = self.state.get();

        match previous_state {
            EVENT_BOUND => {
                self.state.set(EVENT_SET);

                // There was nobody listening via the receiver - our work here is done.
                // The receiver is still connected, so it will clean up.
                Ok(())
            }
            EVENT_AWAITING => {
                // There was someone listening via the receiver. We need to
                // notify the awaiter that they can come back for the value now.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

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
    #[must_use]
    fn poll(&self, waker: &Waker) -> Option<Result<T, Disconnected>> {
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
                // a new one.

                // SAFETY: The only other potential references to the field are other short-lived
                // references in this type, which cannot exist at the moment because
                // the type is single-threaded and does not let any references escape.
                let awaiter_cell_maybe = unsafe { self.awaiter.get().as_mut() };
                // SAFETY: UnsafeCell pointer is never null.
                let awaiter_cell = unsafe { awaiter_cell_maybe.unwrap_unchecked() };

                awaiter_cell.write(waker.clone());
                None
            }
            EVENT_DISCONNECTED => {
                // There is no result coming, ever! This is the end.
                Some(Err(Disconnected))
            }
            state => {
                unreachable!("unreachable LocalEvent state on poll: {state}");
            }
        }
    }

    /// Marks the event as having been disconnected early from the sender side.
    ///
    /// Returns `Err` if the receiver has already disconnected and we must clean up the event now.
    fn sender_dropped_without_set(&self) -> Result<(), Disconnected> {
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
            _ => {
                unreachable!("unreachable LocalEvent state on sender disconnect: {previous_state}");
            }
        }
    }

    /// Checks whether the event has been set (either with a value or with a disconnect signal).
    #[must_use]
    fn is_set(&self) -> bool {
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
    fn final_poll(&self) -> Result<Option<T>, Disconnected> {
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
            EVENT_AWAITING => {
                // We had previously started listening for the value but have not received one,
                // so the sender is the last endpoint remaining. We need to make sure we clean
                // up our old waker first, though, as the sender knows nothing about the waker
                // being present.

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

                // The sender will clean up the event later.
                Ok(None)
            }
            EVENT_DISCONNECTED => {
                // The receiver is the last endpoint remaining, so it will clean up.
                Err(Disconnected)
            }
            _ => {
                unreachable!(
                    "unreachable LocalEvent state on receiver disconnect: {previous_state}"
                );
            }
        }
    }
}

#[expect(clippy::missing_fields_in_debug, reason = "phantoms are boring")]
impl<T> fmt::Debug for LocalEvent<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("LocalEvent");

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

/// Enables a sender or receiver to reference the event that connects them.
///
/// This is a sealed trait and exists for internal use only. User code never needs to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait LocalRef<T>:
    Deref<Target = LocalEvent<T>> + ReflectiveT + LocalRefPrivate<T> + Sealed + fmt::Debug
{
}

trait LocalRefPrivate<T> {
    /// Releases the event, asserting that the last endpoint has been dropped
    /// and nothing will access the event after this call.
    fn release_event(&self);
}

/// References an event stored anywhere, via raw pointer.
///
/// Only used in type names. Instances are created by [`LocalEvent`].
pub struct PtrLocalRef<T> {
    event: NonNull<LocalEvent<T>>,
}

impl<T> Sealed for PtrLocalRef<T> {}
impl<T> LocalRefPrivate<T> for PtrLocalRef<T> {
    fn release_event(&self) {}
}
impl<T> LocalRef<T> for PtrLocalRef<T> {}
impl<T> Deref for PtrLocalRef<T> {
    type Target = LocalEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the event outlives it.
        unsafe { self.event.as_ref() }
    }
}
impl<T> ReflectiveT for PtrLocalRef<T> {
    type T = T;
}
impl<T> fmt::Debug for PtrLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtrLocalRef")
            .field("event", &self.event)
            .finish()
    }
}

/// References an event stored on the heap.
///
/// Only used in type names. Instances are created internally by [`LocalEvent`].
pub struct BoxedLocalRef<T> {
    event: NonNull<LocalEvent<T>>,
}

impl<T> BoxedLocalRef<T> {
    #[must_use]
    fn new_pair() -> (Self, Self) {
        // SAFETY: The layout is correct for the type we are using - all is well.
        let event = NonNull::new(unsafe { alloc(Self::layout()) })
            .expect("memory allocation failed - fatal error")
            .cast();

        // SAFETY: MaybeUninit is a transparent wrapper, so the layout matches.
        // This is the only reference, so we have exclusive access rights.
        let event_as_maybe_uninit = unsafe { event.cast::<MaybeUninit<LocalEvent<T>>>().as_mut() };

        LocalEvent::new_in_inner(event_as_maybe_uninit);

        (Self { event }, Self { event })
    }

    const fn layout() -> Layout {
        Layout::new::<LocalEvent<T>>()
    }
}

impl<T> LocalRefPrivate<T> for BoxedLocalRef<T> {
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

impl<T> Sealed for BoxedLocalRef<T> {}
impl<T> LocalRef<T> for BoxedLocalRef<T> {}
impl<T> Deref for BoxedLocalRef<T> {
    type Target = LocalEvent<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Storage is automatically managed - as long as either sender/receiver
        // are alive, we are guaranteed that the event is alive.
        unsafe { self.event.as_ref() }
    }
}
impl<T> ReflectiveT for BoxedLocalRef<T> {
    type T = T;
}
impl<T> fmt::Debug for BoxedLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxedLocalRef")
            .field("event", &self.event)
            .finish()
    }
}

/// Delivers a single value to the receiver connected to the same event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `LocalSender<BoxedLocalRef<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
pub struct LocalSender<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    event_ref: E,
}

impl<E> LocalSender<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    #[must_use]
    fn new(event_ref: E) -> Self {
        Self { event_ref }
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

impl<E> Drop for LocalSender<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    fn drop(&mut self) {
        if self.event_ref.sender_dropped_without_set() == Err(Disconnected) {
            // The other endpoint has disconnected, so we need to clean up the event.
            self.event_ref.release_event();
        }
    }
}

impl<E> fmt::Debug for LocalSender<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalSender")
            .field("event_ref", &self.event_ref)
            .finish()
    }
}

/// Receives a single value from the sender connected to the same event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `LocalReceiver<BoxedLocalRef<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event.
/// Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
pub struct LocalReceiver<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    // This is `None` if the receiver has already been polled to completion. We need to guard
    // against that because the event will be cleaned up after the first poll that signals "ready".
    event_ref: Option<E>,
}

impl<E> LocalReceiver<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
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
    #[must_use]
    pub fn is_ready(&self) -> bool {
        let Some(event_ref) = &self.event_ref else {
            panic!("receiver queried after completion");
        };

        event_ref.is_set()
    }

    /// Consumes the receiver and transforms it into the received value, if the value is available.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available value without blocking. It returns `Ok(value)` if a value has
    /// already been sent, or returns the receiver if no value is currently available.
    ///
    /// # Panics
    ///
    /// Panics if the value has already been received via `Future::poll()`.
    pub fn into_value(self) -> Result<Result<E::T, Disconnected>, Self> {
        let event_ref = self
            .event_ref
            .as_ref()
            .expect("LocalOnceReceiver polled after completion");

        // Check the current state directly to decide what to do
        let current_state = event_ref.state.get();

        match current_state {
            EVENT_BOUND | EVENT_AWAITING => {
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

impl<E> Future for LocalReceiver<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    type Output = Result<E::T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
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

        inner_poll_result.map_or_else(|| Poll::Pending, Poll::Ready)
    }
}

impl<E> Drop for LocalReceiver<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
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

impl<E> fmt::Debug for LocalReceiver<E>
where
    E: LocalRef<<E as ReflectiveT>::T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalReceiver")
            .field("event_ref", &self.event_ref)
            .finish()
    }
}

#[cfg(test)]
#[expect(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
mod tests {
    use std::pin::pin;

    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(LocalEvent<i32>: Send, Sync);
    assert_not_impl_any!(BoxedLocalRef<i32>: Send, Sync);
    assert_not_impl_any!(PtrLocalRef<i32>: Send, Sync);
    assert_not_impl_any!(LocalSender<BoxedLocalRef<i32>>: Send, Sync);
    assert_not_impl_any!(LocalReceiver<BoxedLocalRef<i32>>: Send, Sync);
    assert_not_impl_any!(LocalSender<PtrLocalRef<i32>>: Send, Sync);
    assert_not_impl_any!(LocalReceiver<PtrLocalRef<i32>>: Send, Sync);

    #[test]
    fn boxed_send_receive() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_send_receive_unit() {
        let (sender, receiver) = LocalEvent::<()>::boxed();
        let mut receiver = pin!(receiver);

        sender.send(());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn boxed_send_receive_u128() {
        let (sender, receiver) = LocalEvent::<u128>::boxed();
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn boxed_send_receive_array() {
        let (sender, receiver) = LocalEvent::<[u128; 4]>::boxed();
        let mut receiver = pin!(receiver);

        sender.send([42, 43, 44, 45]);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok([42, 43, 44, 45]))));
    }

    #[test]
    fn boxed_receive_send_receive() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
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
        let (sender, _) = LocalEvent::<i32>::boxed();

        sender.send(42);
    }

    #[test]
    fn boxed_drop_receive() {
        let (_, receiver) = LocalEvent::<i32>::boxed();
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn boxed_receive_drop_receive() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
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
        let (sender, receiver) = LocalEvent::<i32>::boxed();
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
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        let Err(receiver) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(Ok(42))));
    }

    #[test]
    fn boxed_drop_into_value() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();

        drop(sender);

        assert!(matches!(receiver.into_value(), Ok(Err(Disconnected))));
    }

    #[test]
    #[should_panic]
    fn boxed_panic_poll_after_completion() {
        let (sender, receiver) = LocalEvent::<i32>::boxed();
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
        let (sender, receiver) = LocalEvent::<i32>::boxed();
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn placed_receive_send_receive() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, _) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        sender.send(42);
    }

    #[test]
    fn placed_drop_receive() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (_, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn placed_receive_drop_receive() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn placed_drop_drop_sender_first() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn placed_is_ready() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let Err(receiver) = receiver.into_value() else {
            panic!("expected no value yet");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(Ok(42))));
    }

    #[test]
    fn placed_drop_into_value() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        drop(sender);

        assert!(matches!(receiver.into_value(), Ok(Err(Disconnected))));
    }

    #[test]
    #[should_panic]
    fn placed_panic_poll_after_completion() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
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
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };
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

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_no_awaiter() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let _endpoints = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let mut called = false;
        unsafe { place.as_ref().get_ref().assume_init_ref() }.inspect_awaiter(|backtrace| {
            called = true;
            assert!(backtrace.is_none());
        });

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_with_awaiter() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (_sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = pin!(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        let mut called = false;
        unsafe { place.as_ref().get_ref().assume_init_ref() }.inspect_awaiter(|backtrace| {
            called = true;
            assert!(backtrace.is_some());
        });

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_after_sender_drop() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = pin!(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(sender);

        let mut called = false;
        unsafe { place.as_ref().get_ref().assume_init_ref() }.inspect_awaiter(|backtrace| {
            called = true;
            assert!(backtrace.is_some());
        });

        assert!(called);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiter_after_receiver_drop() {
        let mut place = Box::pin(MaybeUninit::<LocalEvent<i32>>::uninit());
        let (_sender, receiver) = unsafe { LocalEvent::<i32>::placed(place.as_mut()) };

        let mut cx = task::Context::from_waker(Waker::noop());
        let mut receiver = Box::pin(receiver);
        _ = receiver.as_mut().poll(&mut cx);

        drop(receiver);

        let mut called = false;
        unsafe { place.as_ref().get_ref().assume_init_ref() }.inspect_awaiter(|backtrace| {
            called = true;
            assert!(backtrace.is_some());
        });

        assert!(called);
    }
}
