//! Example demonstrating basic usage of `BlindPool` for storing different types.
//!
//! This example shows the recommended `BlindPool` API with automatic resource management.
//! The pool handles cleanup automatically when handles are dropped, making it the easiest
//! and safest option for most use cases.

use std::f32::consts::PI;
use std::f64::consts::E;

use blind_pool::BlindPool;

fn main() {
    println!("BlindPool Basic Usage Example");
    println!("============================");

    // Create a new blind pool using the builder pattern (recommended).
    let pool = BlindPool::builder().build();

    println!();
    println!("Created empty BlindPool:");
    println!("  Length: {}", pool.len());
    println!("  Is empty: {}", pool.is_empty());

    // Insert different types into the same pool.
    println!();
    println!("Inserting different types...");
    let handle_u32 = pool.insert(42_u32);
    let handle_u64 = pool.insert(1234567890_u64);
    let handle_f32 = pool.insert(PI);
    let handle_f64 = pool.insert(E);
    let handle_bool = pool.insert(true);
    let handle_string = pool.insert("Hello, BlindPool!".to_string());

    println!("  Inserted u32: 42");
    println!("  Inserted u64: 1234567890");
    println!("  Inserted f32: PI");
    println!("  Inserted f64: E");
    println!("  Inserted bool: true");
    println!("  Inserted string: 'Hello, BlindPool!'");

    println!();
    println!("Pool status after insertions:");
    println!("  Length: {}", pool.len());
    println!("  Is empty: {}", pool.is_empty());

    // Demonstrate automatic dereferencing.
    println!();
    println!("Accessing values through automatic dereferencing:");
    println!("  u32 value: {}", *handle_u32);
    println!("  u64 value: {}", *handle_u64);
    println!("  f32 value: {}", *handle_f32);
    println!("  f64 value: {}", *handle_f64);
    println!("  bool value: {}", *handle_bool);
    println!("  string value: '{}'", *handle_string);
    println!("  string length: {}", handle_string.len()); // Direct method access

    // Demonstrate that handles can be copied and cloned freely.
    println!();
    println!("Demonstrating copyable and cloneable handles...");
    let handle_u32_copy = handle_u32.clone(); // Clone to avoid move
    let handle_string_clone = handle_string.clone(); // Clone

    println!("  Original u32 handle: {}", *handle_u32);
    println!("  Copied u32 handle: {}", *handle_u32_copy);
    println!("  Original string handle: '{}'", *handle_string);
    println!("  Cloned string handle: '{}'", *handle_string_clone);

    // Show both values refer to the same data.
    assert_eq!(*handle_u32, *handle_u32_copy);
    assert_eq!(*handle_string, *handle_string_clone);

    // Demonstrate raw pointer access for advanced use cases.
    println!();
    println!("Demonstrating raw pointer access:");

    // SAFETY: The pointer is valid and contains the value we just inserted.
    let ptr_value = unsafe { handle_f64.ptr().read() };
    println!("  f64 value via ptr: {ptr_value}");

    // Demonstrate type erasure.
    println!();
    println!("Demonstrating type erasure...");

    // Create a separate handle for erasure (can't erase when multiple references exist)
    let handle_for_erasure = pool.insert(999_u32);
    let erased_u32 = handle_for_erasure.erase(); // Erase type information

    // Can still access raw pointer after type erasure.
    // SAFETY: We know this erased handle contains a u32.
    let val = unsafe { erased_u32.ptr().cast::<u32>().read() };
    println!("  Erased u32 value: {val}");

    // Demonstrate multithreading capabilities.
    println!();
    println!("Demonstrating thread safety...");

    let pool_clone = pool.clone(); // Pool can be cloned and shared
    let handle_clone = handle_f32; // Handles can be sent across threads

    let join_handle = std::thread::spawn(move || {
        // Access the value in another thread
        println!("  Accessed f32 from thread: {}", *handle_clone);

        // Insert new value from thread
        let thread_handle = pool_clone.insert(99_i32);
        *thread_handle
    });

    let thread_result = join_handle.join().unwrap();
    println!("  Value inserted from thread: {thread_result}");

    // Demonstrate automatic cleanup.
    println!();
    println!("Demonstrating automatic cleanup...");
    println!("  Pool length before dropping handles: {}", pool.len());

    // Drop some handles explicitly
    drop(handle_u64);
    drop(handle_bool);
    drop(erased_u32); // This was the handle for erasure demonstration

    println!("  Pool length after dropping some handles: {}", pool.len());

    // Remaining handles will be automatically cleaned up when they go out of scope
    println!();
    println!("Remaining handles will be automatically cleaned up when main() ends.");
    println!("This demonstrates the key advantage of BlindPool - no manual memory management!");

    println!();
    println!("Example completed successfully!");
    println!();
    println!("Key features demonstrated:");
    println!("- Automatic resource management (no manual cleanup needed)");
    println!("- Type safety with automatic dereferencing");
    println!("- Thread safety (pool and handles can be shared across threads)");
    println!("- Type erasure for advanced use cases");
    println!("- Raw pointer access when needed");
}
