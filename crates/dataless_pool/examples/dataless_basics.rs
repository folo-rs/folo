//! Basic usage example for `DatalessPool`.
//!
//! This example demonstrates how to use `DatalessPool` to manage type-erased memory.
//! with dynamic capacity growth.

use std::alloc::Layout;

use dataless_pool::DatalessPool;

fn main() {
    // Create a pool for u32 values.
    let layout = Layout::new::<u32>();
    let mut pool = DatalessPool::new(layout);

    println!("Created DatalessPool with capacity: {}", pool.capacity());

    // Reserve some values.
    let reservation1 = pool.reserve();
    let reservation2 = pool.reserve();
    let reservation3 = pool.reserve();

    // Write values through the pointers.
    // SAFETY: We just reserved these pointers and are writing the correct type.
    unsafe {
        reservation1.ptr().cast::<u32>().as_ptr().write(0xdeadbeef);
    }
    // SAFETY: We just reserved these pointers and are writing the correct type.
    unsafe {
        reservation2.ptr().cast::<u32>().as_ptr().write(0xcafebabe);
    }
    // SAFETY: We just reserved these pointers and are writing the correct type.
    unsafe {
        reservation3.ptr().cast::<u32>().as_ptr().write(0xfeedface);
    }

    println!("Reserved 3 items");

    // Read values back through the reservations.
    // SAFETY: We just wrote these values and are reading the correct type.
    let value1 = unsafe { reservation1.ptr().cast::<u32>().as_ptr().read() };
    // SAFETY: We just wrote these values and are reading the correct type.
    let value2 = unsafe { reservation2.ptr().cast::<u32>().as_ptr().read() };
    // SAFETY: We just wrote these values and are reading the correct type.
    let value3 = unsafe { reservation3.ptr().cast::<u32>().as_ptr().read() };

    println!("Value 1: {value1:#x}");
    println!("Value 2: {value2:#x}");
    println!("Value 3: {value3:#x}");

    println!(
        "Pool now has {} items with capacity {}",
        pool.len(),
        pool.capacity()
    );

    // Release one item.
    pool.release(reservation2);
    println!("Released item");

    // The pool automatically grows as needed.
    let mut reservations = Vec::new();
    for i in 0..100 {
        let reservation = pool.reserve();

        // SAFETY: We just reserved this pointer and are writing the correct type.
        unsafe {
            reservation.ptr().cast::<u32>().as_ptr().write(i);
        }

        reservations.push(reservation);
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
    for (i, reservation) in reservations.iter().take(5).enumerate() {
        // SAFETY: We just wrote these values and are reading the correct type.
        unsafe {
            let value = reservation.ptr().cast::<u32>().as_ptr().read();
            println!("Reservation contains value: {value}");
            assert_eq!(value, i as u32);
        }
    }

    println!("DatalessPool example completed successfully!");
}
