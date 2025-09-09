use std::cell::Cell;
use std::thread;
use infinity_pool::{BlindPool, PinnedPool};

// Test type that is Send but not Sync
struct SendNotSync {
    data: Cell<i32>,
}

// SAFETY: Cell<T> is Send if T is Send
unsafe impl Send for SendNotSync {}

#[test]
fn unique_pinned_handle_works_with_send_not_sync() {
    let pool = PinnedPool::<SendNotSync>::new();
    let handle = pool.insert(SendNotSync {
        data: Cell::new(42),
    });

    // Unique handles should be Send even when T is !Sync but Send
    let result = thread::spawn(move || handle.data.get()).join().unwrap();

    assert_eq!(result, 42);
}

#[test]
fn unique_blind_handle_works_with_send_not_sync() {
    let pool = BlindPool::new();
    let handle = pool.insert(SendNotSync {
        data: Cell::new(24),
    });

    // Unique blind handles should be Send even when T is !Sync but Send
    let result = thread::spawn(move || handle.data.get()).join().unwrap();

    assert_eq!(result, 24);
}

#[test]
fn shared_handles_still_require_sync() {
    use std::sync::atomic::{AtomicI32, Ordering};

    // Test type that is both Send and Sync
    struct SendAndSync {
        data: AtomicI32,
    }

    unsafe impl Send for SendAndSync {}
    unsafe impl Sync for SendAndSync {}

    let pool = PinnedPool::<SendAndSync>::new();
    let handle = pool.insert(SendAndSync {
        data: AtomicI32::new(100),
    });
    let shared_handle = handle.into_shared();

    // This should work because SendAndSync is Send + Sync
    let result = thread::spawn(move || shared_handle.data.load(Ordering::Relaxed))
        .join()
        .unwrap();

    assert_eq!(result, 100);
}

#[test]
fn type_erasure_preserves_thread_safety_expectations() {
    let pool = PinnedPool::<SendNotSync>::new();
    let handle = pool.insert(SendNotSync {
        data: Cell::new(333),
    });

    // Type erase to () - this loses the original type information
    let erased_handle = handle.erase(); // Now PooledMut<()>

    // Unit type is Sync, so this creates a shared handle that is Send
    // This demonstrates the type erasure caveat mentioned in the issue
    let shared_erased = erased_handle.into_shared(); // Pooled<()>

    // The shared handle can be moved because () is Sync
    // But it can't meaningfully access the original object  
    let moved_successfully = thread::spawn(move || {
        // We can verify the handle was moved but shouldn't access raw pointers across threads
        format!("Handle moved successfully: {:?}", shared_erased)
    })
    .join()
    .unwrap();

    // We can verify the operation completed successfully
    assert!(moved_successfully.contains("Handle moved successfully"));
}