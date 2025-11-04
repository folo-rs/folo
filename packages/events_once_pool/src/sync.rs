#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};

use events_once_core::{Disconnected, Event, PtrRef};
use infinity_pool::{RawPinnedPool, RawPooled, RawPooledMut};
use parking_lot::Mutex;

/// A pool of reusable one-time thread-safe events.
pub struct EventPool<T: Send> {
    core: Arc<Core<T>>,

    _owns_some: PhantomData<T>,
}

impl<T: Send> fmt::Debug for EventPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventPool")
            .field("core", &self.core)
            .finish()
    }
}

struct Core<T: Send> {
    pool: Mutex<RawPinnedPool<UnsafeCell<MaybeUninit<Event<T>>>>>,
}

impl<T: Send> EventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Arc::new(Core {
                pool: Mutex::new(RawPinnedPool::new()),
            }),
            _owns_some: PhantomData,
        }
    }

    /// Rents an event from the pool, returning its endpoints.
    ///
    /// The event will be returned to the pool when both endpoints are dropped.
    #[must_use]
    pub fn rent(&self) -> (Sender<T>, Receiver<T>) {
        let mut pool = self.core.pool.lock();

        // SAFETY: We are required to initialize the storage of the item we store in the pool.
        // Funny thing is, the storage is actually going to start off completely uninitialized!
        let mut storage = unsafe { pool.insert_with(|_| {}) };

        // SAFETY: We must guarantee that the inner pool remains alive as long as the storage
        // is used. We do that - all our senders/receivers hold an Arc to the core, which
        // contains the pool.
        let storage_as_pin_mut = unsafe { storage.as_pin_mut() };

        // SAFETY: We are required to keep the storage valid for writes for as long as the
        // endpoints exist. We never reference it again after this, only for dropping it after
        // all references are dropped, so we fulfill this promise. We also just added this item
        // into the pool, so we know it is not already in use by anything else.
        let (sender, receiver) = unsafe { Event::<T>::placed(storage_as_pin_mut) };

        todo!()
    }

    /// Uses the provided closure to inspect the backtraces of the most recent awaiter of each
    /// awaited event in the pool.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the pool that has been awaited at some point
    /// in the past.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(&Backtrace)) {
        todo!()
    }
}

impl<T: Send> Default for EventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send> Clone for EventPool<T> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            _owns_some: PhantomData,
        }
    }
}

impl<T: Send> fmt::Debug for Core<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Core").field("pool", &self.pool).finish()
    }
}

/// Delivers a single value to the receiver connected to the same event.
pub struct Sender<T: Send> {
    core: Arc<Core<T>>,
    inner: events_once_core::Sender<PtrRef<T>>,
    storage: RawPooled<UnsafeCell<MaybeUninit<Event<T>>>>,
}

impl<T: Send> Sender<T> {
    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    pub fn send(self, value: T) {
        todo!()
    }
}

impl<T: Send> Drop for Sender<T> {
    fn drop(&mut self) {
        todo!()
    }
}

impl<T: Send> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender")
            .field("core", &self.core)
            .field("inner", &self.inner)
            .field("storage", &self.storage)
            .finish()
    }
}

/// Receives a single value from the sender connected to the same event.
pub struct Receiver<T: Send> {
    core: Arc<Core<T>>,
    inner: events_once_core::Receiver<PtrRef<T>>,
    storage: RawPooled<UnsafeCell<MaybeUninit<Event<T>>>>,
}

impl<T: Send> Receiver<T> {
    /// Checks whether a value is ready to be received.
    ///
    /// Both a real value and a "disconnected" signal count,
    /// as they are just different kinds of values.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        todo!()
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
    pub fn into_value(self) -> Result<Result<T, Disconnected>, Self> {
        todo!()
    }
}

impl<T: Send> Future for Receiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

impl<T: Send> Drop for Receiver<T> {
    fn drop(&mut self) {
        todo!()
    }
}

impl<T: Send> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receiver")
            .field("core", &self.core)
            .field("inner", &self.inner)
            .field("storage", &self.storage)
            .finish()
    }
}
