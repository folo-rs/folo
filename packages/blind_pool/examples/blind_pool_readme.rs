//! Concise example demonstrating all three blind pool variants.
//!
//! This shows `BlindPool` (automatic, thread-safe), `LocalBlindPool` (automatic, single-threaded), 
//! and `RawBlindPool` (manual management) with the essential features of each.

use blind_pool::{BlindPool, LocalBlindPool, RawBlindPool};

fn main() {
    println!("=== BlindPool (Recommended) ===");
    automatic_thread_safe();

    println!("\n=== LocalBlindPool (Single-threaded) ===");
    automatic_local();

    println!("\n=== RawBlindPool (Manual) ===");
    manual_management();
}

fn automatic_thread_safe() {
    // Create pool with automatic cleanup and thread safety.
    let pool = BlindPool::builder().build();

    // Insert different types, get handles with automatic dereferencing.
    let u32_handle = pool.insert(42_u32);
    let string_handle = pool.insert("hello".to_string());
    
    println!("Values: {} and '{}'", *u32_handle, *string_handle);
    
    // Clone handles freely - values stay alive.
    let cloned = u32_handle.clone();
    println!("Cloned handle: {}", *cloned);
    
    // Raw pointer access when needed.
    // SAFETY: Pointer is valid for inserted value.
    let raw_value = unsafe { u32_handle.ptr().read() };
    println!("Raw access: {raw_value}");
    
    // Automatic cleanup when handles are dropped.
}

fn automatic_local() {
    // Single-threaded version with lower overhead.
    let pool = LocalBlindPool::builder().build_local();
    
    let handle = pool.insert(vec![1, 2, 3]);
    println!("Vector length: {}", handle.len());
    
    // Same automatic cleanup, just not thread-safe.
}

fn manual_management() {
    // Manual control over cleanup timing.
    let mut pool = RawBlindPool::builder().build_raw();
    
    let handle = pool.insert(99_u64);
    
    // SAFETY: Pointer valid for inserted value.
    let value = unsafe { handle.ptr().read() };
    println!("Manual value: {value}");
    
    // Must manually remove from pool.
    pool.remove(handle);
}
