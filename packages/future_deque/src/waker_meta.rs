use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{RawWaker, RawWakerVTable, Waker};

use infinity_pool::{PinnedPool, Pooled, PooledMut};

// Per-slot metadata for activation tracking and waker management.
//
// Stored in a thread-local pinned pool with stable addresses. The RawWaker data pointer
// points directly into the pool slab, avoiding per-future heap allocations after warm-up.
//
// Each WakerMeta is reference-counted: one reference for the owning Slot, plus one per
// outstanding waker clone. When the refcount reaches zero, the metadata is removed from
// the pool and its slot is available for reuse.
pub(crate) struct WakerMeta {
    ref_count: AtomicUsize,

    // Per-slot activation flag. Set by the waker when the future is woken,
    // cleared by poll() when the future is polled. Represented as an
    // AtomicUsize with strict 0/1 semantics: 0 = not activated, 1 = activated.
    pub(crate) activated: AtomicUsize,

    // Shared parent waker, one per FutureDequeCore instance. All slots in the same deque
    // share this Arc, ensuring that parent waker changes propagate automatically without
    // per-slot iteration. Initialized to Waker::noop() and updated in poll() when the
    // executor provides a real waker.
    shared_parent: Arc<Mutex<Waker>>,

    // Self-referential pool handle for cleanup when refcount reaches zero. Dropping the
    // Pooled handle auto-removes the entry from the pool. Set to Some immediately after
    // pool insertion; None only during the brief construction window and after cleanup.
    // Uses UnsafeCell because we need interior mutability during bootstrapping (writing
    // the handle back through a shared reference obtained from the pool).
    self_handle: UnsafeCell<Option<Pooled<Self>>>,
}

// Thread-local pool for waker metadata. PinnedPool provides stable (pinned) addresses
// for the RawWaker data pointer via slab-based allocation, and is internally
// reference-counted (Arc) and synchronized (Mutex), so cross-thread waker drops can
// safely access the pool even after the creating thread has terminated.
thread_local! {
    static WAKER_META_POOL: PinnedPool<WakerMeta> = PinnedPool::new();
}

static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    clone_raw_waker,
    wake_raw_waker,
    wake_by_ref_raw_waker,
    drop_raw_waker,
);

/// All fields of `WakerMeta` are thread-safe (atomics, Arc, Mutex), and the pool
/// is internally synchronized, so this pointer is safe to send across threads.
#[derive(Clone, Copy)]
pub(crate) struct MetaPtr(*const WakerMeta);

// SAFETY: WakerMeta fields are all thread-safe (AtomicUsize, Arc, std::sync::Mutex).
// The pool slab provides a stable (pinned) address, and the pool itself is internally
// reference-counted and synchronized. The UnsafeCell<Option<Pooled<WakerMeta>>> is
// only written during bootstrapping and read when refcount reaches zero (no concurrent
// access possible).
unsafe impl Send for MetaPtr {}

// SAFETY: Same justification as Send above. All WakerMeta fields use thread-safe
// types, and access to the UnsafeCell is serialized by the refcount lifecycle.
unsafe impl Sync for MetaPtr {}

/// Creates a new [`WakerMeta`] in the thread-local pool and returns a [`MetaPtr`] to it.
///
/// The returned pointer is stable (pinned in pool slab) and valid until the metadata
/// is removed from the pool (when its refcount reaches zero).
pub(crate) fn create_waker_meta(shared_parent: &Arc<Mutex<Waker>>) -> MetaPtr {
    WAKER_META_POOL.with(|pool| {
        let handle: PooledMut<WakerMeta> = pool.insert(WakerMeta {
            ref_count: AtomicUsize::new(1),
            activated: AtomicUsize::new(1),
            shared_parent: Arc::clone(shared_parent),
            self_handle: UnsafeCell::new(None),
        });

        // Get stable pointer before consuming the mutable handle.
        let meta_ptr: *const WakerMeta = handle.ptr().as_ptr();

        // Convert to shared (Clone) handle for self-cleanup.
        let shared: Pooled<WakerMeta> = handle.into_shared();

        // Write back the self-handle through UnsafeCell. We have exclusive access:
        // the entry was just inserted and no other code has a reference to it.
        //
        // SAFETY: Exclusive access guaranteed by fresh insertion — no other code
        // path can reach this entry until we return MetaPtr.
        let cell_ptr = unsafe { core::ptr::addr_of!((*meta_ptr).self_handle) };

        // UnsafeCell is #[repr(transparent)] over T, so we cast to access
        // the inner value directly.
        let inner_ptr = cell_ptr as *mut Option<Pooled<WakerMeta>>;

        // SAFETY: Exclusive access guaranteed by fresh insertion. The
        // UnsafeCell provides interior mutability for this bootstrapping write.
        unsafe {
            *inner_ptr = Some(shared);
        }

        MetaPtr(meta_ptr)
    })
}

