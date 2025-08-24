//! Example demonstrating the capacity management features of pool types.
//!
//! This shows how to use `capacity_of`, `reserve_for`, and `shrink_to_fit` methods
//! across [`BlindPool`], [`LocalBlindPool`], and [`RawBlindPool`].

use blind_pool::{BlindPool, LocalBlindPool, RawBlindPool};

fn main() {
    println!("=== Capacity Management Features ===\n");

    demonstrate_blind_pool();
    println!();
    demonstrate_local_blind_pool();
    println!();
    demonstrate_raw_blind_pool();
}

fn demonstrate_blind_pool() {
    println!("--- BlindPool (Thread-safe) ---");
    let pool = BlindPool::new();

    // Initially no capacity for any type
    println!("Initial capacity for u32: {}", pool.capacity_of::<u32>());
    println!("Initial capacity for String: {}", pool.capacity_of::<String>());

    // Reserve capacity before inserting
    pool.reserve_for::<u32>(50);
    println!("After reserving 50 u32s: {}", pool.capacity_of::<u32>());

    // Insert some items
    let mut handles = Vec::new();
    for i in 0..10 {
        handles.push(pool.insert(i));
    }
    println!("After inserting 10 u32s: capacity={}, len={}", 
             pool.capacity_of::<u32>(), pool.len());

    // Insert different type
    let _string_handle = pool.insert("Hello".to_string());
    println!("After inserting String: u32_capacity={}, string_capacity={}, len={}", 
             pool.capacity_of::<u32>(), pool.capacity_of::<String>(), pool.len());

    // Drop all handles to empty the pool
    drop(handles);
    drop(_string_handle);
    println!("After dropping all handles: len={}, u32_capacity={}", 
             pool.len(), pool.capacity_of::<u32>());

    // Shrink to fit
    pool.shrink_to_fit();
    println!("After shrink_to_fit: u32_capacity={}, string_capacity={}", 
             pool.capacity_of::<u32>(), pool.capacity_of::<String>());
}

fn demonstrate_local_blind_pool() {
    println!("--- LocalBlindPool (Single-threaded) ---");
    let pool = LocalBlindPool::new();

    // Same API as BlindPool, but single-threaded
    pool.reserve_for::<f64>(25);
    println!("Reserved capacity for f64: {}", pool.capacity_of::<f64>());

    let mut handles = Vec::new();
    for i in 0..5 {
        handles.push(pool.insert(f64::from(i) * 1.5));
    }
    println!("After inserting 5 f64s: capacity={}, len={}", 
             pool.capacity_of::<f64>(), pool.len());

    drop(handles);
    pool.shrink_to_fit();
    println!("After cleanup and shrink: capacity={}, len={}", 
             pool.capacity_of::<f64>(), pool.len());
}

fn demonstrate_raw_blind_pool() {
    println!("--- RawBlindPool (Manual management) ---");
    let mut pool = RawBlindPool::new();

    // Same capacity management, but requires manual cleanup
    pool.reserve_for::<i32>(100);
    println!("Reserved capacity for i32: {}", pool.capacity_of::<i32>());

    let mut handles = Vec::new();
    for i in 0_i32..20 {
        handles.push(pool.insert(i.saturating_mul(2)));
    }
    println!("After inserting 20 i32s: capacity={}, len={}", 
             pool.capacity_of::<i32>(), pool.len());

    // Manual cleanup required
    for handle in handles {
        // SAFETY: Each handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&handle);
        }
    }
    println!("After manual cleanup: capacity={}, len={}", 
             pool.capacity_of::<i32>(), pool.len());

    pool.shrink_to_fit();
    println!("After shrink_to_fit: capacity={}, len={}", 
             pool.capacity_of::<i32>(), pool.len());
}