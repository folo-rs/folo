//! A high-performance object pool that guarantees pinning of its items and enables safe
//! out-of-band access via unsafe code.
//!
//! This crate provides [`PinnedPool`], a collection that stores items with guaranteed stable
//! memory addresses. Once an item is inserted, it remains pinned in memory until explicitly
//! removed, making it safe to create pointers to items for out-of-band access.
//!
//! # Key Features
//!
//! * **Guaranteed pinning**: All items remain at stable memory addresses until removed
//! * **Fast key-based lookup**: Efficient access via opaque [`Key`] handles
//! * **Out-of-band access**: Safe concurrent access to multiple items via pointers
//! * **Dynamic capacity**: Automatic growth as needed with no upfront allocation
//! * **Zero-copy insertion**: Advanced insertion API for immediate item access
//!
//! # Use Cases
//!
//! - High-performance data structures requiring stable pointers
//! - Concurrent systems where items need out-of-band access
//! - Memory pools for objects that must not move in memory
//! - Collections where pinning guarantees are essential
//!
//! The pool is designed to offer efficient pinned storage for large collections. The exact
//! storage mechanism/layout is not part of the API contract and may change in future versions.
//!
//! # Basic usage
//!
//! ```
//! use pinned_pool::PinnedPool;
//!
//! let mut pool = PinnedPool::<String>::new();
//!
//! // Inserting an item gives you a key that you can later use to look up the item again.
//! let alice_key = pool.insert("Alice".to_string());
//! let bob_key = pool.insert("Bob".to_string());
//! let charlie_key = pool.insert("Charlie".to_string());
//!
//! println!(
//!     "Object pool contains {} items, with an auto-adjusting capacity of {}",
//!     pool.len(),
//!     pool.capacity()
//! );
//!
//! // Retrieving items from a pool is fast, similar to `Vec[key]`.
//! let alice = pool.get(alice_key);
//! println!("Retrieved item: {alice}");
//!
//! pool.remove(bob_key);
//! pool.remove(charlie_key);
//!
//! // Retrieving an item borrows the pool for as long as you use the item, so we have to
//! // re-lookup `alice` here because otherwise the above `remove()` would be blocked.
//! let alice = pool.get(alice_key);
//! println!("Retrieved item after removal of other items: {alice}",);
//!
//! // You can also modify the items in-place.
//! let mut alice = pool.get_mut(alice_key);
//! alice.push_str(" Smith");
//! println!("Modified item: {alice}");
//! ```
//!
//! # Out of band concurrent access
//!
//! This requires unsafe code, which lowers the safety guardrails that prevent you from writing
//! invalid code. You are still not allowed to write invalid code but now you take on some of
//! the responsibility instead of leaving it all up to the compiler.
//!
//! ```
//! use std::ptr;
//!
//! use pinned_pool::{DropPolicy, PinnedPool};
//!
//! let mut pool = PinnedPool::<String>::builder()
//!     // This is an extra safeguard, requiring you to remove all items from the pool before
//!     // you drop it. It helps detect situations where you have forgotten to remove some items
//!     // and are at risk of accessing items from unsafe code after the pool is destroyed. You
//!     // are recommended to apply this policy for any pool whose items you access via pointers.
//!     .drop_policy(DropPolicy::MustNotDropItems)
//!     .build();
//!
//! // As we know that we will be accessing the items from unsafe code, we can immediately
//! // transform them to pointers at insertion time, without doing even a single lookup, for
//! // optimal efficiency. Every little bit adds up if you do it 100K times per second!
//! let inserter = pool.begin_insert();
//! let alice_key = inserter.key();
//! let alice_ptr = ptr::from_mut(inserter.insert_mut("Alice".to_string()).get_mut());
//!
//! let inserter = pool.begin_insert();
//! let bob_key = inserter.key();
//! let bob_ptr = ptr::from_mut(inserter.insert_mut("Bob".to_string()).get_mut());
//!
//! let inserter = pool.begin_insert();
//! let charlie_key = inserter.key();
//! let charlie_ptr = ptr::from_mut(inserter.insert_mut("Charlie".to_string()).get_mut());
//!
//! println!(
//!     "Object pool contains {} items, with an auto-adjusting capacity of {}",
//!     pool.len(),
//!     pool.capacity()
//! );
//!
//! // We can do whatever we want to the items through the pointers, including writing
//! // to them (as long as we got the pointer from `insert_mut()`), provided that we
//! // do not access the item via the pool methods concurrently (e.g. `remove()` but
//! // also `get()` and `get_mut()` because they may create conflicting references).
//!
//! // Modify Alice's name
//! unsafe {
//!     // SAFETY: We have exclusive access to this item via the pointer, and no
//!     // concurrent access via pool methods. The pointer is valid until removed.
//!     (*alice_ptr).push_str(" Smith");
//! }
//!
//! // Modify Bob's name
//! unsafe {
//!     // SAFETY: We have exclusive access to this item via the pointer, and no
//!     // concurrent access via pool methods. The pointer is valid until removed.
//!     (*bob_ptr).push_str(" Johnson");
//! }
//!
//! // Modify Charlie's name
//! unsafe {
//!     // SAFETY: We have exclusive access to this item via the pointer, and no
//!     // concurrent access via pool methods. The pointer is valid until removed.
//!     (*charlie_ptr).push_str(" Brown");
//! }
//!
//! // Once we remove an item, the pointer to it becomes invalid.
//! //
//! // This implies that we can only call this if we are 100% confident that nothing is concurrently
//! // accessing the item via the pointer (otherwise we would invalidate a pointer that is in use).
//! // We can know this is valid because we can read the code to determine there is no concurrent
//! // access (it is all in this one function) - it is not so easy in a real application. There is
//! // no compiler protection here - only human diligence can ensure this code is valid.
//! pool.remove(bob_key);
//!
//! // Same applies to get() - only valid if there is no conflicting access via pointers.
//! // By "conflicting" we mean something that would violate Rust's aliasing rules (either creating
//! // two `&mut` exclusive references or a `&mut` exclusive reference and a `&` shared reference
//! // at the same time to the same item in different parts of the app).
//! let charlie = pool.get(charlie_key);
//! println!("Retrieved item: {charlie}");
//!
//! // SAFETY: The reference `charlie` is not used after this point, so we can safely mutate
//! // the item via the pointer again. If we had still used `charlie` after this `println!`,
//! // this would be invalid code because this pointer dereference creates an exclusive reference.
//! unsafe {
//!     // SAFETY: No conflicting references exist to this item - the `charlie` reference
//!     // is no longer used and we have exclusive access via the pointer.
//!     (*charlie_ptr).push_str(" von Neumann");
//! }
//!
//! // Read items via pointers for display
//! let alice_name = unsafe {
//!     // SAFETY: We have valid pointers to these items and no conflicting references.
//!     &*alice_ptr
//! };
//! let charlie_name = unsafe {
//!     // SAFETY: We have valid pointers to these items and no conflicting references.
//!     &*charlie_ptr
//! };
//!
//! println!(
//!     "Items accessed via pointers: Alice = {}, Charlie = {}",
//!     alice_name, charlie_name
//! );
//!
//! // The policy we set at the beginning requires us to remove all items from the pool
//! // before we drop it. This is to ensure that we do not accidentally access items from
//! // unsafe code after the pool is dropped. It is an optional but recommended safeguard.
//! pool.remove(alice_key);
//! pool.remove(charlie_key);
//! ```

mod builder;
mod drop_policy;
mod pinned_pool;
mod pinned_slab;

pub use builder::*;
pub use drop_policy::*;
pub use pinned_pool::*;
pub(crate) use pinned_slab::*;
