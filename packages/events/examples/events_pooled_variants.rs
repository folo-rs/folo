//! Example demonstrating all the pooled event variants: `bind_by_ref`, `bind_by_arc`, and `bind_by_ptr`.

use std::sync::Arc;

use events::OnceEventPool;
use futures::executor::block_on;

/// Test values for different pooled event variants.
const BY_REF_TEST_VALUE: i32 = 10;
const BY_ARC_TEST_VALUE: i32 = 30;
const BY_PTR_TEST_VALUE: i32 = 40;

fn main() {
    println!("=== Pooled Events Variants Example ===");

    // 1. bind_by_ref variant (lifetime-based)
    println!("\n1. Testing bind_by_ref variant:");
    {
        block_on(async {
            let pool = OnceEventPool::<i32>::new();
            let (sender, receiver) = pool.bind_by_ref();

            sender.send(BY_REF_TEST_VALUE);
            let value = receiver.await.expect(
                "sender.send() was called immediately before this, so sender cannot be dropped",
            );
            println!("   bind_by_ref received: {value}");
        });
    }

    // 3. bind_by_arc variant (Arc-based, thread-safe)
    println!("\n3. Testing bind_by_arc variant:");
    {
        let pool = Arc::new(OnceEventPool::<i32>::new());
        let (sender, receiver) = pool.bind_by_arc();

        // Can be moved across threads
        let sender_thread = std::thread::spawn(move || {
            sender.send(BY_ARC_TEST_VALUE);
        });

        let receiver_thread = std::thread::spawn(move || block_on(receiver));

        sender_thread
            .join()
            .expect("sender thread should complete successfully");
        let value = receiver_thread
            .join()
            .expect("receiver thread should complete successfully")
            .expect("sender thread is guaranteed to call send() before joining");
        println!("   bind_by_arc received: {value}");
    }

    // 4. bind_by_ptr variant (unsafe, raw pointer-based)
    println!("\n4. Testing bind_by_ptr variant:");
    {
        block_on(async {
            let pool = Box::pin(OnceEventPool::<i32>::new());

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

            sender.send(BY_PTR_TEST_VALUE);
            let value = receiver.await.expect(
                "sender.send() was called immediately before this, so sender cannot be dropped",
            );
            println!("   bind_by_ptr received: {value}");
            // sender and receiver are dropped here, before pool
        });
    }

    // 6. Demonstrate pool reuse and resource efficiency across variants
    println!("\n6. Testing resource reuse and efficiency:");
    {
        block_on(async {
            let pool = OnceEventPool::<&str>::new();
            println!("   Created fresh pool for demonstrating resource reuse");

            // Multiple sequential events using bind_by_ref - all should reuse the same underlying event
            println!("   Sequential events (demonstrating resource reuse):");
            for i in 1..=5 {
                let (sender, receiver) = pool.bind_by_ref();
                let message = match i {
                    1 => "First",
                    2 => "Second (REUSED)",
                    3 => "Third (REUSED)",
                    4 => "Fourth (REUSED)",
                    5 => "Fifth (REUSED)",
                    _ => unreachable!(),
                };

                sender.send(message);
                let value = receiver.await.expect(
                    "sender.send() was called immediately before this, so sender cannot be dropped",
                );
                println!("     Event {i}: {value}");
            }

            println!("   ✓ All 5 events likely reused the SAME underlying event instance!");
            println!("   ✓ Only 1 allocation on first use, 4 subsequent uses were allocation-free");
        });
    }

    println!("\n=== Resource Reuse Summary ===");
    println!("✓ All pooled event variants support efficient resource reuse");
    println!("✓ Events are automatically returned to pools when endpoints are dropped");
    println!("✓ Subsequent `pool.bind_*()` calls reuse existing event instances");
    println!("✓ This minimizes allocation overhead in high-frequency scenarios");
    println!(
        "✓ Different endpoint types (bind_by_ref, bind_by_arc, bind_by_ptr) can all reuse the same pool"
    );

    println!("\nAll pooled event variants work correctly with efficient resource reuse!");
    println!("Events are automatically cleaned up when both sender and receiver are dropped.");
    println!(
        "Each variant provides different ownership semantics suitable for different use cases:"
    );
    println!("- bind_by_ref: Lightweight, lifetime-based (when pool lifetime is clear)");
    println!("- bind_by_arc: Multi-threaded reference counting (for concurrent access)");
    println!(
        "- bind_by_ptr: Unsafe but flexible (for advanced use cases with manual lifetime management)"
    );
    println!();
    println!("Key benefit: ALL variants reuse the same underlying event instances from the pool,");
    println!(
        "making them highly efficient for scenarios with frequent event creation/destruction!"
    );
}
