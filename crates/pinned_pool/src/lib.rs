//! An object pool that guarantees pinning of its items and enables easy item access
//! via unsafe code by not maintaining any Rust references to its items.
//!
//! Features:
//!
//! * All items are guaranteed to be pinned for as long as they are in the pool.
//! * Fast lookup by opaque key returned from the pool.
//! * Out of band concurrent access to multiple items via pointers while
//!   simultaneously adding/removing items from the pool.
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
//! // SAFETY: See above comment.
//! unsafe {
//!     (*alice_ptr).push_str(" Smith");
//! }
//! // SAFETY: See above comment.
//! unsafe {
//!     (*bob_ptr).push_str(" Johnson");
//! }
//! // SAFETY: See above comment.
//! unsafe {
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
//!     (*charlie_ptr).push_str(" von Neumann");
//! }
//!
//! println!(
//!     "Items accessed via pointers: Alice = {}, Charlie = {}",
//!     // SAFETY: See above comments.
//!     unsafe { &*alice_ptr },
//!     // SAFETY: See above comments.
//!     unsafe { &*charlie_ptr }
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