/// Creates a [`Waker`] from a metadata pointer, incrementing the refcount.
pub(crate) fn make_waker(meta: MetaPtr) -> Waker {
    // SAFETY: The metadata is valid (refcount > 0 guarantees it has not been removed).
    let meta_ref = unsafe { &*meta.0 };
    meta_ref.ref_count.fetch_add(1, Ordering::Relaxed);

    // SAFETY: The vtable functions correctly match the data pointer layout.
    unsafe { Waker::from_raw(RawWaker::new(meta.0 as *const (), &WAKER_VTABLE)) }
}

/// Reads the activation flag, atomically clearing it. Returns `true` if the slot was
/// activated since the last call.
pub(crate) fn check_activated(meta: MetaPtr) -> bool {
    // SAFETY: The metadata is valid (refcount > 0 guarantees it has not been removed).
    let meta_ref = unsafe { &*meta.0 };
    meta_ref.activated.swap(0, Ordering::AcqRel) != 0
}

/// Decrements the refcount and removes the metadata from the pool if this was the
/// last reference. Called when a Slot releases its reference (future completes or
/// deque is dropped) and when the last waker clone is dropped.
// Detecting this mutation requires observing that pool entries are not returned,
// which is an internal pool detail invisible to tests without pool introspection.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn release_ref(meta: MetaPtr) {
    // SAFETY: The metadata is valid (refcount > 0 guarantees it has not been removed).
    let previous = unsafe { &*meta.0 }.ref_count.fetch_sub(1, Ordering::AcqRel);

    if previous == 1 {
        // Last reference — extract the self-handle. Dropping it auto-removes the
        // entry from the pool. We must not hold any reference to the WakerMeta
        // across the drop because the pool slot is freed.
        //
        // We use addr_of! to project a raw pointer to the self_handle field
        // without creating an intermediate &WakerMeta reference.
        //
        // SAFETY: No concurrent access is possible (refcount was 1, now 0).
        let cell_ptr = unsafe { core::ptr::addr_of!((*meta.0).self_handle) };

        // UnsafeCell is #[repr(transparent)] over T, so *const UnsafeCell<T>
        // has the same layout as *mut T. We cast to access the inner value.
        let inner_ptr = cell_ptr as *mut Option<Pooled<WakerMeta>>;

        // SAFETY: The UnsafeCell was written during bootstrapping. No concurrent
        // access is possible because the refcount reached zero.
        let self_handle = unsafe { (*inner_ptr).take() }
            .expect("self_handle is always set immediately after pool insertion");

        drop(self_handle);
    }
}

// --- RawWaker vtable functions ---

unsafe fn clone_raw_waker(data: *const ()) -> RawWaker {
    // SAFETY: The data pointer is a valid WakerMeta pointer (guaranteed by
    // construction in make_waker and create_waker_meta).
    let meta = unsafe { &*(data as *const WakerMeta) };
    meta.ref_count.fetch_add(1, Ordering::Relaxed);
    RawWaker::new(data, &WAKER_VTABLE)
}

unsafe fn wake_raw_waker(data: *const ()) {
    // Owned wake: activate, wake parent, then release this reference.
    // SAFETY: Delegating to vtable function with the same valid pointer.
    unsafe {
        wake_by_ref_raw_waker(data);
    }

    // SAFETY: Delegating to vtable function with the same valid pointer.
    unsafe {
        drop_raw_waker(data);
    }
}

unsafe fn wake_by_ref_raw_waker(data: *const ()) {
    // SAFETY: The data pointer is a valid WakerMeta pointer.
    let meta = unsafe { &*(data as *const WakerMeta) };

    // Only wake the parent if we are the first to set the activation flag.
    // If it was already set, the parent was already woken by a prior activation.
    if meta.activated.swap(1, Ordering::AcqRel) == 0 {
        // Clone the parent waker under the lock, then drop the lock before waking
        // to avoid potential deadlock if the wake path re-enters and tries to lock
        // shared_parent again (e.g. some executor waker implementations).
        let parent = meta
            .shared_parent
            .lock()
            .expect("we never panic while holding this lock")
            .clone();

        parent.wake_by_ref();
    }
}

unsafe fn drop_raw_waker(data: *const ()) {
    release_ref(MetaPtr(data as *const WakerMeta));
}
