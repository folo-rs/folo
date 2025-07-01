//! Example demonstrating pooled events with automatic resource management and reuse.

use events::OnceEventPool;
use futures::executor::block_on;

/// First test value for demonstrating pooled event usage.
const FIRST_VALUE: i32 = 42;

/// Second test value for demonstrating resource reuse.
const SECOND_VALUE: i32 = 100;

/// Third test value for demonstrating multiple sequential usage.
const THIRD_VALUE: i32 = 200;

/// Fourth test value for demonstrating efficient resource recycling.
const FOURTH_VALUE: i32 = 300;

fn main() {
    println!("=== Pooled Events Example: Resource Reuse Demonstration ===");
    println!();

    block_on(async {
        // Create a pool that will efficiently reuse event instances to minimize allocations.
        // The pool automatically manages the lifecycle of events - creating them when needed
        // and returning them to the pool for reuse when both sender and receiver are dropped.
        let pool = OnceEventPool::<i32>::new();
        println!("Created event pool for efficient resource management");
        println!();

        // === First Event Usage ===
        println!("1. First event usage:");
        {
            let (sender, receiver) = pool.bind_by_ref();
            println!("   - Obtained event from pool (may create new event)");

            sender.send(FIRST_VALUE);
            println!("   - Sent value: {FIRST_VALUE}");

            let received = receiver.await.expect(
                "sender.send() was called immediately before this, so sender cannot be dropped",
            );
            println!("   - Received value: {received}");

            // When this scope ends, sender and receiver are dropped automatically,
            // and the event is returned to the pool for reuse
        }
        println!("   - Event returned to pool when sender/receiver dropped");
        println!();

        // === Second Event Usage (Reuses Resources) ===
        println!("2. Second event usage (REUSING same underlying event):");
        {
            let (sender, receiver) = pool.bind_by_ref();
            println!("   - Obtained event from pool (REUSED previous event - no allocation!)");

            sender.send(SECOND_VALUE);
            println!("   - Sent value: {SECOND_VALUE}");

            let received = receiver.await.expect(
                "sender.send() was called immediately before this, so sender cannot be dropped",
            );
            println!("   - Received value: {received}");
        }
        println!("   - Event returned to pool again for future reuse");
        println!();

        // === Third Event Usage (Continues Resource Reuse) ===
        println!("3. Third event usage (REUSING same event again):");
        {
            let (sender, receiver) = pool.bind_by_ref();
            println!("   - Obtained event from pool (REUSED same event instance)");

            sender.send(THIRD_VALUE);
            println!("   - Sent value: {THIRD_VALUE}");

            let received = receiver.await.expect(
                "sender.send() was called immediately before this, so sender cannot be dropped",
            );
            println!("   - Received value: {received}");
        }
        println!("   - Event returned to pool once more");
        println!();

        // === Fourth Event Usage (Demonstrates Continued Efficiency) ===
        println!("4. Fourth event usage (EFFICIENT resource recycling):");
        {
            let (sender, receiver) = pool.bind_by_ref();
            println!("   - Obtained event from pool (SAME event, reset and ready for use)");

            sender.send(FOURTH_VALUE);
            println!("   - Sent value: {FOURTH_VALUE}");

            let received = receiver.await.expect(
                "sender.send() was called immediately before this, so sender cannot be dropped",
            );
            println!("   - Received value: {received}");
        }
        println!("   - Event returned to pool for potential future use");
        println!();

        println!("=== Resource Reuse Summary ===");
        println!("✓ All four events likely reused the SAME underlying event instance");
        println!("✓ Only ONE event allocation occurred (during first usage)");
        println!("✓ Three subsequent usages were FREE from allocation overhead");
        println!("✓ Pool automatically manages event lifecycle and reuse");
        println!("✓ No manual resource management required by user code");
        println!();
        println!("This demonstrates the efficiency of pooled events for high-frequency scenarios!");
    });
}
