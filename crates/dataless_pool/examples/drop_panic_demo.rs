//! Demo showing `DatalessPool` panic-on-drop functionality.

use std::alloc::Layout;

use dataless_pool::DatalessPool;

fn main() {
    println!("DatalessPool Drop Panic Demo");
    println!("============================");

    // Demonstrate normal operation - no panic.
    {
        println!("1. Normal operation - pool with proper cleanup (no panic expected):");
        let layout = Layout::new::<u64>();
        let mut pool = DatalessPool::new(layout);

        let reservation = pool.reserve();
        // SAFETY: The pointer is valid and aligned for u64, and we own the memory.
        unsafe {
            reservation.ptr().cast::<u64>().as_ptr().write(42);
        }
        println!("   Wrote value: 42");
        // SAFETY: The pointer is valid and the memory was just initialized.
        let value = unsafe { reservation.ptr().cast::<u64>().as_ptr().read() };
        println!("   Read value: {value}");

        // SAFETY: We know the reservation is valid since we just created it.
        unsafe {
            pool.release(reservation);
        }
        println!("   Released reservation - pool should drop cleanly");
        // Pool drops here without panic.
    }
    println!("   ✓ Pool dropped successfully - no active reservations");

    // Demonstrate panic behavior.
    println!("\n2. Demonstrating panic behavior - pool with active reservation:");
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
        let mut pool = DatalessPool::new(layout);

        let _reservation = pool.reserve(); // This reservation is never released!
        println!("   Created reservation but will not release it");

        // Pool will panic on drop because reservation is still active.
    });

    match result {
        Ok(()) => println!("   ❌ Expected panic did not occur"),
        Err(_) => println!("   ✓ Pool correctly panicked when dropped with active reservation"),
    }

    println!("\nDemo completed - the panic-on-drop functionality is working correctly!");
}
