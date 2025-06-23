//! Basic usage example for `MemoryPool`.
//!
//! This example demonstrates how to use `MemoryPool` to manage type-erased memory
//! with dynamic capacity growth.

use std::alloc::Layout;

use memory_slab::MemoryPool;

fn main() {
    // Create a pool for u32 values
    let layout = Layout::new::<u32>();
    let mut pool = MemoryPool::new(layout);

    println!("Created MemoryPool with capacity: {}", pool.capacity());

    // Reserve some values
    let (key1, ptr1) = pool.reserve();
    let (key2, ptr2) = pool.reserve();
    let (key3, ptr3) = pool.reserve();

    // Write values through the pointers
    // SAFETY: We just reserved these pointers and are writing the correct type
    unsafe {
        ptr1.cast::<u32>().as_ptr().write(0xdeadbeef);
    }
    // SAFETY: We just reserved these pointers and are writing the correct type
    unsafe {
        ptr2.cast::<u32>().as_ptr().write(0xcafebabe);
    }
    // SAFETY: We just reserved these pointers and are writing the correct type
    unsafe {
        ptr3.cast::<u32>().as_ptr().write(0xfeedface);
    }

    println!("Reserved 3 items at keys: {key1:?}, {key2:?}, {key3:?}");

    // Read values back through the keys
    // SAFETY: We just wrote these values and are reading the correct type
    let value1 = unsafe { pool.get(key1).cast::<u32>().as_ptr().read() };
    // SAFETY: We just wrote these values and are reading the correct type
    let value2 = unsafe { pool.get(key2).cast::<u32>().as_ptr().read() };
    // SAFETY: We just wrote these values and are reading the correct type
    let value3 = unsafe { pool.get(key3).cast::<u32>().as_ptr().read() };

    println!("Value 1 via key: {value1:#x}");
    println!("Value 2 via key: {value2:#x}");
    println!("Value 3 via key: {value3:#x}");

    println!(
        "Pool now has {} items with capacity {}",
        pool.len(),
        pool.capacity()
    );

    // Release one item
    pool.release(key2);
    println!("Released item at key {key2:?}");

    // The pool automatically grows as needed
    let mut keys = Vec::new();
    for i in 0..100 {
        let (key, ptr) = pool.reserve();

        // SAFETY: We just reserved this pointer and are writing the correct type
        unsafe {
            ptr.cast::<u32>().as_ptr().write(i);
        }

        keys.push(key);
    }

    println!(
        "Added 100 more items. Pool now has {} items with capacity {}",
        pool.len(),
        pool.capacity()
    );

    // Verify some values
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test uses small values that fit in u32"
    )]
    for (i, &key) in keys.iter().take(5).enumerate() {
        // SAFETY: We just wrote these values and are reading the correct type
        unsafe {
            let value = pool.get(key).cast::<u32>().as_ptr().read();
            println!("Key {key:?} contains value: {value}");
            assert_eq!(value, i as u32);
        }
    }

    println!("MemoryPool example completed successfully!");
}
