// This test verifies that shared handles require T: Send + Sync,
// while unique handles only require T: Send.

use infinity_pool::{OpaquePool, Pooled};
use std::cell::Cell;

// Type that is Send but not Sync
struct SendNotSync {
    data: Cell<i32>,
}

unsafe impl Send for SendNotSync {}

fn require_send<T: Send>(_: T) {}

fn main() {
    let pool = OpaquePool::with_layout_of::<SendNotSync>();
    let unique_handle = pool.insert(SendNotSync { data: Cell::new(42) });
    
    // Convert to shared handle
    let shared_handle: Pooled<SendNotSync> = unique_handle.into_shared();
    
    // This should NOT compile - shared handles need T: Send + Sync
    require_send(shared_handle);
}