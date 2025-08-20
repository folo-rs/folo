//! Example demonstrating the `into_shared()` method for converting exclusive handles to shared handles.

use blind_pool::{BlindPool, LocalBlindPool};

fn main() {
    thread_safe_example();
    println!("---");
    single_threaded_example();
}

fn thread_safe_example() {
    println!("Thread-safe BlindPool example:");
    
    let pool = BlindPool::new();
    
    // Create an exclusive handle and modify the value
    let mut exclusive_handle = pool.insert_mut("Hello".to_string());
    exclusive_handle.push_str(" World");
    println!("Exclusive handle value: {}", *exclusive_handle);
    
    // Convert to shared handle - now we can create multiple references
    let shared_handle = exclusive_handle.into_shared();
    println!("Shared handle value: {}", *shared_handle);
    
    // Create multiple shared references
    let cloned_handle1 = shared_handle.clone();
    let cloned_handle2 = shared_handle.clone();
    
    println!("All handles point to the same value:");
    println!("  Original: {}", *shared_handle);
    println!("  Clone 1:  {}", *cloned_handle1);
    println!("  Clone 2:  {}", *cloned_handle2);
    
    // The pool still contains one item
    println!("Pool length: {}", pool.len());
    
    // When all handles are dropped, the value is automatically cleaned up
    drop(shared_handle);
    drop(cloned_handle1);
    println!("Pool length after dropping 2 handles: {}", pool.len());
    
    drop(cloned_handle2);
    println!("Pool length after dropping all handles: {}", pool.len());
}

fn single_threaded_example() {
    println!("Single-threaded LocalBlindPool example:");
    
    let pool = LocalBlindPool::new();
    
    // Create an exclusive handle with a vector
    let mut exclusive_handle = pool.insert_mut(vec![1, 2, 3]);
    exclusive_handle.push(4);
    exclusive_handle.push(5);
    println!("Exclusive handle value: {:?}", *exclusive_handle);
    
    // Convert to shared handle
    let shared_handle = exclusive_handle.into_shared();
    println!("Shared handle value: {:?}", *shared_handle);
    
    // Create multiple shared references
    let cloned_handle = shared_handle.clone();
    
    println!("Both handles access the same vector:");
    println!("  Original: {:?}", *shared_handle);
    println!("  Clone:    {:?}", *cloned_handle);
    
    // The pool still contains one item
    println!("Pool length: {}", pool.len());
    
    // Automatic cleanup when all handles are dropped
    drop(shared_handle);
    drop(cloned_handle);
    println!("Pool length after cleanup: {}", pool.len());
}