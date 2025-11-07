use std::any::{Any, TypeId};
use std::backtrace::Backtrace;
use std::fmt;
use std::sync::Arc;

use hash_hasher::HashedMap;
use parking_lot::Mutex;

use crate::{EventPool, PooledReceiver, PooledSender};

/// An event pool for events of different payloads.
///
/// You can use this if you need to constantly create events with different/unknown payload types.
/// Functionally, it is similar to [`EventPool`] but does not require any generic type parameters.
#[derive(Clone, Debug)]
pub struct EventLake {
    core: Arc<Core>,
}

struct Core {
    // This is a transparent HashMap, meaning it does not do any hashing.
    // The reason is that the TypeId is already a hash, so hashing it again is redundant.
    pools: Mutex<HashedMap<TypeId, Box<dyn ErasedPool>>>,
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Core").field("pools", &self.pools).finish()
    }
}

impl EventLake {
    /// Creates a new empty event lake.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Arc::new(Core {
                pools: Mutex::new(HashedMap::default()),
            }),
        }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub fn rent<T: Send + 'static>(&self) -> (PooledSender<T>, PooledReceiver<T>) {
        let type_id = TypeId::of::<T>();

        let mut pools = self.core.pools.lock();

        let entry = pools
            .entry(type_id)
            .or_insert_with(|| Box::new(PoolWrapper::<T>::new()));

        let pool = entry
            .as_any()
            .downcast_ref::<PoolWrapper<T>>()
            .expect("guarded by TypeId");

        pool.rent()
    }

    /// Uses the provided closure to inspect the backtraces of the most recent awaiter of each
    /// awaited event in the lake.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the lake that has been awaited at some point
    /// in the past.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(&Backtrace)) {
        let pools = self.core.pools.lock();

        for entry in pools.values() {
            entry.inspect_awaiters(&mut f);
        }
    }
}

impl Default for EventLake {
    fn default() -> Self {
        Self::new()
    }
}

struct PoolWrapper<T: Send + 'static> {
    inner: EventPool<T>,
}

impl<T: Send + 'static> PoolWrapper<T> {
    fn new() -> Self {
        Self {
            inner: EventPool::new(),
        }
    }

    fn rent(&self) -> (PooledSender<T>, PooledReceiver<T>) {
        self.inner.rent()
    }
}

impl<T: Send + 'static> fmt::Debug for PoolWrapper<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolWrapper")
            .field("inner", &self.inner)
            .finish()
    }
}

/// A type-erased event pool for which we do not know the payload type any more.
///
/// We downcast from this to a specific pool wrapper when we need to rent events.
trait ErasedPool: fmt::Debug + Send {
    fn as_any(&self) -> &dyn Any;
    fn inspect_awaiters(&self, f: &mut dyn FnMut(&Backtrace));
}

impl<T: Send + 'static> ErasedPool for PoolWrapper<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn inspect_awaiters(&self, f: &mut dyn FnMut(&Backtrace)) {
        self.inner.inspect_awaiters(|bt| f(bt));
    }
}
