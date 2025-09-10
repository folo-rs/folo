// This test verifies that blind shared handles require T: Send + Sync.

use infinity_pool::{BlindPool, BlindPooled};
use std::cell::Cell;

// Type that is Send but not Sync
struct SendNotSync {
    data: Cell<i32>,
}

unsafe impl Send for SendNotSync {}

fn require_send<T: Send>(_: T) {}

fn main() {
    let pool = BlindPool::new();
    let unique_handle = pool.insert(SendNotSync { data: Cell::new(42) });
    
    // Convert to shared handle
    let shared_handle: BlindPooled<SendNotSync> = unique_handle.into_shared();
    
    // This should NOT compile - shared handles need T: Send + Sync
    require_send(shared_handle);
}