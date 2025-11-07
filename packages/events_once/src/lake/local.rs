use std::any::{Any, TypeId};
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use hash_hasher::HashedMap;

use crate::{LocalEventPool, PooledLocalReceiver, PooledLocalSender};

/// An event pool for single-threaded events of different payloads.
///
/// You can use this if you need to constantly create single-threaded events with different/unknown
/// payload types. Functionally, it is similar to [`LocalEventPool`] but does not require any
/// generic type parameters.
#[derive(Clone, Debug)]
pub struct LocalEventLake {
    core: Rc<Core>,
}

struct Core {
    // This is a transparent HashMap, meaning it does not do any hashing.
    // The reason is that the TypeId is already a hash, so hashing it again is redundant.
    pools: RefCell<HashedMap<TypeId, Box<dyn ErasedPool>>>,
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Core").field("pools", &self.pools).finish()
    }
}

impl LocalEventLake {
    /// Creates a new empty event lake.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Rc::new(Core {
                pools: RefCell::new(HashedMap::default()),
            }),
        }
    }

    /// Rents an event from the lake, returning its endpoints.
    ///
    /// The event will be returned to the lake when both endpoints are dropped.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub fn rent<T: 'static>(&self) -> (PooledLocalSender<T>, PooledLocalReceiver<T>) {
        let type_id = TypeId::of::<T>();

        let mut pools = self.core.pools.borrow_mut();

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
        let pools = self.core.pools.borrow();

        for entry in pools.values() {
            entry.inspect_awaiters(&mut f);
        }
    }
}

impl Default for LocalEventLake {
    fn default() -> Self {
        Self::new()
    }
}

struct PoolWrapper<T: 'static> {
    inner: LocalEventPool<T>,
}

impl<T: 'static> PoolWrapper<T> {
    fn new() -> Self {
        Self {
            inner: LocalEventPool::new(),
        }
    }

    fn rent(&self) -> (PooledLocalSender<T>, PooledLocalReceiver<T>) {
        self.inner.rent()
    }
}

impl<T: 'static> fmt::Debug for PoolWrapper<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolWrapper")
            .field("inner", &self.inner)
            .finish()
    }
}

/// A type-erased event pool for which we do not know the payload type any more.
///
/// We downcast from this to a specific pool wrapper when we need to rent events.
trait ErasedPool: fmt::Debug {
    fn as_any(&self) -> &dyn Any;
    fn inspect_awaiters(&self, f: &mut dyn FnMut(&Backtrace));
}

impl<T: 'static> ErasedPool for PoolWrapper<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn inspect_awaiters(&self, f: &mut dyn FnMut(&Backtrace)) {
        self.inner.inspect_awaiters(|bt| f(bt));
    }
}
