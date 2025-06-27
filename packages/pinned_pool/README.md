An object pool that guarantees pinning of its items and enables easy item access
via unsafe code by not maintaining any Rust references to its items.

```rust
use pinned_pool::PinnedPool;

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
```

More details in the [package documentation](https://docs.rs/pinned_pool/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.