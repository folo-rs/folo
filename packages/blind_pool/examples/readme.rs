//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `BlindPool` to store objects of any type.

use blind_pool::BlindPool;

fn main() {
    println!("=== Blind Pool README Example ===");

    // Create a blind pool that can store any type.
    let mut pool = BlindPool::new();

    // Insert values of different types into the same pool.
    let pooled_u64 = pool.insert(42_u64);
    let pooled_i32 = pool.insert(-123_i32);
    let _pooled_f32 = pool.insert(2.71_f32);

    // Read data back from the pooled items.
    // SAFETY: The pointer is valid and the value was just inserted.
    let value_u64 = unsafe { pooled_u64.ptr().read() };

    // SAFETY: The pointer is valid and the value was just inserted.
    let value_i32 = unsafe { pooled_i32.ptr().read() };

    assert_eq!(value_u64, 42);
    assert_eq!(value_i32, -123);

    println!("Retrieved u64 value: {value_u64}");
    println!("Retrieved i32 value: {value_i32}");
    println!("README example completed successfully!");
}
