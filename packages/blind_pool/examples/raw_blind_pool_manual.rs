//! Example demonstrating manual resource management with `RawBlindPool`.
//!
//! This example shows when and how to use `RawBlindPool` for scenarios requiring explicit
//! control over memory allocation and deallocation. Use this API when automatic cleanup
//! is not desired or when you need precise control over resource lifecycle.

use blind_pool::RawBlindPool;

fn main() {
    println!("RawBlindPool Manual Management Example");
    println!("=====================================");

    // Create a new raw blind pool using the builder pattern.
    let mut pool = RawBlindPool::builder().build_raw();

    println!();
    println!("Created empty RawBlindPool:");
    println!("  Length: {}", pool.len());
    println!("  Is empty: {}", pool.is_empty());

    // Insert values and get raw handles that require manual cleanup.
    println!();
    println!("Inserting values with manual management...");
    let handle_u32 = pool.insert(42_u32);
    let handle_f64 = pool.insert(3.14159_f64);
    let handle_string = pool.insert("Manual management".to_string());

    println!("  Inserted u32: 42");
    println!("  Inserted f64: 3.14159");
    println!("  Inserted string: 'Manual management'");

    println!();
    println!("Pool status after insertions:");
    println!("  Length: {}", pool.len());

    // Access values through raw pointer access (copying values out).
    println!();
    println!("Accessing values via raw pointers:");
    // SAFETY: The pointer is valid and contains the value we just inserted.
    let u32_value = unsafe { handle_u32.ptr().read() };
    let f64_value = unsafe { handle_f64.ptr().read() };
    // SAFETY: The pointer is valid and contains the value we just inserted.
    let string_value = unsafe { handle_string.ptr().as_ref() };
    println!("  u32 value: {u32_value}");
    println!("  f64 value: {f64_value}");
    println!("  string value: '{}'", string_value);

    // Demonstrate manual memory management - this is the key difference.
    println!();
    println!("Demonstrating manual cleanup...");
    println!("  Pool length before cleanup: {}", pool.len());
    // CRITICAL: With RawBlindPool, you must manually remove items from the pool.
    // Handles do NOT automatically clean up when dropped.
    pool.remove(handle_u32);
    println!("  Removed u32 value");
    println!("  Pool length after removing u32: {}", pool.len());
    pool.remove(handle_f64);
    println!("  Removed f64 value");
    println!("  Pool length after removing f64: {}", pool.len());
    pool.remove(handle_string);
    println!("  Removed string value");
    println!("  Pool length after removing string: {}", pool.len());

    println!();
    println!("Pool is now empty: {}", pool.is_empty());

    // Demonstrate a common pattern: batch operations with explicit cleanup.
    println!();
    println!("Demonstrating batch operations with explicit cleanup...");
    let mut handles = Vec::new();
    for i in 0..5 {
        let handle = pool.insert(i * i); // Insert squares
        handles.push(handle);
    }
    println!("  Inserted 5 square values");
    println!("  Pool length: {}", pool.len());
    let mut sum = 0;
    for (i, &handle) in handles.iter().enumerate() {
        let value = unsafe { handle.ptr().read() };
        sum += value;
        println!("    Square of {i}: {value}");
    }
    println!("  Sum of all squares: {sum}");
    for handle in handles {
        pool.remove(handle);
    }
    println!("  Cleaned up all batch items");
    println!("  Pool length after cleanup: {}", pool.len());

    println!();
    println!("IMPORTANT: Unlike BlindPool, RawBlindPool handles do NOT automatically");
    println!("clean up when dropped. You MUST call pool.remove() for every handle,");
    println!("or you will have memory leaks!");
    println!();
    println!("Note: The remove() method drops values in place - it does not return them.");
    println!("If you need the value after removal, read it first using ptr().read().");
    println!();
    println!("Example completed successfully!");
    println!(
        "Final pool state - Length: {}, Empty: {}",
        pool.len(),
        pool.is_empty()
    );
}
