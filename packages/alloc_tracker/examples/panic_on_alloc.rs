//! Example demonstrating panic-on-next-allocation functionality.
//!
//! This example shows how to use the `panic_on_next_alloc` function to detect
//! unexpected allocations in performance-critical code sections.

use alloc_tracker::{Allocator, panic_on_next_alloc};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("Demonstrating panic-on-next-allocation functionality...");

    // Normal allocation should work fine
    println!("Allocating some memory...");
    #[expect(clippy::useless_vec, reason = "example needs to show allocation")]
    let mut data = vec![1, 2, 3, 4, 5];
    println!("Successfully allocated vector with {} elements", data.len());

    // Enable panic on next allocation
    println!("Enabling panic-on-next-allocation...");
    panic_on_next_alloc(true);

    // This section should not allocate - we only manipulate existing data
    println!("Performing operations that do not allocate...");
    #[expect(
        clippy::indexing_slicing,
        reason = "we know the vector has at least 2 elements"
    )]
    {
        data[0] = 10;
        data[1] = 20;
    }
    let sum: i32 = data.iter().sum();
    println!("Sum of modified data: {sum}");

    // The panic flag is still enabled, so the next allocation will panic
    // Let's disable it manually before doing more allocations
    println!("Disabling panic-on-next-allocation...");
    panic_on_next_alloc(false);

    // Now we can allocate again
    println!("Allocating more memory after disabling panic-on-next-allocation...");
    #[expect(clippy::useless_vec, reason = "example needs to show allocation")]
    let more_data = vec![6, 7, 8, 9, 10];
    println!(
        "Successfully allocated another vector with {} elements",
        more_data.len()
    );

    // Demonstrate the one-shot behavior
    println!("Demonstrating one-shot behavior - enabling and then allocating twice...");
    panic_on_next_alloc(true);

    // First allocation would panic (but we'll skip it in this safe example)
    // The second allocation would work because the flag auto-resets
    println!("Flag is enabled but we won't actually allocate to avoid panic");
    panic_on_next_alloc(false); // Disable to continue safely

    println!("Example completed successfully!");

    // Uncomment the following lines to see the panic in action:
    // panic_on_next_alloc(true);
    // let _this_will_panic = vec![1, 2, 3]; // This would panic
    // let _this_would_work = vec![4, 5, 6]; // This would work (flag auto-reset)
}
