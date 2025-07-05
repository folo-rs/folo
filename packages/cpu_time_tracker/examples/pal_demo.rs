//! Demonstration of the Platform Abstraction Layer (`PAL`) in `cpu_time_tracker`.
//!
//! This example shows how the `PAL` allows for controlled testing by using
//! fake platforms instead of relying on actual system calls.

use cpu_time_tracker::Session;

fn main() {
    println!("=== Platform Abstraction Layer Demo ===");
    
    // This is what users normally do - real platform measurement
    println!("✓ Using real platform (actual system calls):");
    let mut real_session = Session::new();
    let real_operation = real_session.operation("real_work");
    
    {
        let _span = real_operation.measure_thread();
        // Some actual work
        let mut sum = 0_u64;
        for i in 0..1000 {
            sum = sum.wrapping_add(i);
        }
        std::hint::black_box(sum);
    }
    
    println!("  Real CPU time recorded: {:?}", real_operation.total_cpu_time());
    
    // This is what happens internally in tests - fake platform with controlled values
    #[cfg(test)]
    println!("✓ Using fake platform (controlled values for testing):");
    
    #[cfg(test)]
    {
        use cpu_time_tracker::Session;
        // Note: FakePlatform and with_platform are not public APIs, 
        // they're internal testing tools
        println!("  (This would use fake platform in test code)");
        println!("  Fake platforms allow tests to have predictable CPU time values");
    }
    
    println!("✓ PAL enables reliable testing without depending on actual CPU load");
}
