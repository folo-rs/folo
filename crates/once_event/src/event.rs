use std::cell::UnsafeCell;
use std::future::Future;
use std::marker::PhantomPinned;
use std::mem;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{self, Waker};

use crate::{PinnedRcBox, PinnedRcStorage, RcPinnedRc, RefPinnedRc, UnsafePinnedRc, WithRefCount};

/// An asynchronous event that can be triggered at most once to deliver a value of type T to at most
/// one listener awaiting that value.
///
/// This is the core implementation that handles the actual event state and logic.
/// It is not directly instantiated by users - instead, use one of the storage types
/// like `OnceEventEmbedded`, `OnceEventPoolByRef`, etc.
///
/// # Thread safety
///
/// This type is `?Send + !Sync`. It is safe to send across threads as long as `T` supports that.
/// Methods on this type may not be called across threads.
///
/// See `OnceEventShared` for a thread-safe version.
#[derive(Debug)]
struct OnceEventCore<T> {
    // We only have a get() and a set() that access the state and we guarantee this happens on the
    // same thread (because UnsafeCell is !Sync), so there is no point in wasting cycles on borrow
    // counting at runtime with RefCell.
    state: UnsafeCell<EventState<T>>,
}

impl<T> OnceEventCore<T> {
    fn set(&self, result: T) {
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

        match &*state {
            EventState::NotSet => {
                *state = EventState::Set(result);
            }
            EventState::Awaiting(_) => {
                let previous_state = mem::replace(&mut *state, EventState::Set(result));

                match previous_state {
                    EventState::Awaiting(waker) => waker.wake(),
                    _ => unreachable!("we are re-matching an already matched pattern"),
                }
            }
            EventState::Set(_) => {
                panic!("result already set");
            }
            EventState::Consumed => {
                panic!("result already consumed");
            }
        }
    }

