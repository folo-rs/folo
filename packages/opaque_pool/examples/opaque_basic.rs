//! Basic usage example for `OpaquePool`.
//!
//! This example demonstrates how to use `OpaquePool` to manage type-erased memory
//! with dynamic capacity growth, including the copyable nature of pool handles.

use opaque_pool::OpaquePool;

fn main() {
    // Create a pool for u32 values.
    let mut pool = OpaquePool::builder().layout_of::<u32>().build();

    println!("Created OpaquePool with capacity: {}", pool.capacity());

    // Insert some values.
    // SAFETY: u32 matches the layout used to create the pool.
    let pooled1 = unsafe { pool.insert(0xdeadbeef_u32) };
    // SAFETY: u32 matches the layout used to create the pool.
    let pooled2 = unsafe { pool.insert(0xcafebabe_u32) };
    // SAFETY: u32 matches the layout used to create the pool.
    let pooled3 = unsafe { pool.insert(0xfeedface_u32) };

    println!("Inserted 3 items");

    // Access values directly through Deref - much cleaner than unsafe pointer operations!
    let value1 = *pooled1;
    let value2 = *pooled2;
    let value3 = *pooled3;

    println!("Value 1: {value1:#x}");
    println!("Value 2: {value2:#x}");
    println!("Value 3: {value3:#x}");

    // Convert the first item to shared for demonstration of copyable handles
    let pooled1_shared = pooled1.into_shared();
    let pooled1_copy = pooled1_shared;
    #[expect(
        clippy::clone_on_copy,
        reason = "demonstrating that Clone is available"
    )]
    let pooled1_clone = pooled1_shared.clone();

    println!("Created copies of the first handle");

    // Read values back through the shared handles - all copies refer to the same stored value.
    let value1_original = *pooled1_shared;
    let value1_copy = *pooled1_copy;
    let value1_clone = *pooled1_clone;

    println!("Value 1 (original): {value1_original:#x}");
    println!("Value 1 (copy): {value1_copy:#x}");
    println!("Value 1 (clone): {value1_clone:#x}");

    // All copies should refer to the same value.
    assert_eq!(value1_original, value1_copy);
    assert_eq!(value1_original, value1_clone);
    assert_eq!(value1_original, 0xdeadbeef_u32);

    println!(
        "Pool now has {} items with capacity {}",
        pool.len(),
        pool.capacity()
    );

    // Remove one item using the exclusive handle (safer than shared removal).
    pool.remove_mut(pooled2);
    println!("Removed item");

    // The pool automatically grows as needed.
    let mut pooled_items = Vec::new();
    for i in 0..100 {
        // SAFETY: u32 matches the layout used to create the pool.
        let pooled = unsafe { pool.insert(i) };
        pooled_items.push(pooled);
    }

    println!(
        "Added 100 more items. Pool now has {} items with capacity {}",
        pool.len(),
        pool.capacity()
    );

    // Verify some values using clean Deref syntax.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test uses small values that fit in u32"
    )]
    for (i, pooled) in pooled_items.iter().take(5).enumerate() {
        let value = **pooled; // Deref the pooled item directly
        println!("Pooled item contains value: {value}");
        assert_eq!(value, i as u32);
    }

    println!("All items will be automatically cleaned up when pool is dropped");
    println!("OpaquePool example completed successfully!");
}
