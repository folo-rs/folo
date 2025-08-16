//! Example demonstrating single-threaded usage of `LocalBlindPool`.
//!
//! This shows the single-threaded pool with automatic resource management.
//! More efficient than `BlindPool` when thread safety is not needed.

use blind_pool::LocalBlindPool;

fn main() {
    println!("=== LocalBlindPool: Single-threaded, Automatic Cleanup ===");

    // Create a single-threaded pool (more efficient than BlindPool).
    let pool = LocalBlindPool::new();

    // Insert different types.
    let number = pool.insert(42_u32);
    let text = pool.insert("Hello".to_string());
    let list = pool.insert(vec![1, 2, 3]);

    // Access values through automatic dereferencing.
    println!("Number: {}", *number);
    println!("Text: {}", *text);
    println!("List: {:?}", *list);

    // Clone handles freely.
    let number_copy = number;
    println!("Number copy: {}", *number_copy);

    // Pool can be cloned (shares same underlying storage).
    let pool_clone = pool.clone();
    let _from_clone = pool_clone.insert(99_i32);

    println!("Pool length: {}", pool.len());

    // NOT thread-safe - cannot send across threads.
    // This would fail to compile:
    // std::thread::spawn(move || { println!("{}", *number); });

    // Automatic cleanup when handles are dropped - no manual cleanup needed!
}
