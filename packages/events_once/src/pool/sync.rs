use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::Arc;

use infinity_pool::RawPinnedPool;
use parking_lot::Mutex;

use crate::{Event, PooledReceiver, PooledRef, PooledSender, ReceiverCore, SenderCore};

/// A pool of reusable one-time thread-safe events.
pub struct EventPool<T: Send> {
    core: Arc<EventPoolCore<T>>,

    _owns_some: PhantomData<T>,
}

impl<T: Send> fmt::Debug for EventPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .finish()
    }
}

pub(crate) struct EventPoolCore<T: Send> {
    pub(crate) pool: Mutex<RawPinnedPool<UnsafeCell<MaybeUninit<Event<T>>>>>,
}

impl<T: Send> EventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Arc::new(EventPoolCore {
                pool: Mutex::new(RawPinnedPool::new()),
            }),
            _owns_some: PhantomData,
        }
    }

    /// Rents an event from the pool, returning its endpoints.
    ///
    /// The event will be returned to the pool when both endpoints are dropped.
    #[must_use]
    pub fn rent(&self) -> (PooledSender<T>, PooledReceiver<T>) {
        let storage = {
            let mut pool = self.core.pool.lock();

            // SAFETY: We are required to initialize the storage of the item we store in the pool.
            // Funny thing is, the storage is actually going to start off completely uninitialized!
            // That is because it is just a great big UnsafeCell<MaybeUninit> that we will manually
            // initialize later on.
            #[expect(
                clippy::multiple_unsafe_ops_per_block,
                unused_unsafe,
                reason = "it cannot handle the closure"
            )]
            unsafe {
                pool.insert_with(|place| {
                    // This is a sandwich of MaybeUninit<UnsafeCell<MaybeUninit<Event<T>>>>.
                    // The outer MaybeUninit is for the pool to manage uninitialized storage.
                    // It does not know that we are expecting to use the internal MaybeUninit
                    // instead (which we want to do to preserve the UnsafeCell around everything).
                    //
                    // SAFETY: We still treat it as uninitialized due to the inner MaybeUninit.
                    let place = unsafe { place.assume_init_mut() };

                    Event::new_in_inner(place);
                })
            }
        }
        .into_shared();

        let event_ref = PooledRef::new(Arc::clone(&self.core), storage);

        let inner_sender = SenderCore::new(event_ref.clone());
        let inner_receiver = ReceiverCore::new(event_ref);

        (
            PooledSender::new(inner_sender),
            PooledReceiver::new(inner_receiver),
        )
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
        let pool = self.core.pool.lock();

        for event_ptr in pool.iter() {
            // SAFETY: The pool remains alive for the duration of this function call, satisfying
            // the lifetime requirement. The pointer is valid as it comes from the pool's iterator.
            // We only ever create shared references to the events, so no conflicting exclusive
            // references can exist.
            let event_cell = unsafe { event_ptr.as_ref() };

            // SAFETY: See above.
            let event_maybe = unsafe { event_cell.get().as_ref() };

            // SAFETY: UnsafeCell pointer is never null.
            let event = unsafe { event_maybe.unwrap_unchecked() };

            // SAFETY: We only ever create shared references, never exclusive ones.
            let event = unsafe { event.assume_init_ref() };

            event.inspect_awaiter(|bt| {
                if let Some(bt) = bt {
                    f(bt);
                }
            });
        }
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

impl<T: Send> fmt::Debug for EventPoolCore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pool", &self.pool)
            .finish()
    }
}
