//! Example demonstrating single-threaded usage of `LocalBlindPool`.
//!
//! This example shows when and how to use `LocalBlindPool` for single-threaded scenarios.
//! `LocalBlindPool` provides the same automatic resource management as `BlindPool` but with
//! lower overhead since it doesn't need thread synchronization.

use std::f64::consts::PI;

use blind_pool::LocalBlindPool;

fn main() {
    println!("LocalBlindPool Single-Threaded Example");
    println!("======================================");

    // Create a new local blind pool using the builder pattern.
    let pool = LocalBlindPool::builder().build_local();

    println!();
    println!("Created empty LocalBlindPool:");
    println!("  Length: {}", pool.len());
    println!("  Is empty: {}", pool.is_empty());

    // Insert different types into the same pool.
    println!();
    println!("Inserting different types...");
    let handle_u32 = pool.insert(42_u32);
    let handle_f64 = pool.insert(PI);
    let handle_string = pool.insert("Single-threaded pool".to_string());
    let handle_vec = pool.insert(vec![1, 2, 3, 4, 5]);

    println!("  Inserted u32: 42");
    println!("  Inserted f64: PI");
    println!("  Inserted string: 'Single-threaded pool'");
    println!("  Inserted vec: [1, 2, 3, 4, 5]");

    println!();
    println!("Pool status after insertions:");
    println!("  Length: {}", pool.len());
    println!("  Is empty: {}", pool.is_empty());

    // Demonstrate automatic dereferencing.
    println!();
    println!("Accessing values through automatic dereferencing:");
    println!("  u32 value: {}", *handle_u32);
    println!("  f64 value: {}", *handle_f64);
    println!("  string value: '{}'", *handle_string);
    println!("  vector length: {}", handle_vec.len());
    println!("  vector sum: {}", handle_vec.iter().sum::<i32>());

    // Demonstrate that handles can be cloned freely.
    println!();
    println!("Demonstrating cloneable handles...");
    let handle_u32_clone = handle_u32.clone();
    let handle_string_clone = handle_string.clone();

    println!("  Original u32 handle: {}", *handle_u32);
    println!("  Cloned u32 handle: {}", *handle_u32_clone);
    println!("  Original string handle: '{}'", *handle_string);
    println!("  Cloned string handle: '{}'", *handle_string_clone);

    // Show both values refer to the same data.
    assert_eq!(*handle_u32, *handle_u32_clone);
    assert_eq!(*handle_string, *handle_string_clone);

    // Demonstrate that the pool itself can be cloned.
    println!();
    println!("Demonstrating pool cloning...");
    let pool_clone = pool.clone();
    
    // Insert into the cloned pool handle
    let handle_bool = pool_clone.insert(true);
    println!("  Inserted bool into cloned pool: {}", *handle_bool);
    
    // Both pool handles see the same data
    println!("  Original pool length: {}", pool.len());
    println!("  Cloned pool length: {}", pool_clone.len());

    // Demonstrate raw pointer access.
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

    // Demonstrate automatic cleanup.
    println!();
    println!("Demonstrating automatic cleanup...");
    println!("  Pool length before dropping handles: {}", pool.len());
    
    // Drop some handles explicitly
    drop(handle_f64);
    drop(handle_bool);
    drop(erased_u32); // This was the handle for erasure demonstration
    
    println!("  Pool length after dropping some handles: {}", pool.len());

    // Demonstrate working with collections efficiently.
    println!();
    println!("Demonstrating efficient collection operations...");
    
    let mut collection_handles = Vec::new();
    
    // Insert a collection of strings
    let words = ["hello", "world", "from", "local", "pool"];
    for word in &words {
        let handle = pool.insert(word.to_string());
        collection_handles.push(handle);
    }
    
    println!("  Inserted {} words", words.len());
    println!("  Pool length: {}", pool.len());
    
    // Process the collection
    let mut concatenated = String::new();
    for (i, handle) in collection_handles.iter().enumerate() {
        if i > 0 {
            concatenated.push(' ');
        }
        concatenated.push_str(handle);
    }
    
    println!("  Concatenated string: '{concatenated}'");
    
    // Keep the handles for automatic cleanup
    println!("  Collection handles will be cleaned up automatically");

    println!();
    println!("When to use LocalBlindPool:");
    println!("1. Single-threaded applications where thread safety isn't needed");
    println!("2. Performance-critical code where mutex overhead should be avoided");
    println!("3. Libraries that know they'll only be used in single-threaded contexts");
    println!("4. Embedded systems or other resource-constrained environments");
    println!("5. When building single-threaded data structures or algorithms");

    println!();
    println!("Performance benefits:");
    println!("- No mutex overhead (faster than BlindPool in single-threaded use)");
    println!("- Uses Rc instead of Arc (lighter weight reference counting)");
    println!("- RefCell instead of Mutex (no blocking or lock contention)");
    println!("- Same automatic cleanup as BlindPool, just more efficient");

    println!();
    println!("IMPORTANT: LocalBlindPool is NOT thread-safe!");
    println!("It cannot be sent across threads or accessed from multiple threads.");
    println!("Use BlindPool if you need thread safety.");

    println!();
    println!("Example completed successfully!");
    println!("Final pool state - Length: {}, Empty: {}", pool.len(), pool.is_empty());
    println!();
    println!("All handles will be automatically cleaned up when main() ends.");
}
