//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `OpaquePool` for type-erased object management with copyable handles.

use std::alloc::Layout;

use opaque_pool::OpaquePool;

fn main() {
    println!("=== Opaque Pool README Example ===");

    // Create a pool for storing values that match the layout of `u64`.
    let layout = Layout::new::<u64>();
    let mut pool = OpaquePool::builder().layout(layout).build();

    // Insert values into the pool.
    // SAFETY: The layout of u64 matches the pool's item layout.
    let pooled1 = unsafe { pool.insert(42_u64) };
    // SAFETY: The layout of u64 matches the pool's item layout.
    let _pooled2 = unsafe { pool.insert(123_u64) };

    // The handles act like super-powered pointers - they can be copied freely.
    let pooled1_copy = pooled1;

    // Read data back from the pooled items.
    // SAFETY: The pointer is valid and the value was just inserted.
    let value1 = unsafe { pooled1.ptr().read() };

    // SAFETY: Both handles refer to the same stored value.
    let value1_copy = unsafe { pooled1_copy.ptr().read() };

    assert_eq!(value1, value1_copy);

    println!("Original value: {value1}");
    println!("Copy value: {value1_copy}");
    assert_eq!(value1, 42);

    println!("README example completed successfully!");
}
