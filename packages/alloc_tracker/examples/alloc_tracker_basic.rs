//! Demonstrating key `alloc_tracker` types working together.
//!
//! Run with: `cargo run --example comprehensive_tracking`

use std::collections::HashMap;
use std::hint::black_box;

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("=== Allocation Tracking Example ===\n");

    // Create a tracking session - this enables allocation monitoring
    let mut session = Session::new();
    println!("✓ Created tracking session\n");

    // Track string formatting
    {
        let string_op = session.operation("string_formatting");
        for i in 0..3 {
            let _span = string_op.measure_process();
            let s = format!("String number {i} with some content");
            black_box(s);
        }
    }

    // Track hashmap creation
    {
        let hashmap_op = session.operation("hashmap_creation");
        for _ in 0..3 {
            let _span = hashmap_op.measure_process();
            let mut map = HashMap::new();
            map.insert("key1", "value1");
            map.insert("key2", "value2");
            map.insert("key3", "value3");
            black_box(map);
        }
    }

    // Track vector allocation
    {
        let vector_op = session.operation("vector_allocation");
        for i in 0..3 {
            let _span = vector_op.measure_process();
            let vec = vec![i; 50]; // 50 elements each time
            black_box(vec);
        }
    }

    session.print_to_stdout();

    println!("\nSession automatically cleaned up when dropped.");
}
