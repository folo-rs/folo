// This test should fail to compile because we're trying to send a shared handle
// from a type that is Send but not Sync across thread boundaries.
use std::cell::Cell;
use std::thread;
use infinity_pool::PinnedPool;

// Test type that is Send but not Sync.
struct SendNotSync {
    data: Cell<i32>,
}

// SAFETY: Cell<T> is Send if T is Send.
unsafe impl Send for SendNotSync {}
// Note: Deliberately NOT implementing Sync - Cell<T> is !Sync

fn main() {
    let pool = PinnedPool::<SendNotSync>::new();
    let handle = pool.insert(SendNotSync {
        data: Cell::new(42),
    });
    
    // Convert to shared handle - this part works because into_shared() doesn't require Send
    let shared_handle = handle.into_shared();
    
    // This should fail to compile because SendNotSync is !Sync, so Pooled<SendNotSync> is !Send
    // and therefore cannot be moved into the thread closure
    let _result = thread::spawn(move || {
        format!("Handle moved successfully: {shared_handle:?}")
    })
    .join()
    .unwrap();
}