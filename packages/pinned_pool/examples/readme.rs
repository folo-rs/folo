//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `PinnedPool` for object management.

use pinned_pool::PinnedPool;

fn main() {
    println!("=== Pinned Pool README Example ===");
    
    let mut pool = PinnedPool::<String>::new();

    // Inserting an item gives you a key that you can later use to look up the item again.
    let alice_key = pool.insert("Alice".to_string());
    let bob_key = pool.insert("Bob".to_string());
    let charlie_key = pool.insert("Charlie".to_string());

    // Retrieving items from a pool is fast, similar to `Vec[key]`.
    let alice = pool.get(alice_key);
    println!("Retrieved item: {alice}");

    pool.remove(bob_key);
    pool.remove(charlie_key);

    // You can also modify the items in-place.
    let mut alice = pool.get_mut(alice_key);
    alice.push_str(" Smith");
    println!("Modified item: {alice}");
    
    println!("README example completed successfully!");
}
