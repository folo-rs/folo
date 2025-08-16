//! Example demonstrating manual resource management with `RawBlindPool`.
//!
//! This shows the raw pool requiring manual cleanup.
//! Use when you need precise control over resource lifecycles.

use blind_pool::RawBlindPool;

fn main() {
    println!("=== RawBlindPool: Manual Resource Management ===");

    // Create a raw pool - requires manual cleanup.
    let mut pool = RawBlindPool::new();

    // Insert different types.
    let number = pool.insert(42_u32);
    let text = pool.insert("Hello".to_string());
    let list = pool.insert(vec![1, 2, 3]);

    // Access values through raw pointers.
    // SAFETY: Pointers are valid and contain the values we just inserted.
    let number_val = unsafe { number.ptr().read() };
    // SAFETY: Pointer is valid and contains the value we just inserted.
    let text_ref = unsafe { text.ptr().as_ref() };
    // SAFETY: Pointer is valid and contains the value we just inserted.
    let list_ref = unsafe { list.ptr().as_ref() };
    
    println!("Number: {number_val}");
    println!("Text: {text_ref}");
    println!("List: {list_ref:?}");

    println!("Pool length: {}", pool.len());

    // CRITICAL: Must manually remove all items or you get memory leaks!
    pool.remove(number);
    pool.remove(text);
    pool.remove(list);

    println!("Pool after cleanup: {}", pool.len());

    // No automatic cleanup - you must call pool.remove() for every handle!
}
