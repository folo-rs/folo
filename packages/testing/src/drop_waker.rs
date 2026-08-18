use std::marker::PhantomData;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable, Waker};

/// Creates a [`Waker`] that owns one reference to `data`, whose `wake()` and destructor both
/// merely release that reference.
///
/// The reentrant behavior under test therefore belongs in the [`Drop`] implementation of `T`,
/// which runs once the last reference goes away - typically when the primitive under test wakes
/// the waker it stored, or discards that waker during cancellation.
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
    // SAFETY: The vtable upholds the `RawWaker` contract for an `Arc<T>` payload - it is only
    // ever paired with a pointer from `Arc::into_raw`, and each operation is an atomic reference
    // count operation, which satisfies the thread-safety requirement on `RawWakerVTable`. The
    // caller upholds the remaining requirement, that `T` is only dropped where it may be dropped.
    unsafe { Waker::from_raw(raw_waker(data)) }
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
    /// `data` must be a pointer obtained from `Arc::<T>::into_raw()` whose reference is still
    /// owned by the waker being cloned.
    unsafe fn clone_raw(data: *const ()) -> RawWaker {
        // SAFETY: Forwarding the guarantees of the caller.
        let arc = unsafe { Arc::from_raw(data.cast::<T>()) };

        let clone = Arc::clone(&arc);

        // The reference we reconstructed still belongs to the waker we were cloned from, so we
        // must not release it here.
        std::mem::forget(arc);

        raw_waker(clone)
    }

    /// # Safety
    ///
    /// `data` must be a pointer obtained from `Arc::<T>::into_raw()` whose reference is being
    /// consumed by this call.
    unsafe fn wake_raw(data: *const ()) {
        // SAFETY: Forwarding the guarantees of the caller.
        unsafe { Self::drop_raw(data) }
    }

    fn wake_by_ref_raw(_data: *const ()) {}

    /// # Safety
    ///
    /// `data` must be a pointer obtained from `Arc::<T>::into_raw()` whose reference is being
    /// consumed by this call.
    unsafe fn drop_raw(data: *const ()) {
        // SAFETY: Forwarding the guarantees of the caller.
        drop(unsafe { Arc::from_raw(data.cast::<T>()) });
    }
}