    // We are intended to be polled via Future::poll, so we have an equivalent signature here.
    fn poll(&self, waker: &Waker) -> Option<T> {
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

        match &*state {
            EventState::NotSet => {
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Awaiting(_) => {
                // This is permitted by the Future API contract, in which case only the waker
                // from the most recent poll should be woken up when the result is available.
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Set(_) => {
                let previous_state = mem::replace(&mut *state, EventState::Consumed);

                match previous_state {
                    EventState::Set(result) => Some(result),
                    _ => unreachable!("we are re-matching an already matched pattern"),
                }
            }
            EventState::Consumed => {
                // We do not want to keep a copy of the result around, so we can only return it once.
                // The futures API contract allows us to panic in this situation.
                panic!("event polled after result was already consumed");
            }
        }
    }

    fn new() -> Self {
        Self {
            state: UnsafeCell::new(EventState::NotSet),
        }
    }
}

#[derive(Debug)]
enum EventState<T> {
    /// The event has not been set and nobody is listening for a result.
    NotSet,

    /// The event has not been set and someone is listening for a result.
    Awaiting(Waker),

    /// The event has been set but nobody has yet started listening.
    Set(T),

    /// The event has been set and the result has been consumed.
    Consumed,
}

// ############## Embedded Storage ##############

/// Embedded storage for a single `OnceEvent` that can be embedded directly into a data structure
/// owned by the caller.
///
/// # Usage
///
/// 1. Create an instance using `OnceEventEmbedded::new()` or `Default::default()`.
/// 2. Pin the storage (required for safe operation).
/// 3. Call `activate()` to get a sender/receiver pair.
/// 4. Use the sender to set a value and the receiver to await it.
/// 5. Once both sender and receiver are dropped, the storage becomes inert and can be reused
///    with a new `activate()` call.
///
/// # Example
///
/// ```rust
/// use std::pin::Pin;
///
/// use once_event::OnceEventEmbedded;
///
/// let mut storage = Box::pin(OnceEventEmbedded::<u32>::new());
/// let (sender, receiver) = storage.as_mut().activate();
///
/// sender.set(42);
/// // await receiver to get the value 42
/// ```
#[derive(Debug)]
pub struct OnceEventEmbedded<T> {
    inner: UnsafeCell<WithRefCount<Option<OnceEventCore<T>>>>,
    _must_pin: PhantomPinned,
}

impl<T> OnceEventEmbedded<T> {
    /// Creates a new embedded storage instance.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Activates the storage, creating a sender/receiver pair for a new event.
    ///
    /// # Panics
    ///
    /// Panics if the storage is already active (contains an event that hasn't been fully consumed).
    /// Use `activate_checked()` for a non-panicking version.
    #[must_use]
    pub fn activate(self: Pin<&mut Self>) -> (EmbeddedSender<T>, EmbeddedReceiver<T>) {
        self.activate_checked()
            .expect("OnceEventEmbedded storage is already active")
    }

    /// Activates the storage, creating a sender/receiver pair for a new event.
    ///
    /// Returns `None` if the storage is already active (contains an event that hasn't been fully consumed).
    #[must_use]
    pub fn activate_checked(
        self: Pin<&mut Self>,
    ) -> Option<(EmbeddedSender<T>, EmbeddedReceiver<T>)> {
        // SAFETY: See comments on type.
        let storage = unsafe { &mut *self.inner.get() };

        if storage.is_referenced() {
            return None;
        }

        let mut with_ref_count = WithRefCount::new(Some(OnceEventCore::new()));

        // Sender + Receiver
        with_ref_count.inc_ref();
        with_ref_count.inc_ref();

        *storage = with_ref_count;

        // SAFETY: The caller guarantees that the storage outlives all references by pinning it.
        let event = unsafe { Pin::into_inner_unchecked(self) };
        Some((EmbeddedSender { event }, EmbeddedReceiver { event }))
    }

    /// Returns `true` if no event is currently stored and no references exist.
    pub fn is_inert(&self) -> bool {
        // SAFETY: See comments on type.
        let storage = unsafe { &*self.inner.get() };

        !storage.is_referenced()
    }

    /// Returns the current reference count for debugging purposes.
    pub fn ref_count(&self) -> usize {
        // SAFETY: See comments on type.
        let storage = unsafe { &*self.inner.get() };

        storage.ref_count()
    }
}

impl<T> Default for OnceEventEmbedded<T> {
    fn default() -> Self {
        Self {
            inner: UnsafeCell::new(WithRefCount::new(None)),
            _must_pin: PhantomPinned,
        }
    }
}

// ############## Pool-based Storage ##############

/// Pool-based storage that can contain multiple `OnceEvent` instances accessed via shared references.
///
/// This storage type allows multiple independent sender/receiver pairs to be created
/// simultaneously, with each pair managing its own event lifecycle.
///
/// # Example
///
/// ```rust
/// use once_event::OnceEventPoolByRef;
///
/// let storage = OnceEventPoolByRef::<u32>::new();
/// let (sender1, receiver1) = storage.activate();
/// let (sender2, receiver2) = storage.activate();
///
/// sender1.set(42);
/// sender2.set(100);
/// // await receiver1 to get 42, receiver2 to get 100
/// ```
#[derive(Debug)]
pub struct OnceEventPoolByRef<T> {
    storage: PinnedRcStorage<OnceEventCore<T>>,
}

impl<T> OnceEventPoolByRef<T> {
    /// Creates a new pool-based storage instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            storage: PinnedRcBox::new_storage_ref(),
        }
    }

    /// Activates a new event in the pool, creating a sender/receiver pair.
    pub fn activate(&self) -> (RefSender<'_, T>, RefReceiver<'_, T>) {
        let event = PinnedRcBox::new(OnceEventCore::new()).insert_into_ref(&self.storage);

        (
            RefSender {
                event: event.clone(),
            },
            RefReceiver { event },
        )
    }
}

