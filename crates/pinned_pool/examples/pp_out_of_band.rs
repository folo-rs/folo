//! Showcases how to efficiently access multiple items in a pool concurrently.
//! This is a variation of `pp_basic.rs` - familiarize with that example first.
//!
//! This requires unsafe code, which lowers the safety guardrails that prevent you from writing
//! invalid code. You are still not allowed to write invalid code but now you take on some of
//! the responsibility instead of leaving it all up to the compiler.

use std::ptr;

use pinned_pool::{DropPolicy, PinnedPool};

fn main() {
    let mut pool = PinnedPool::<String>::builder()
        // This is an extra safeguard, requiring you to remove all items from the pool before
        // you drop it. It helps detect situations where you have forgotten to remove some items
        // and are at risk of accessing items from unsafe code after the pool is destroyed. You
        // are recommended to apply this policy for any pool whose items you access via pointers.
        .drop_policy(DropPolicy::MustNotDropItems)
        .build();

    // As we know that we will be accessing the items from unsafe code, we can immediately
    // transform them to pointers at insertion time, without doing even a single lookup, for
    // optimal efficiency. Every little bit adds up if you do it 100K times per second!
    let inserter = pool.begin_insert();
    let alice_key = inserter.key();
    let alice_ptr = ptr::from_mut(inserter.insert_mut("Alice".to_string()).get_mut());

    let inserter = pool.begin_insert();
    let bob_key = inserter.key();
    let bob_ptr = ptr::from_mut(inserter.insert_mut("Bob".to_string()).get_mut());

    let inserter = pool.begin_insert();
    let charlie_key = inserter.key();
    let charlie_ptr = ptr::from_mut(inserter.insert_mut("Charlie".to_string()).get_mut());

    println!(
        "Object pool contains {} items, with an auto-adjusting capacity of {}",
        pool.len(),
        pool.capacity()
    );

    // We can do whatever we want to the items through the pointers, including writing
    // to them (as long as we got the pointer from `insert_mut()`), provided that we
    // do not access the item via the pool methods concurrently (e.g. `remove()` but
    // also `get()` because `get()` may create conflicting references, which are not valid).

    // SAFETY: See above comment.
    unsafe {
        (*alice_ptr).push_str(" Smith");
    }
    // SAFETY: See above comment.
    unsafe {
        (*bob_ptr).push_str(" Johnson");
    }
    // SAFETY: See above comment.
    unsafe {
        (*charlie_ptr).push_str(" Brown");
    }

    // Once we remove an item, the pointer to it becomes invalid.
    //
    // This implies that we can only call this if we are 100% confident that nothing is concurrently
    // accessing the item via the pointer (otherwise we would invalidate a pointer that is in use).
    // We can know this is valid because we can read the code to determine there is no concurrent
    // access (it is all in this one function) - it is not so easy in a real application. There is
    // no compiler protection here - only human diligence can ensure this code is valid.
    pool.remove(bob_key);

    // Same applies to get() - only valid if there is no conflicting access via pointers.
    // By "conflicting" we mean something that would violate Rust's aliasing rules (either creating
    // two `&mut` exclusive references or a `&mut` exclusive reference and a `&` shared reference
    // at the same time to the same item in different parts of the app).
    let charlie = pool.get(charlie_key);
    println!("Retrieved item: {charlie}");

    // The reference `charlie` is not used after this point, so we can safely access
    // the item via pointers again. If we had still used `charlie` after this `println!`,
    // this would be invalid code.
    println!(
        "Items accessed via pointers: Alice = {}, Charlie = {}",
        // SAFETY: See above comments.
        unsafe { &*alice_ptr },
        // SAFETY: See above comments.
        unsafe { &*charlie_ptr }
    );

    // The policy we set at the beginning requires us to remove all items from the pool
    // before we drop it. This is to ensure that we do not accidentally access items from
    // unsafe code after the pool is dropped. It is an optional but recommended safeguard.
    pool.remove(alice_key);
    pool.remove(charlie_key);
}
