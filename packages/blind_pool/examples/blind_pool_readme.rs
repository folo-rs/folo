//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `BlindPool` (thread-safe) and `LocalBlindPool`
//! (single-threaded) for memory pool management with automatic cleanup,
//! as well as a `RawBlindPool` that relies on manual memory management by user code.

use blind_pool::{BlindPool, LocalBlindPool, RawBlindPool};

fn main() {
    println!("=== Thread-safe BlindPool ===");
    thread_safe_example();

    println!("\n=== Single-threaded LocalBlindPool ===");
    single_threaded_example();

    println!("\n=== RawBlindPool ===");
    raw_example();
}

fn thread_safe_example() {
    // Create a pool from a raw BlindPool.
    let pool = BlindPool::from(RawBlindPool::new());

    // The pool can be cloned and shared across threads.
    let pool_clone = pool.clone();

    // Insert values and get handles.
    let u32_handle = pool.insert(42_u32);
    let string_handle = pool.insert("hello".to_string());

    // Access values through dereferencing.
    println!("u32 value: {}", *u32_handle);
    println!("string value: {}", *string_handle);

    // Get raw pointers for unsafe access.
    let ptr = u32_handle.ptr();
    // SAFETY: The pointer is valid and contains the value we just inserted.
    let value = unsafe { ptr.read() };
    println!("Value read via pointer: {value}");

    // Clone handles - the value remains alive as long as any handle exists.
    let cloned_handle = u32_handle.clone();
    drop(u32_handle);
    println!("Value still accessible: {}", *cloned_handle);

    // Use different types in the same pool.
    let float_handle = pool_clone.insert(2.5_f64);
    println!("Float value: {}", *float_handle);

    // Check pool statistics.
    println!("Pool length: {}", pool.len());
    println!("Pool is empty: {}", pool.is_empty());

    // Values are automatically removed when all handles are dropped.
    drop(string_handle);
    drop(cloned_handle);
    drop(float_handle);

    println!("Pool length after cleanup: {}", pool.len());
    println!("Pool is empty after cleanup: {}", pool.is_empty());
}

fn single_threaded_example() {
    // Create a local pool (single-threaded, higher performance)
    let pool = LocalBlindPool::from(RawBlindPool::new());

    // Insert values and get handles.
    let u32_handle = pool.insert(100_u32);
    let string_handle = pool.insert("local".to_string());

    // Access values through dereferencing.
    println!("u32 value: {}", *u32_handle);
    println!("string value: {}", *string_handle);

    // Clone handles work the same way.
    let cloned_handle = u32_handle.clone();
    println!("Cloned handle value: {}", *cloned_handle);

    // Pool statistics.
    println!("Pool length: {}", pool.len());

    // Automatic cleanup works the same way.
    drop(u32_handle);
    drop(cloned_handle);
    drop(string_handle);

    println!("Pool length after cleanup: {}", pool.len());
    println!("Pool is empty after cleanup: {}", pool.is_empty());
}

fn raw_example() {
    // Create a blind pool that can store any type.
    let mut pool = RawBlindPool::new();

    // Insert values of different types into the same pool.
    let pooled_u64 = pool.insert(42_u64);
    let pooled_i32 = pool.insert(-123_i32);
    let _pooled_f32 = pool.insert(2.71_f32);

    // The handles act like super-powered pointers - they can be copied freely.
    let pooled_u64_copy = pooled_u64;

    // Read data back from the pooled items.
    // SAFETY: The pointer is valid and the value was just inserted.
    let value_u64 = unsafe { pooled_u64.ptr().read() };

    // SAFETY: Both handles refer to the same stored value.
    let value_u64_copy = unsafe { pooled_u64_copy.ptr().read() };

    // SAFETY: The pointer is valid and the value was just inserted.
    let value_i32 = unsafe { pooled_i32.ptr().read() };

    assert_eq!(value_u64, 42);
    assert_eq!(value_u64_copy, 42);
    assert_eq!(value_i32, -123);

    println!("Retrieved u64 value: {value_u64}");
    println!("Retrieved u64 copy value: {value_u64_copy}");
    println!("Retrieved i32 value: {value_i32}");
    println!("README example completed successfully!");
}
