//! Demo showing `OpaquePool` panic-on-drop functionality.

use std::alloc::Layout;

use opaque_pool::OpaquePool;

fn main() {
    println!("OpaquePool Drop Panic Demo");
    println!("============================");

    // Demonstrate normal operation - no panic.
    {
        println!("1. Normal operation - pool with proper cleanup (no panic expected):");
        let layout = Layout::new::<u64>();
        let mut pool = OpaquePool::new(layout);

        // SAFETY: u64 matches the layout used to create the pool.
        let pooled = unsafe { pool.insert(42_u64) };
        println!("   Inserted value: 42");

        // SAFETY: The pointer is valid and the memory contains the value we just inserted.
        let value = unsafe { pooled.ptr().cast::<u64>().read() };
        println!("   Read value: {value}");

        pool.remove(pooled);
        println!("   Removed value - pool should drop cleanly");
        // Pool drops here without panic.
    }
    println!("   ✓ Pool dropped successfully - no active items");

    // Demonstrate panic behavior.
    println!("\n2. Demonstrating panic behavior - pool with active item:");
    println!("   (This will cause a panic when the pool is dropped)");

    std::panic::set_hook(Box::new(|panic_info| {
        if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            println!("   ✓ Panic caught: {s}");
        } else if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            println!("   ✓ Panic caught: {s}");
        } else {
            println!("   ✓ Panic caught (unknown payload type)");
        }
    }));

    let result = std::panic::catch_unwind(|| {
        let layout = Layout::new::<u64>();
        let mut pool = OpaquePool::new(layout);

        // SAFETY: u64 matches the layout used to create the pool.
        let _pooled = unsafe { pool.insert(123_u64) }; // This item is never removed!
        println!("   Inserted value but will not remove it");

        // Pool will panic on drop because the item is still active.
    });

    match result {
        Ok(()) => println!("   ❌ Expected panic did not occur"),
        Err(_) => println!("   ✓ Pool correctly panicked when dropped with active item"),
    }

    println!("\nDemo completed - the panic-on-drop functionality is working correctly!");
}
