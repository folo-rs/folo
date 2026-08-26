use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::{fmt, mem};

/// Creates a [`Waker`] that owns one reference to `data`, whose `wake()` and destructor both
/// merely release that reference.
///
/// The reentrant behavior under test therefore belongs in the [`Drop`] implementation of `T`,
/// which runs once the last reference goes away - typically when the primitive under test wakes
/// the waker it stored, or discards that waker during cancellation. [`DropOnWakerRelease`]
/// provides a ready-made payload for that pattern.
///
/// [`Waker::wake_by_ref()`] does not consume a reference, so it does not release the payload and
/// does not reach the behavior under test. A test must therefore observe that the payload really
/// was dropped rather than assume it; the flag from [`DropOnWakerRelease::new()`] serves that
/// purpose, and turns a primitive that only wakes by reference into a failure rather than a
/// vacuous pass.
///
/// Prefer this over [`ReentrantWakerData`][crate::ReentrantWakerData] when the test must run
/// under Miri: this waker owns its data outright, whereas `ReentrantWakerData` hands out a
/// borrowed pointer to data the test still owns.
///
/// # Safety
///
/// Unless `T` is `Send + Sync`, the returned waker and every clone of it must be used and dropped
/// on the calling thread. [`Waker`] is `Send + Sync` regardless of `T`, so this cannot be
/// expressed in the type system.
pub unsafe fn drop_waker<T: 'static>(data: Arc<T>) -> Waker {
    let raw = raw_waker(data);

    // SAFETY: The vtable upholds the `RawWaker` contract for an `Arc<T>` payload - it is only
    // ever paired with a pointer from `Arc::into_raw`, and every operation is an atomic reference
    // count operation, which satisfies the thread-safety requirement on `RawWakerVTable`. The
    // caller upholds the remaining requirement, that `T` is only dropped where it may be dropped.
    unsafe { Waker::from_raw(raw) }
}

fn raw_waker<T: 'static>(data: Arc<T>) -> RawWaker {
    RawWaker::new(Arc::into_raw(data).cast(), &VTable::<T>::VTABLE)
}

/// Carries the [`RawWakerVTable`] for [`drop_waker`], one instance per payload type.
struct VTable<T>(PhantomData<T>);

impl<T: 'static> VTable<T> {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_raw,
        Self::wake_raw,
        Self::wake_by_ref_raw,
        Self::drop_raw,
    );

    /// # Safety
    ///
    /// `data` must come from `Arc::<T>::into_raw()` and its reference must still be owned by the
    /// waker being cloned.
    unsafe fn clone_raw(data: *const ()) -> RawWaker {
        // SAFETY: Forwarding the guarantees of the caller.
        let arc = unsafe { Arc::from_raw(data.cast::<T>()) };

        let clone = Arc::clone(&arc);

        // The reference we reconstructed still belongs to the waker we were cloned from, so we
        // must not release it here.
        mem::forget(arc);

        raw_waker(clone)
    }

    /// # Safety
    ///
    /// `data` must come from `Arc::<T>::into_raw()` and its reference must be consumed by this
    /// call.
    unsafe fn wake_raw(data: *const ()) {
        // SAFETY: Forwarding the guarantees of the caller.
        unsafe { Self::drop_raw(data) }
    }

    /// Waking by reference does not consume a reference, so there is nothing to release.
    fn wake_by_ref_raw(_data: *const ()) {}

    /// # Safety
    ///
    /// `data` must come from `Arc::<T>::into_raw()` and its reference must be consumed by this
    /// call.
    unsafe fn drop_raw(data: *const ()) {
        // SAFETY: Forwarding the guarantees of the caller.
        let arc = unsafe { Arc::from_raw(data.cast::<T>()) };

        drop(arc);
    }
}

/// Owns a value on behalf of a [`drop_waker`] waker, dropping that value once the last waker
/// reference goes away.
///
/// When a primitive under test releases a waker it had stored - while completing, while waking or
/// while cancelling - the value is dropped reentrantly, from inside the operation under test.
/// Parking one of the primitive's own endpoints in here is how callback-safety tests reach back
/// into the primitive at exactly that moment.
///
/// This type is single-threaded, so wakers built from it must stay on one thread.
pub struct DropOnWakerRelease<T> {
    value: RefCell<Option<T>>,
    dropped: Rc<Cell<bool>>,
}

impl<T> DropOnWakerRelease<T> {
    /// Wraps `value`, returning it alongside a flag that records whether it has been dropped.
    ///
    /// The flag lets a test prove that the reentrancy it targets actually occurred, instead of
    /// passing vacuously because the primitive never held on to a waker.
    #[must_use]
    pub fn new(value: T) -> (Arc<Self>, Rc<Cell<bool>>) {
        let dropped = Rc::new(Cell::new(false));

        let data = Arc::new(Self {
            value: RefCell::new(Some(value)),
            dropped: Rc::clone(&dropped),
        });

        (data, dropped)
    }

    /// Calls `f` with the still-owned value.
    ///
    /// # Panics
    ///
    /// Panics if the value has already been dropped.
    pub fn with_value<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut value = self.value.borrow_mut();

        f(value.as_mut().expect("the value has already been dropped"))
    }
}

impl<T> Drop for DropOnWakerRelease<T> {
    fn drop(&mut self) {
        drop(self.value.borrow_mut().take());
        self.dropped.set(true);
    }
}

impl<T> fmt::Debug for DropOnWakerRelease<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("dropped", &self.dropped.get())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use super::*;

    /// Enough concurrent users to interleave reference count operations from several threads,
    /// while staying small enough for Miri to execute the test quickly.
    const THREAD_COUNT: usize = 4;

    #[test]
    fn concurrent_clone_wake_and_drop_releases_payload_exactly_once() {
        struct Payload {
            drop_count: Arc<AtomicUsize>,
        }

        impl Drop for Payload {
            fn drop(&mut self) {
                self.drop_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        let drop_count = Arc::new(AtomicUsize::new(0));
        let payload = Arc::new(Payload {
            drop_count: Arc::clone(&drop_count),
        });

        // SAFETY: The payload is `Send + Sync`, so the waker may travel between threads.
        let waker = unsafe { drop_waker(payload) };

        let barrier = Barrier::new(THREAD_COUNT);

        thread::scope(|scope| {
            for _ in 0..THREAD_COUNT {
                let waker = waker.clone();
                let barrier = &barrier;

                scope.spawn(move || {
                    barrier.wait();

                    let clone = waker.clone();
                    clone.wake_by_ref();
                    clone.wake();

                    drop(waker);
                });
            }
        });

        assert_eq!(
            drop_count.load(Ordering::Relaxed),
            0,
            "the payload must survive while the original waker still holds a reference"
        );

        drop(waker);

        assert_eq!(drop_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn releasing_the_last_waker_drops_the_value() {
        let (data, dropped) = DropOnWakerRelease::new(42_u32);

        // SAFETY: The payload is not `Send`, and this test keeps the waker on one thread.
        let waker = unsafe { drop_waker(Arc::clone(&data)) };

        assert_eq!(data.with_value(|value| *value), 42);

        drop(data);
        assert!(!dropped.get());

        waker.wake();
        assert!(dropped.get());
    }
}
