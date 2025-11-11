// This test verifies that erasing a handle to a !Send type fails to compile
// for thread-safe handles, as () is Send but the original type is not.

use infinity_pool::{OpaquePool, PooledMut};
use std::rc::Rc;

fn main() {
    let pool = OpaquePool::with_layout_of::<Rc<i32>>();
    let handle: PooledMut<Rc<i32>> = pool.insert(Rc::new(42));
    
    // This should NOT compile - Rc<i32> is !Send, so we cannot erase it
    // to PooledMut<()> which would be Send.
    let _erased = handle.erase();
}
