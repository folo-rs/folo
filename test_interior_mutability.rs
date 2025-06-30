//! Test to verify interior mutability is working correctly

use events::once::{EventPool, LocalEventPool};
use std::rc::Rc;
use std::sync::Arc;

fn main() {
    // Test that EventPool can be used with only shared references
    let pool = Arc::new(EventPool::<i32>::new());
    
    // This should work now - no &mut required!
    let (sender1, receiver1) = pool.by_ref();
    let (sender2, receiver2) = pool.by_arc(&pool);
    
    std::thread::spawn(move || {
        sender1.send(42);
    });
    
    std::thread::spawn(move || {
        sender2.send(100);
    });
    
    let result1 = futures::executor::block_on(receiver1.recv_async());
    let result2 = futures::executor::block_on(receiver2.recv_async());
    
    println!("EventPool results: {} and {}", result1, result2);
    
    // Test LocalEventPool with only shared references
    let local_pool = Rc::new(LocalEventPool::<String>::new());
    
    // This should also work now - no &mut required!
    let (sender3, receiver3) = local_pool.by_ref();
    let (sender4, receiver4) = local_pool.by_rc(&local_pool);
    
    sender3.send("Hello".to_string());
    sender4.send("World".to_string());
    
    let result3 = futures::executor::block_on(receiver3);
    let result4 = futures::executor::block_on(receiver4);
    
    println!("LocalEventPool results: {} and {}", result3, result4);
    
    println!("Interior mutability test completed successfully!");
}
