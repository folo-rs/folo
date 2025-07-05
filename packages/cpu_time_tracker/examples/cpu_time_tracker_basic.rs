//! Simplified example demonstrating key `cpu_time_tracker` types working together.
//!
//! This example shows how to use the main types in the `cpu_time_tracker` crate:
//! - `Session`: Manages CPU time tracking state
//! - `Span`: Tracks CPU time over a time period  
//! - `Operation`: Calculates average CPU time across multiple spans
//!
//! Run with: `cargo run --example cpu_time_tracker_basic`
#![expect(
    clippy::arithmetic_side_effects,
    clippy::cast_sign_loss,
    clippy::unseparated_literal_suffix,
    reason = "this is example code that doesn't need production-level safety"
)]

use std::collections::HashMap;
use std::hint::black_box;

use cpu_time_tracker::Session;

fn main() {
    println!("=== CPU Time Tracking Example ===\n");

    // Create a tracking session - this enables CPU time monitoring
    let mut session = Session::new();
    println!("✓ Created tracking session\n");

    // Track string formatting - do more work to get measurable CPU time
    {
        let string_op = session.operation("string_formatting");
        {
            let _span = string_op.iterations(10).measure_thread();
            for i in 0..10 {
                let mut result = String::new();
                for j in 0..5000 {
                    use std::fmt::Write;
                    write!(
                        result,
                        "String number {i}-{j} with some content that is longer to force more work. "
                    )
                    .unwrap();
                }
                // Add some string processing to make it more CPU intensive
                let processed = result.chars().rev().collect::<String>();
                black_box(processed);
            }
        }
    }

    // Track hashmap creation - do more work to get measurable CPU time
    {
        let hashmap_op = session.operation("hashmap_creation");
        {
            let _span = hashmap_op.iterations(10).measure_thread();
            for i in 0..10 {
                let mut map = HashMap::new();
                for j in 0..1000 {
                    map.insert(format!("key{i}-{j}"), format!("value{i}-{j}"));
                }
                // Add some lookups to make it more CPU intensive
                for j in 0..1000 {
                    let key = format!("key{i}-{j}");
                    black_box(map.get(&key));
                }
                black_box(map);
            }
        }
    }

    // Track computation - do more work to get measurable CPU time
    {
        let computation_op = session.operation("computation");
        {
            let _span = computation_op.iterations(10).measure_thread();
            for i in 0..10 {
                let mut sum = 0u64;
                // More CPU-intensive computation with nested loops
                for j in 0..50000 {
                    for k in 0..10 {
                        sum += (j as u64 * k as u64 * i as u64) % 1000;
                        // Add some more computation to make it heavier
                        sum = sum.wrapping_mul(1103515245).wrapping_add(12345);
                    }
                }
                black_box(sum);
            }
        }
    }

    session.print_to_stdout();
    println!("\nSession automatically cleaned up when dropped.");
}
