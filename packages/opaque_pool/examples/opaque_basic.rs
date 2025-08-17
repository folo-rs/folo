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

    // Demonstrate that handles act like super-powered pointers - they can be copied freely.
    let pooled1_copy = pooled1;
    #[expect(
        clippy::clone_on_copy,
        reason = "demonstrating that Clone is available"
    )]
    let pooled1_clone = pooled1.clone();

    println!("Created copies of the first handle");

    // Read values back through the handles - all copies refer to the same stored value.
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value1_original = unsafe { pooled1.ptr().cast::<u32>().read() };
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value1_copy = unsafe { pooled1_copy.ptr().cast::<u32>().read() };
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value1_clone = unsafe { pooled1_clone.ptr().cast::<u32>().read() };
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value2 = unsafe { pooled2.ptr().cast::<u32>().read() };
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value3 = unsafe { pooled3.ptr().cast::<u32>().read() };

    println!("Value 1 (original): {value1_original:#x}");
    println!("Value 1 (copy): {value1_copy:#x}");
    println!("Value 1 (clone): {value1_clone:#x}");
    println!("Value 2: {value2:#x}");
    println!("Value 3: {value3:#x}");

    // All copies should refer to the same value.
    assert_eq!(value1_original, value1_copy);
    assert_eq!(value1_original, value1_clone);

    println!(
        "Pool now has {} items with capacity {}",
        pool.len(),
        pool.capacity()
    );

    // Remove one item using any of the handles.
    pool.remove(&pooled2);
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

    // Verify some values.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test uses small values that fit in u32"
    )]
    for (i, pooled) in pooled_items.iter().take(5).enumerate() {
        // SAFETY: The pointers are valid and the memory contains the values we just inserted.
        unsafe {
            let value = pooled.ptr().cast::<u32>().read();
            println!("Pooled item contains value: {value}");
            assert_eq!(value, i as u32);
        }
    }

    // Clean up the remaining pooled items.
    for pooled in pooled_items {
        pool.remove(&pooled);
    }

    // Also clean up the first and third items we still have (using any of the copies).
    pool.remove(&pooled1); // Could also use pooled1_copy or pooled1_clone
    pool.remove(&pooled3);

    println!("All items cleaned up successfully!");
    println!("OpaquePool example completed successfully!");
}
