//! Basic usage of the `pinned_pool` crate:
//!
//! * Creating a pool.
//! * Adding items.
//! * Retrieving items.
//! * Removing items.

use pinned_pool::PinnedPool;

fn main() {
    let mut pool = PinnedPool::<String>::new();

    // Inserting an item gives you a key that you can later use to look up the item again.
    let alice_key = pool.insert("Alice".to_string());
    let bob_key = pool.insert("Bob".to_string());
    let charlie_key = pool.insert("Charlie".to_string());

    println!(
        "Object pool contains {} items, with an auto-adjusting capacity of {}",
        pool.len(),
        pool.capacity()
    );

    // Retrieving items from a pool is fast, similar to `Vec[key]`.
    let alice = pool.get(alice_key);
    println!("Retrieved item: {alice}");

    pool.remove(bob_key);
    pool.remove(charlie_key);

    // Retrieving an item borrows the pool for as long as you use the item, so we have to
    // re-lookup `alice` here because otherwise the above `remove()` would be blocked.
    let alice = pool.get(alice_key);
    println!("Retrieved item after removal of other items: {alice}",);

    // Does this mean you can only access one item at a time?
    // Yes and no. There are two options to access multiple items concurrently:
    //
    // 1. Wrap the items in `Arc` (and use interior mutability if you wish to mutate).
    // 2. Use unsafe code. See `pp_out_of_band.rs´.

    // You can also modify the items in-place.
    let mut alice = pool.get_mut(alice_key);
    alice.push_str(" Smith");
    println!("Modified item: {alice}");
}
