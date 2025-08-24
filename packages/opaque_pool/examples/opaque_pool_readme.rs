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

    // The handles start as exclusive (PooledMut), convert to shared for copying.
    let pooled1_shared = pooled1.into_shared();
    let pooled1_copy = pooled1_shared;

    // Read data back from the pooled items using Deref.
    let value1 = *pooled1_shared;
    let value1_copy = *pooled1_copy;

    assert_eq!(value1, value1_copy);

    println!("Original value: {value1}");
    println!("Copy value: {value1_copy}");
    assert_eq!(value1, 42);

    println!("README example completed successfully!");
}