impl<T> Default for OnceEventPoolByRef<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Pool-based storage that can contain multiple `OnceEvent` instances accessed via `Rc` references.
///
/// This storage type allows multiple independent sender/receiver pairs to be created
/// simultaneously, with each pair managing its own event lifecycle. Unlike `OnceEventPoolByRef`,
/// this doesn't require lifetime management but incurs reference counting overhead.
///
/// # Example
///
/// ```rust
/// use std::rc::Rc;
///
/// use once_event::OnceEventPoolByRc;
///
/// let storage = Rc::new(OnceEventPoolByRc::<u32>::new());
/// let (sender1, receiver1) = storage.activate();
/// let (sender2, receiver2) = storage.activate();
///
/// sender1.set(42);
/// sender2.set(100);
/// // await receiver1 to get 42, receiver2 to get 100
/// ```
#[derive(Debug)]
pub struct OnceEventPoolByRc<T> {
    storage: Rc<PinnedRcStorage<OnceEventCore<T>>>,
}

impl<T> OnceEventPoolByRc<T> {
    /// Creates a new pool-based storage instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            storage: Rc::new(PinnedRcBox::new_storage_ref()),
        }
    }

    /// Activates a new event in the pool, creating a sender/receiver pair.
    #[must_use]
    pub fn activate(&self) -> (RcSender<T>, RcReceiver<T>) {
        let event = PinnedRcBox::new(OnceEventCore::new()).insert_into_rc(Rc::clone(&self.storage));

        (
            RcSender {
                event: event.clone(),
            },
            RcReceiver { event },
        )
    }
}

impl<T> Default for OnceEventPoolByRc<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Pool-based storage that can contain multiple `OnceEvent` instances accessed via unsafe raw pointers.
///
/// This storage type allows multiple independent sender/receiver pairs to be created
/// simultaneously, with each pair managing its own event lifecycle. This avoids any reference
/// counting overhead but requires the caller to guarantee safety.
///
/// # Safety
///
/// The storage must remain pinned and alive for the entire lifetime of any sender/receiver
/// pairs created from it.
///
/// # Example
///
/// ```rust
/// use std::pin::Pin;
///
/// use once_event::OnceEventPoolUnsafe;
///
/// let storage = Box::pin(OnceEventPoolUnsafe::<u32>::new());
/// let (sender1, receiver1) = unsafe { storage.as_ref().activate() };
/// let (sender2, receiver2) = unsafe { storage.as_ref().activate() };
///
/// sender1.set(42);
/// sender2.set(100);
/// // await receiver1 to get 42, receiver2 to get 100
/// ```
#[derive(Debug)]
pub struct OnceEventPoolUnsafe<T> {
    storage: PinnedRcStorage<OnceEventCore<T>>,
}

impl<T> OnceEventPoolUnsafe<T> {
    /// Creates a new pool-based storage instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            storage: PinnedRcBox::new_storage_ref(),
        }
    }

    /// Activates a new event in the pool, creating a sender/receiver pair.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the storage outlives both the
    /// sender and receiver.
    ///
    /// The storage must be pinned at all times during the lifetime of the sender/receiver.
    #[must_use]
    pub unsafe fn activate(self: Pin<&Self>) -> (UnsafeSender<T>, UnsafeReceiver<T>) {
        // SAFETY: The caller guarantees that the storage outlives all smart pointers.
        let storage_ref = unsafe { self.map_unchecked(|s| &s.storage) };
        // SAFETY: The caller guarantees that the storage outlives all smart pointers.
        let event =
            unsafe { PinnedRcBox::new(OnceEventCore::new()).insert_into_unsafe(storage_ref) };

        (
            UnsafeSender {
                event: event.clone(),
            },
            UnsafeReceiver { event },
        )
    }
}

impl<T> Default for OnceEventPoolUnsafe<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ############## Sender/Receiver Types ##############

#[derive(Debug)]
/// Sender side of an embedded `OnceEvent`.
pub struct EmbeddedSender<T> {
    // The owner of the event is responsible for ensuring that we reference pinned memory that
    // outlives the event.
    event: *const OnceEventEmbedded<T>,
}

