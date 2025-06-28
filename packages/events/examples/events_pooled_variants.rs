//! Example demonstrating all the pooled event variants: `by_ref`, `by_rc`, `by_arc`, and `by_ptr`.

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use events::once::EventPool;

fn main() {
    println!("=== Pooled Events Variants Example ===");

    // 1. by_ref variant (lifetime-based)
    println!("\n1. Testing by_ref variant:");
    {
        let mut pool = EventPool::<i32>::new();
        let (sender, receiver) = pool.by_ref();

        sender.send(10);
        let value = receiver.recv();
        println!("   by_ref received: {value}");
    }

    // 2. by_rc variant (Rc-based, single-threaded)
    println!("\n2. Testing by_rc variant:");
    {
        let pool = Rc::new(RefCell::new(EventPool::<i32>::new()));
        let (sender, receiver) = pool.borrow_mut().by_rc(&pool);

        sender.send(20);
        let value = receiver.recv();
        println!("   by_rc received: {value}");
    }

    // 3. by_arc variant (Arc-based, thread-safe)
    println!("\n3. Testing by_arc variant:");
    {
        let pool = Arc::new(Mutex::new(EventPool::<i32>::new()));
        let (sender, receiver) = pool.lock().unwrap().by_arc(&pool);

        // Can be moved across threads
        let sender_thread = std::thread::spawn(move || {
            sender.send(30);
        });

        let receiver_thread = std::thread::spawn(move || receiver.recv());

        sender_thread.join().unwrap();
        let value = receiver_thread.join().unwrap();
        println!("   by_arc received: {value}");
    }

    // 4. by_ptr variant (unsafe, raw pointer-based)
    println!("\n4. Testing by_ptr variant:");
    {
        let mut pool = EventPool::<i32>::new();
        let pinned_pool = Pin::new(&mut pool);

        // SAFETY: We ensure the pool outlives the sender and receiver
        let (sender, receiver) = unsafe { pinned_pool.by_ptr() };

        sender.send(40);
        let value = receiver.recv();
        println!("   by_ptr received: {value}");
        // sender and receiver are dropped here, before pool
    }

    // 5. Async example with by_arc
    println!("\n5. Testing by_arc with async:");
    {
        let pool = Arc::new(Mutex::new(EventPool::<String>::new()));
        let (sender, receiver) = pool.lock().unwrap().by_arc(&pool);

        // Spawn async task to send
        let send_task = async move {
            sender.send("Hello async!".to_string());
        };

        // Spawn async task to receive
        let recv_task = async move { receiver.recv_async().await };

        // Run both tasks
        futures::executor::block_on(async {
            send_task.await;
            let value = recv_task.await;
            println!("   by_arc async received: {value}");
        });
    }

    // 6. Demonstrate pool reuse across variants
    println!("\n6. Testing pool reuse:");
    {
        let mut pool = EventPool::<&str>::new();

        // First event using by_ref
        let (sender1, receiver1) = pool.by_ref();
        sender1.send("First");
        let value1 = receiver1.recv();
        println!("   First event: {value1}");

        // Second event using by_ref again (different event, same pool)
        let (sender2, receiver2) = pool.by_ref();
        sender2.send("Second");
        let value2 = receiver2.recv();
        println!("   Second event: {value2}");
    }

    println!("\nAll pooled event variants work correctly!");
    println!("Events are automatically cleaned up when both sender and receiver are dropped.");
    println!(
        "Each variant provides different ownership semantics suitable for different use cases:"
    );
    println!("- by_ref: Lightweight, lifetime-based (when pool lifetime is clear)");
    println!("- by_rc: Single-threaded reference counting (when pool needs to be shared)");
    println!("- by_arc: Multi-threaded reference counting (for concurrent access)");
    println!(
        "- by_ptr: Unsafe but flexible (for advanced use cases with manual lifetime management)"
    );
}
