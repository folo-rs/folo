//! Basic usage example for `DatalessPool`.
//!
//! This example demonstrates how to use `DatalessPool` to manage type-erased memory
//! with dynamic capacity growth.

use std::alloc::Layout;

use dataless_pool::DatalessPool;

fn main() {
    // Create a pool for u32 values.
    let layout = Layout::new::<u32>();
    let mut pool = DatalessPool::new(layout);

    println!("Created DatalessPool with capacity: {}", pool.capacity());

    // Insert some values.
    // SAFETY: u32 matches the layout used to create the pool.
    let pooled1 = unsafe { pool.insert(0xdeadbeef_u32) };
    // SAFETY: u32 matches the layout used to create the pool.
    let pooled2 = unsafe { pool.insert(0xcafebabe_u32) };
    // SAFETY: u32 matches the layout used to create the pool.
    let pooled3 = unsafe { pool.insert(0xfeedface_u32) };

    println!("Inserted 3 items");

    // Read values back through the handles.
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value1 = unsafe { pooled1.ptr().cast::<u32>().read() };
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value2 = unsafe { pooled2.ptr().cast::<u32>().read() };
    // SAFETY: The pointers are valid and the memory contains the values we just inserted.
    let value3 = unsafe { pooled3.ptr().cast::<u32>().read() };

    println!("Value 1: {value1:#x}");
    println!("Value 2: {value2:#x}");
    println!("Value 3: {value3:#x}");

    println!(
        "Pool now has {} items with capacity {}",
        pool.len(),
        pool.capacity()
    );

    // Remove one item.
    pool.remove(pooled2);
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
        pool.remove(pooled);
    }

    // Also clean up the first and third items we still have.
    pool.remove(pooled1);
    pool.remove(pooled3);

    println!("All items cleaned up successfully!");
    println!("DatalessPool example completed successfully!");
}
