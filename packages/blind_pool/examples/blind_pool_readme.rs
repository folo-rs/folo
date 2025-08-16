//! `BlindPool` README example showcasing key features.
//!
//! This demonstrates the main `BlindPool` type with its automatic cleanup and thread safety,
//! plus the powerful trait object functionality that makes blind pools unique.

use std::fmt::Display;

use blind_pool::{BlindPool, define_pooled_dyn_cast};

// Enable casting to Display trait objects
define_pooled_dyn_cast!(Display);

fn main() {
    println!("=== BlindPool: Basic Usage ===");
    basic_usage();

    println!("\n=== BlindPool: Trait Objects ===");
    trait_object_usage();

    println!("\n=== Other Variants ===");
    other_variants();
}

fn basic_usage() {
    // Create pool with automatic cleanup and thread safety.
    let pool = BlindPool::new();

    // Insert different types, get handles with automatic dereferencing.
    let u32_handle = pool.insert(42_u32);
    let string_handle = pool.insert("hello".to_string());

    println!("Values: {} and '{}'", *u32_handle, *string_handle);

    // Clone handles freely - values stay alive.
    let cloned = u32_handle;
    println!("Moved handle: {}", *cloned);

    // Automatic cleanup when handles are dropped.
}

fn trait_object_usage() {
    let pool = BlindPool::new();

    // Insert different types that implement Display
    let int_handle = pool.insert(123_i32);
    let float_handle = pool.insert(45.67_f64);
    let string_handle = pool.insert("world".to_string());

    // Cast to trait objects while preserving reference counting
    let int_display = int_handle.cast_display();
    let float_display = float_handle.cast_display();
    let string_display = string_handle.cast_display();

    // Use them uniformly through the trait
    print_values(&[&*int_display, &*float_display, &*string_display]);
}

fn print_values(values: &[&dyn Display]) {
    println!("Displaying values:");
    for (i, value) in values.iter().enumerate() {
        println!("  {i}: {value}");
    }
}

fn other_variants() {
    println!("Also available:");
    println!("  - LocalBlindPool::new() for single-threaded usage");
    println!("  - RawBlindPool::new() for manual memory management");
}