impl<T> EmbeddedSender<T> {
    /// Sets the result of the event, notifying any waiting receiver.
    pub fn set(self, result: T) {
        // SAFETY: We rely on the owner of the event to guarantee that the backing storage remains
        // alive for at least as long as the event itself.
        let storage = unsafe { &*self.event };

        // SAFETY: See comments on storage type alias.
        let storage = unsafe { &mut *storage.inner.get() };

        storage
            .get()
            .as_ref()
            .expect("OnceEvent must still exist because sender exists")
            .set(result);

        // There is no sender anymore, so we can drop a reference.
        storage.dec_ref();
    }
}

#[derive(Debug)]
/// Receiver side of an embedded `OnceEvent`.
pub struct EmbeddedReceiver<T> {
    // The owner of the event is responsible for ensuring that we reference pinned memory that
    // outlives the event.
    event: *const OnceEventEmbedded<T>,
}

impl<T> Future for EmbeddedReceiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        // SAFETY: We rely on the owner of the event to guarantee that the backing storage remains
        // alive for at least as long as the event itself.
        let storage = unsafe { &*self.event };

        // SAFETY: See comments on storage type alias.
        let storage = unsafe { &*storage.inner.get() };

        let result = storage
            .get()
            .as_ref()
            .expect("OnceEvent must still exist because receiver exists")
            .poll(cx.waker());

        result.map_or_else(|| task::Poll::Pending, |result| task::Poll::Ready(result))
    }
}

impl<T> Drop for EmbeddedReceiver<T> {
    fn drop(&mut self) {
        // SAFETY: We rely on the owner of the event to guarantee that the backing storage remains
        // alive for at least as long as the event itself.
        let storage = unsafe { &*self.event };

        // SAFETY: See comments on storage type alias.
        let storage = unsafe { &mut *storage.inner.get() };

        // There is no receiver anymore, so we can drop a reference.
        storage.dec_ref();
    }
}

#[derive(Debug)]
/// Sender side of a ref-based `OnceEvent`.
pub struct RefSender<'storage, T> {
    event: RefPinnedRc<'storage, OnceEventCore<T>>,
}

impl<T> RefSender<'_, T> {
    /// Sets the result of the event, notifying any waiting receiver.
    pub fn set(self, result: T) {
        self.event.deref_pin().set(result);
    }
}

#[derive(Debug)]
/// Receiver side of a ref-based `OnceEvent`.
pub struct RefReceiver<'storage, T> {
    event: RefPinnedRc<'storage, OnceEventCore<T>>,
}

impl<T> Future for RefReceiver<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let result = self.event.deref_pin().poll(cx.waker());

        result.map_or_else(|| task::Poll::Pending, |result| task::Poll::Ready(result))
    }
}

#[derive(Debug)]
/// Sender side of an RC-based `OnceEvent`.
pub struct RcSender<T> {
    event: RcPinnedRc<OnceEventCore<T>>,
}

impl<T> RcSender<T> {
    /// Sets the result of the event, notifying any waiting receiver.
    pub fn set(self, result: T) {
        self.event.deref_pin().set(result);
    }
}

#[derive(Debug)]
/// Receiver side of an RC-based `OnceEvent`.
pub struct RcReceiver<T> {
    event: RcPinnedRc<OnceEventCore<T>>,
}

impl<T> Future for RcReceiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let result = self.event.deref_pin().poll(cx.waker());

        result.map_or_else(|| task::Poll::Pending, |result| task::Poll::Ready(result))
    }
}

#[derive(Debug)]
/// Sender side of an unsafe pointer-based `OnceEvent`.
pub struct UnsafeSender<T> {
    event: UnsafePinnedRc<OnceEventCore<T>>,
}

impl<T> UnsafeSender<T> {
    /// Sets the result of the event, notifying any waiting receiver.
    pub fn set(self, result: T) {
        self.event.deref_pin().set(result);
    }
}

#[derive(Debug)]
/// Receiver side of an unsafe pointer-based `OnceEvent`.
pub struct UnsafeReceiver<T> {
    event: UnsafePinnedRc<OnceEventCore<T>>,
}

