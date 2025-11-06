use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use infinity_pool::RawPinnedPool;

use crate::{
    LocalEvent, LocalReceiverCore, LocalSenderCore, PooledLocalReceiver, PooledLocalRef,
    PooledLocalSender,
};

/// A pool of reusable one-time single-threaded events.
pub struct LocalEventPool<T> {
    core: Rc<LocalPoolCore<T>>,

    _owns_some: PhantomData<T>,
}

impl<T> fmt::Debug for LocalEventPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .finish()
    }
}

pub(crate) struct LocalPoolCore<T> {
    pub(crate) pool: RefCell<RawPinnedPool<LocalEvent<T>>>,
}

impl<T> LocalEventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Rc::new(LocalPoolCore {
                pool: RefCell::new(RawPinnedPool::new()),
            }),
            _owns_some: PhantomData,
        }
    }

    /// Rents an event from the pool, returning its endpoints.
    ///
    /// The event will be returned to the pool when both endpoints are dropped.
    #[must_use]
    pub fn rent(&self) -> (PooledLocalSender<T>, PooledLocalReceiver<T>) {
        let storage = {
            let mut pool = self.core.pool.borrow_mut();

            // SAFETY: We are required to initialize the storage of the item we store in the pool.
            // We do - that is what new_in_inner is for.
            unsafe {
                pool.insert_with(|place| {
                    LocalEvent::new_in_inner(place);
                })
            }
        }
        .into_shared();

        let event_ref = PooledLocalRef::new(Rc::clone(&self.core), storage);

        let inner_sender = LocalSenderCore::new(event_ref.clone());
        let inner_receiver = LocalReceiverCore::new(event_ref);

        (
            PooledLocalSender::new(inner_sender),
            PooledLocalReceiver::new(inner_receiver),
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
        let pool = self.core.pool.borrow_mut();

        for event_ptr in pool.iter() {
            // SAFETY: The pool remains alive for the duration of this function call, satisfying
            // the lifetime requirement. The pointer is valid as it comes from the pool's iterator.
            // We only ever create shared references to the events, so no conflicting exclusive
            // references can exist.
            let event = unsafe { event_ptr.as_ref() };

            event.inspect_awaiter(|bt| {
                if let Some(bt) = bt {
                    f(bt);
                }
            });
        }
    }
}

impl<T> Default for LocalEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for LocalEventPool<T> {
    fn clone(&self) -> Self {
        Self {
            core: Rc::clone(&self.core),
            _owns_some: PhantomData,
        }
    }
}

impl<T> fmt::Debug for LocalPoolCore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pool", &self.pool)
            .finish()
    }
}
