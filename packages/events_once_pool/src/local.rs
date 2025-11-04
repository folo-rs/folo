#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{self, Poll};

use events_once_core::{Disconnected, LocalEvent, PtrLocalRef};
use infinity_pool::RawPinnedPool;

pub struct LocalEventPool<T> {
    core: Rc<Core<T>>,

    _owns_some: PhantomData<T>,
}

struct Core<T> {
    pool: RefCell<RawPinnedPool<MaybeUninit<LocalEvent<T>>>>,
}

impl<T> LocalEventPool<T> {
    pub fn new() -> Self {
        Self {
            core: Rc::new(Core {
                pool: RefCell::new(RawPinnedPool::new()),
            }),
            _owns_some: PhantomData,
        }
    }

    pub fn rent(&self) -> (LocalSender<T>, LocalReceiver<T>) {
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

pub struct LocalSender<T> {
    core: Rc<Core<T>>,
    inner: PtrLocalRef<T>,
}

impl<T> LocalSender<T> {
    /// Sends a value to the receiver connected to the same event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    pub fn send(self, value: T) {
        todo!()
    }
}

impl<T> Drop for LocalSender<T> {
    fn drop(&mut self) {
        todo!()
    }
}

pub struct LocalReceiver<T> {
    core: Rc<Core<T>>,
    inner: PtrLocalRef<T>,
}

impl<T> LocalReceiver<T> {
    /// Checks whether a value is ready to be received.
    ///
    /// Both a real value and a "disconnected" signal count,
    /// as they are just different kinds of values.
    ///
    /// # Panics
    ///
    /// Panics if called after `poll()` has returned `Ready`.
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

impl<T> Future for LocalReceiver<T> {
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

impl<T> Drop for LocalReceiver<T> {
    fn drop(&mut self) {
        todo!()
    }
}
