// This test verifies that erasing a shared handle to a !Sync type fails to compile
// for thread-safe handles, as () is Sync but the original type is not.

use infinity_pool::{OpaquePool, Pooled};
use std::cell::Cell;

// Type that is Send but not Sync
struct SendNotSync {
    data: Cell<i32>,
}

unsafe impl Send for SendNotSync {}

fn main() {
    let pool = OpaquePool::with_layout_of::<SendNotSync>();
    let unique_handle = pool.insert(SendNotSync { data: Cell::new(42) });
    let shared_handle: Pooled<SendNotSync> = unique_handle.into_shared();
    
    // This should NOT compile - SendNotSync is !Sync, so we cannot erase it
    // to Pooled<()> which would be Send (requires T: Sync for shared handles).
    let _erased = shared_handle.erase();
}
