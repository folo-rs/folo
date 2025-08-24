//! Demonstrates memory management features of `OpaquePool`.
//!
//! This example shows how to use capacity management, reservation, and shrinking
//! features to control memory usage and optimize performance.

use opaque_pool::OpaquePool;

// Constants for examples.
const BATCH_SIZE: usize = 1000;

/// Demonstrates automatic capacity growth as items are added.
fn demonstrate_capacity_growth() {
    println!("Example 1: Automatic capacity growth");
    println!("------------------------------------");

    let mut pool = OpaquePool::builder().layout_of::<u64>().build();

    println!("Initial state:");
    println!("  Length: {}, Capacity: {}", pool.len(), pool.capacity());
    println!("  Is empty: {}", pool.is_empty());

    // Insert a few items to trigger initial capacity allocation.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "example code with small values"
    )]
    for i in 0_u64..5 {
        // SAFETY: u64 matches the layout used to create the pool.
        let _item = unsafe { pool.insert(i * 100) };
        println!(
            "  After inserting item {}: Length={}, Capacity={}",
            i + 1,
            pool.len(),
            pool.capacity()
        );
    }

    println!();
}

/// Demonstrates pre-allocating capacity to avoid reallocations.
fn demonstrate_capacity_reservation() {
    println!("Example 2: Pre-allocating capacity");
    println!("----------------------------------");

    let mut reserved_pool = OpaquePool::builder().layout_of::<String>().build();

    println!("Before reservation:");
    println!(
        "  Length: {}, Capacity: {}",
        reserved_pool.len(),
        reserved_pool.capacity()
    );

    // Reserve space for 200 items up front to avoid reallocations.
    reserved_pool.reserve(200);

    println!("After reserving space for 200 items:");
    println!(
        "  Length: {}, Capacity: {}",
        reserved_pool.len(),
        reserved_pool.capacity()
    );

    // Insert many items - no additional capacity allocations needed.
    let mut reserved_items = Vec::new();
    for i in 0..150 {
        // SAFETY: String matches the layout used to create the pool.
        let item = unsafe { reserved_pool.insert(format!("Item {i}")) };
        reserved_items.push(item);
    }

    println!("After inserting 150 items:");
    println!(
        "  Length: {}, Capacity: {}",
        reserved_pool.len(),
        reserved_pool.capacity()
    );
    println!("  Note: Capacity didn't change because we pre-allocated!");

    // Clean up since items are not automatically released.
    for item in reserved_items {
        reserved_pool.remove_mut(item);
    }

    println!();
}

/// Demonstrates shrinking unused capacity to reclaim memory.
fn demonstrate_capacity_shrinking() {
    println!("Example 3: Shrinking unused capacity");
    println!("------------------------------------");

    let mut pool = OpaquePool::builder().layout_of::<String>().build();
    pool.reserve(200);

    // Fill up the pool.
    let mut items = Vec::new();
    for i in 0..150 {
        // SAFETY: String matches the layout used to create the pool.
        let item = unsafe { pool.insert(format!("Item {i}")) };
        items.push(item);
    }

    println!("After filling pool with 150 items:");
    println!("  Length: {}, Capacity: {}", pool.len(), pool.capacity());

    // Remove most items to create unused capacity.
    let items_to_keep = 3;
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "example with safe subtraction"
    )]
    let items_to_remove = items.len() - items_to_keep;

    for _ in 0..items_to_remove {
        if let Some(item) = items.pop() {
            pool.remove_mut(item);
        }
    }

    println!("After removing {items_to_remove} items:");
    println!("  Length: {}, Capacity: {}", pool.len(), pool.capacity());
    println!(
        "  Wasted capacity: {}",
        pool.capacity().saturating_sub(pool.len())
    );

    // Shrink to fit the actual number of items.
    pool.shrink_to_fit();

    println!("After shrink_to_fit():");
    println!("  Length: {}, Capacity: {}", pool.len(), pool.capacity());
    println!(
        "  Wasted capacity: {}",
        pool.capacity().saturating_sub(pool.len())
    );

    // Clean up remaining items.
    for item in items {
        pool.remove_mut(item);
    }

    println!();
}

/// Demonstrates layout information for different types.
fn demonstrate_layout_information() {
    println!("Example 4: Layout information");
    println!("----------------------------");

    let u32_pool = OpaquePool::builder().layout_of::<u32>().build();
    let string_pool = OpaquePool::builder().layout_of::<String>().build();

    println!("u32 pool layout:");
    println!(
        "  Size: {} bytes, Align: {} bytes",
        u32_pool.item_layout().size(),
        u32_pool.item_layout().align()
    );

    println!("String pool layout:");
    println!(
        "  Size: {} bytes, Align: {} bytes",
        string_pool.item_layout().size(),
        string_pool.item_layout().align()
    );

    println!();
}

/// Demonstrates large batch operations with pre-allocation.
fn demonstrate_large_batch_operations() {
    println!("Example 5: Large batch operations with pre-allocation");
    println!("-----------------------------------------------------");

    let mut batch_pool = OpaquePool::builder().layout_of::<i32>().build();
    batch_pool.reserve(BATCH_SIZE);

    println!("Reserved capacity for {BATCH_SIZE} items");
    println!("  Capacity: {}", batch_pool.capacity());

    // Insert a large batch.
    let mut batch_items = Vec::with_capacity(BATCH_SIZE);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "example uses small values that fit in i32"
    )]
    for i in 0..BATCH_SIZE {
        // SAFETY: i32 matches the layout used to create the pool.
        let item = unsafe { batch_pool.insert(i as i32) };
        batch_items.push(item);
    }

    println!("After batch insertion:");
    println!(
        "  Length: {}, Capacity: {}",
        batch_pool.len(),
        batch_pool.capacity()
    );

    // Verify some values.
    println!("Verifying batch values...");
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "example uses small values that fit in i32"
    )]
    for (expected, item) in batch_items.iter().take(5).enumerate() {
        assert_eq!(**item, expected as i32);
    }
    println!("  First 5 values verified successfully");

    // Clean up.
    for item in batch_items {
        batch_pool.remove_mut(item);
    }

    println!();
}

fn main() {
    println!("=== OpaquePool Memory Management Examples ===");
    println!();

    demonstrate_capacity_growth();
    demonstrate_capacity_reservation();
    demonstrate_capacity_shrinking();
    demonstrate_layout_information();
    demonstrate_large_batch_operations();

    println!("Memory management best practices:");
    println!("- Use reserve() when you know the approximate number of items");
    println!("- Call shrink_to_fit() after removing many items to reclaim memory");
    println!("- Monitor len() and capacity() to understand memory usage");
    println!("- Larger batch operations benefit significantly from pre-allocation");
    println!();
    println!("Memory management example completed successfully!");
}
