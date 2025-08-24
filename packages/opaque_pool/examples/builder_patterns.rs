//! Demonstrates the builder pattern and configuration options for `OpaquePool`.
//!
//! This example shows all the different ways to configure an `OpaquePool`
//! using the builder pattern, including layout specification and drop policies.

use std::alloc::Layout;

use opaque_pool::{DropPolicy, OpaquePool};

fn main() {
    println!("=== OpaquePool Builder Pattern Examples ===");
    println!();

    // Example 1: Type-based layout (most common)
    println!("Example 1: Type-based layout configuration");
    println!("------------------------------------------");

    let mut pool1 = OpaquePool::builder().layout_of::<String>().build();

    println!("Created pool for String type:");
    println!("  Layout size: {} bytes", pool1.item_layout().size());
    println!("  Layout align: {} bytes", pool1.item_layout().align());
    println!("  Initial length: {}", pool1.len());
    println!("  Initial capacity: {}", pool1.capacity());

    println!();

    // Example 2: Explicit layout specification
    println!("Example 2: Explicit layout specification");
    println!("----------------------------------------");

    // Create a custom layout for a hypothetical 256-byte aligned structure.
    let custom_layout = Layout::from_size_align(64, 8).expect("Valid layout parameters");

    let pool2 = OpaquePool::builder().layout(custom_layout).build();

    println!("Created pool with custom layout:");
    println!("  Layout size: {} bytes", pool2.item_layout().size());
    println!("  Layout align: {} bytes", pool2.item_layout().align());

    println!();

    // Example 3: Drop policy configuration
    println!("Example 3: Drop policy configuration");
    println!("------------------------------------");

    // Pool that must be empty when dropped.
    let mut strict_pool = OpaquePool::builder()
        .layout_of::<u64>()
        .drop_policy(DropPolicy::MustNotDropItems)
        .build();

    println!("Created strict pool (must be empty when dropped):");
    println!("  Drop policy: MustNotDropItems");

    // Pool that can be dropped with items remaining.
    let _lenient_pool = OpaquePool::builder()
        .layout_of::<u64>()
        .drop_policy(DropPolicy::MayDropItems)
        .build();

    println!("Created lenient pool (may drop items):");
    println!("  Drop policy: MayDropItems");

    println!();

    // Example 4: Chained builder configuration
    println!("Example 4: Chained builder configuration");
    println!("----------------------------------------");

    let mut configured_pool = OpaquePool::builder()
        .layout_of::<Vec<i32>>() // Store Vec<i32>
        .drop_policy(DropPolicy::MustNotDropItems) // Strict cleanup required
        .build();

    println!("Created fully configured pool:");
    println!("  Type: Vec<i32>");
    println!(
        "  Layout size: {} bytes",
        configured_pool.item_layout().size()
    );
    println!(
        "  Layout align: {} bytes",
        configured_pool.item_layout().align()
    );
    println!("  Drop policy: MustNotDropItems");

    println!();

    // Example 5: Different types with same layout
    println!("Example 5: Different types with compatible layouts");
    println!("-------------------------------------------------");

    // These types have the same layout on most platforms.
    let u64_layout = Layout::new::<u64>();
    let i64_layout = Layout::new::<i64>();
    let usize_layout = Layout::new::<usize>();

    println!("Layout comparison:");
    println!(
        "  u64:   size={:2}, align={}",
        u64_layout.size(),
        u64_layout.align()
    );
    println!(
        "  i64:   size={:2}, align={}",
        i64_layout.size(),
        i64_layout.align()
    );
    println!(
        "  usize: size={:2}, align={}",
        usize_layout.size(),
        usize_layout.align()
    );

    // Create a pool that can store any 8-byte aligned, 8-byte value.
    let _multi_type_pool = OpaquePool::builder()
        .layout(u64_layout) // Using u64 layout
        .build();

    println!();
    println!("Created multi-type pool using u64 layout:");
    println!("  Can store u64, i64, f64, and other compatible types");

    println!();

    // Example 6: Builder pattern with method chaining variations
    println!("Example 6: Builder pattern variations");
    println!("-------------------------------------");

    // Verbose style (each method on separate line).
    let verbose_pool = OpaquePool::builder()
        .layout_of::<String>()
        .drop_policy(DropPolicy::MayDropItems)
        .build();

    // Compact style (all chained).
    let compact_pool = OpaquePool::builder().layout_of::<u32>().build();

    // Intermediate variable style.
    let mut builder = OpaquePool::builder();
    builder = builder.layout_of::<f64>();
    builder = builder.drop_policy(DropPolicy::MustNotDropItems);
    let intermediate_pool = builder.build();

    println!("Created pools using different builder styles:");
    println!(
        "  Verbose pool:      {} byte items",
        verbose_pool.item_layout().size()
    );
    println!(
        "  Compact pool:      {} byte items",
        compact_pool.item_layout().size()
    );
    println!(
        "  Intermediate pool: {} byte items",
        intermediate_pool.item_layout().size()
    );

    println!();

    // Example 7: Error handling (would panic in real code)
    println!("Example 7: Builder validation");
    println!("-----------------------------");

    println!("Builder pattern includes validation:");
    println!("  - Layout must be specified before calling build()");
    println!("  - Layout must have non-zero size");
    println!("  - Invalid layouts are rejected at build time");

    // This would panic: OpaquePool::builder().build(); // No layout specified!
    // This would panic:
    // let zero_layout = Layout::from_size_align(0, 1).unwrap();
    // OpaquePool::builder().layout(zero_layout).build();

    println!("  (Commented out to avoid panics in this example)");

    println!();

    // Demonstrate that all pools work.
    // SAFETY: String matches the layout used to create pool1.
    let item1 = unsafe { pool1.insert("Hello".to_string()) };

    // SAFETY: u64 matches the layout used to create strict_pool.
    let item2 = unsafe { strict_pool.insert(12345_u64) };

    // SAFETY: Vec<i32> matches the layout used to create configured_pool.
    let item3 = unsafe { configured_pool.insert(vec![1, 2, 3, 4, 5]) };

    println!("Demonstration that all pools work:");
    println!("  pool1: {}", &*item1);
    println!("  strict_pool: {}", *item2);
    println!("  configured_pool: {item3:?}");

    // Clean up for strict pools (required by MustNotDropItems policy).
    pool1.remove_mut(item1);
    strict_pool.remove_mut(item2);
    configured_pool.remove_mut(item3);
    drop(intermediate_pool); // Empty, so safe to drop

    // Note: lenient_pool, compact_pool, verbose_pool use MayDropItems (default)
    // so they can be dropped even if they contained items.

    println!();
    println!("Builder pattern best practices:");
    println!("- Always specify layout using .layout_of::<T>() or .layout()");
    println!("- Choose appropriate drop policy for your use case");
    println!("- Use method chaining for concise, readable configuration");
    println!("- The builder validates configuration at build time");
    println!();
    println!("Builder pattern example completed successfully!");
}
