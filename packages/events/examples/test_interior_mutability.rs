//! Test to verify interior mutability is working correctly

use events::once::{EventPool, LocalEventPool};
use futures::executor::block_on;
use std::sync::Arc;

fn main() {
    println!("Testing interior mutability...");
    
    // Test EventPool with shared references using Arc variants for thread safety
    let pool = Arc::new(EventPool::<i32>::new());
    
    // Create multiple senders from shared references (not &mut!)
    let (sender1, receiver1) = pool.by_arc(&pool);  // Thread-safe Arc variant
    let (sender2, receiver2) = pool.by_arc(&pool);  // Thread-safe Arc variant
    
    // Test that we can use these in threads
    let handle1 = std::thread::spawn(move || {
        sender1.send(42);
    });
    
    let handle2 = std::thread::spawn(move || {
        sender2.send(100);
    });
    
    handle1.join().unwrap();
    handle2.join().unwrap();
    
    // Receive the values
    let val1 = block_on(receiver1.recv_async());
    let val2 = block_on(receiver2.recv_async());
    
    println!("Received: {val1} and {val2}");
    
    // Test LocalEventPool with shared references (single-threaded)
    let local_pool = LocalEventPool::<String>::new();
    
    // Create multiple senders from shared references - this is now possible!
    let (local_sender1, local_receiver1) = local_pool.by_ref();
    let (local_sender2, local_receiver2) = local_pool.by_ref();
    
    local_sender1.send("Hello".to_string());
    local_sender2.send("World".to_string());
    
    let msg1 = block_on(local_receiver1);
    let msg2 = block_on(local_receiver2);
    
    println!("Local received: {msg1} and {msg2}");
    
    // Demonstrate that pools can be shared without exclusive access
    let shared_local_pool = &local_pool;  // Just a shared reference
    let (sender3, receiver3) = shared_local_pool.by_ref();  // Works with &self!
    sender3.send("Shared access works!".to_string());
    let msg3 = block_on(receiver3);
    println!("Shared access result: {msg3}");
    
    println!("Interior mutability test completed successfully!");
}