impl<T> Future for UnsafeReceiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let result = self.event.deref_pin().poll(cx.waker());

        result.map_or_else(|| task::Poll::Pending, |result| task::Poll::Ready(result))
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use futures::task::noop_waker_ref;

    use super::*;

    #[test]
    fn embedded_get_after_set() {
        let mut storage = Box::pin(OnceEventEmbedded::<u32>::new());
        let (sender, mut receiver) = storage.as_mut().activate();

        sender.set(42);

        let cx = &mut task::Context::from_waker(noop_waker_ref());
        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Ready(42));
    }

    #[test]
    fn embedded_get_before_set() {
        let mut storage = Box::pin(OnceEventEmbedded::<u32>::new());
        let (sender, mut receiver) = storage.as_mut().activate();

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Pending);

        sender.set(42);

        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Ready(42));
    }

    #[test]
    fn embedded_reuse_after_completion() {
        let mut storage = Box::pin(OnceEventEmbedded::<u32>::new());

        // First activation
        let (sender1, mut receiver1) = storage.as_mut().activate();
        sender1.set(42);

        let cx = &mut task::Context::from_waker(noop_waker_ref());
        let result = receiver1.poll_unpin(cx);
        assert_eq!(result, task::Poll::Ready(42));

        // Drop receiver to complete the cycle
        drop(receiver1);

        // Should be able to activate again
        let (sender2, mut receiver2) = storage.as_mut().activate();
        sender2.set(100);

        let result = receiver2.poll_unpin(cx);
        assert_eq!(result, task::Poll::Ready(100));
    }

    #[test]
    #[should_panic(expected = "OnceEventEmbedded storage is already active")]
    fn embedded_activate_while_active_panics() {
        let mut storage = Box::pin(OnceEventEmbedded::<u32>::new());
        let (_sender1, _receiver1) = storage.as_mut().activate();
        let (_sender2, _receiver2) = storage.as_mut().activate(); // Should panic
    }

    #[test]
    fn embedded_activate_checked_while_active_returns_none() {
        let mut storage = Box::pin(OnceEventEmbedded::<u32>::new());
        let (_sender1, _receiver1) = storage.as_mut().activate();
        let result = storage.as_mut().activate_checked();
        assert!(result.is_none());
    }

    #[test]
    fn pool_by_ref_multiple_events() {
        let storage = OnceEventPoolByRef::<u32>::new();
        let (sender1, mut receiver1) = storage.activate();
        let (sender2, mut receiver2) = storage.activate();

        sender1.set(42);
        sender2.set(100);

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result1 = receiver1.poll_unpin(cx);
        assert_eq!(result1, task::Poll::Ready(42));

        let result2 = receiver2.poll_unpin(cx);
        assert_eq!(result2, task::Poll::Ready(100));
    }

    #[test]
    fn pool_by_rc_multiple_events() {
        let storage = OnceEventPoolByRc::<u32>::new();
        let (sender1, mut receiver1) = storage.activate();
        let (sender2, mut receiver2) = storage.activate();

        sender1.set(42);
        sender2.set(100);

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result1 = receiver1.poll_unpin(cx);
        assert_eq!(result1, task::Poll::Ready(42));

        let result2 = receiver2.poll_unpin(cx);
        assert_eq!(result2, task::Poll::Ready(100));
    }

    #[test]
    fn pool_unsafe_multiple_events() {
        let storage = Box::pin(OnceEventPoolUnsafe::<u32>::new());
        // SAFETY: The storage outlives the sender and receiver in this test.
        let (sender1, mut receiver1) = unsafe { storage.as_ref().activate() };
        // SAFETY: The storage outlives the sender and receiver in this test.
        let (sender2, mut receiver2) = unsafe { storage.as_ref().activate() };

        sender1.set(42);
        sender2.set(100);

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result1 = receiver1.poll_unpin(cx);
        assert_eq!(result1, task::Poll::Ready(42));

        let result2 = receiver2.poll_unpin(cx);
        assert_eq!(result2, task::Poll::Ready(100));
    }
}
