//! Basic usage examples for `infinity_pool`

use std::fmt::Display;

use infinity_pool::{BlindPool, PinnedPool, define_pooled_dyn_cast};

// Enable casting to Display trait objects
define_pooled_dyn_cast!(Display);

fn main() {
    // PinnedPool: thread-safe pool for a single type
    let mut pinned_pool = PinnedPool::<String>::new();
    let handle = pinned_pool.insert("Hello, PinnedPool!".to_string());

    // Cast PinnedPool handle to trait object and pass to function
    let display_pinned = handle.cast_display();
    print_item("PinnedPool", &display_pinned);

    // BlindPool: thread-safe pool for multiple types
    let mut blind_pool = BlindPool::new();
    let string_handle = blind_pool.insert("Hello, BlindPool!".to_string());
    let number_handle = blind_pool.insert(42_i32);

    // Cast to trait objects and pass to function that accepts AsRef<dyn Display>
    let display_string = string_handle.cast_display();
    let display_number = number_handle.cast_display();

    print_item("BlindPool string", &display_string);
    print_item("BlindPool number", &display_number);
}

/// Function that accepts anything that can be borrowed as a Display trait object
fn print_item(label: &str, item: &impl AsRef<dyn Display>) {
    println!("{}: {}", label, item.as_ref());
}
