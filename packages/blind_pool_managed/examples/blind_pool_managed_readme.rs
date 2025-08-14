//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `ManagedBlindPool` for thread-safe memory pool management
//! with automatic cleanup.

use blind_pool::BlindPool;
use blind_pool_managed::ManagedBlindPool;

fn main() {
    // Create a managed pool from a regular BlindPool.
    let pool = ManagedBlindPool::from(BlindPool::new());

    // The pool can be cloned and shared across threads.
    let pool_clone = pool.clone();

    // Insert values and get managed handles.
    let managed_u32 = pool.insert(42_u32);
    let managed_string = pool.insert("hello".to_string());

    // Access values through dereferencing.
    println!("u32 value: {}", *managed_u32);
    println!("string value: {}", *managed_string);

    // Get raw pointers for unsafe access.
    let ptr = managed_u32.ptr();
    // SAFETY: The pointer is valid and contains the value we just inserted.
    let value = unsafe { ptr.read() };
    println!("Value read via pointer: {value}");

    // Clone handles - the value remains alive as long as any handle exists.
    let cloned_handle = managed_u32.clone();
    drop(managed_u32);
    println!("Value still accessible: {}", *cloned_handle);

    // Use different types in the same pool.
    let managed_float = pool_clone.insert(2.5_f64);
    println!("Float value: {}", *managed_float);

    // Check pool statistics.
    println!("Pool length: {}", pool.len());
    println!("Pool is empty: {}", pool.is_empty());

    // Values are automatically removed when all handles are dropped.
    drop(managed_string);
    drop(cloned_handle);
    drop(managed_float);

    println!("Pool length after cleanup: {}", pool.len());
    println!("Pool is empty after cleanup: {}", pool.is_empty());
}
