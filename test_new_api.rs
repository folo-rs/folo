// Test script to verify the new Report API functionality
use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("Testing new Report API functionality...\n");

    // Test all_the_time
    println!("=== Testing all_the_time Report API ===");
    let time_session = TimeSession::new();
    
    {
        let op1 = time_session.operation("fast_work");
        let _span1 = op1.iterations(100).measure_thread();
        for _ in 0..100 {
            std::hint::black_box(42 * 2);
        }
    }
    
    {
        let op2 = time_session.operation("slow_work");
        let _span2 = op2.iterations(10).measure_thread();
        for _ in 0..10 {
            // Simulate more expensive work
            let mut sum = 0;
            for i in 0..1000 {
                sum += i;
            }
            std::hint::black_box(sum);
        }
    }

    let time_report = time_session.to_report();
    
    println!("Operations found in time report:");
    for (name, op) in time_report.operations() {
        println!("  {}: {} iterations, mean time: {:?}, total time: {:?}", 
                 name, op.total_iterations(), op.mean(), op.total_processor_time());
    }

    // Test alloc_tracker
    println!("\n=== Testing alloc_tracker Report API ===");
    let alloc_session = AllocSession::new();
    
    {
        let op1 = alloc_session.operation("small_allocs");
        let _span1 = op1.iterations(5).measure_process();
        for _ in 0..5 {
            let _data = vec![1u8; 100]; // 100 bytes each
        }
    }
    
    {
        let op2 = alloc_session.operation("large_allocs");
        let _span2 = op2.iterations(2).measure_process();
        for _ in 0..2 {
            let _data = vec![1u8; 1000]; // 1000 bytes each
        }
    }

    let alloc_report = alloc_session.to_report();
    
    println!("Operations found in allocation report:");
    for (name, op) in alloc_report.operations() {
        println!("  {}: {} iterations, mean allocation: {} bytes, total allocation: {} bytes", 
                 name, op.total_iterations(), op.mean(), op.total_bytes_allocated());
    }

    println!("\n✓ All API tests completed successfully!");
}
