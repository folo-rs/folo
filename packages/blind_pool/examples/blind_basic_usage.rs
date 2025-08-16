//! Example demonstrating basic usage of `BlindPool` with automatic cleanup.
//!
//! This shows the thread-safe pool with automatic resource management.
//! Best choice for most use cases.

use blind_pool::BlindPool;

fn main() {
    println!("=== BlindPool: Thread-safe, Automatic Cleanup ===");

    // Create a thread-safe pool with automatic cleanup.
    let pool = BlindPool::new();

    // Insert different types.
    let number = pool.insert(42_u32);
    let text = pool.insert("Hello".to_string());
    let list = pool.insert(vec![1, 2, 3]);

    // Access values through automatic dereferencing.
    println!("Number: {}", *number);
    println!("Text: {}", *text);
    println!("List: {:?}", *list);

    // Clone handles freely.
    let number_copy = number.clone();
    println!("Number copy: {}", *number_copy);

    // Thread-safe sharing.
    let pool_clone = pool.clone();
    let number_clone = number;
    
    std::thread::spawn(move || {
        println!("From thread: {}", *number_clone);
        let _from_thread = pool_clone.insert(99_i32);
    }).join().unwrap();

    println!("Pool length: {}", pool.len());

    // Automatic cleanup when handles are dropped - no manual cleanup needed!
}
