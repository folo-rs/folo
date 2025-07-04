use cpu_time::{ProcessTime, ThreadTime};
use std::time::Duration;

fn main() {
    // Test ProcessTime
    let start = ProcessTime::now();
    let mut sum = 0u64;
    for i in 0..10000 {
        sum += i;
    }
    let duration = start.elapsed();
    println!("ProcessTime duration: {:?}", duration);
    
    // Test ThreadTime
    let thread_start = ThreadTime::now();
    let mut sum2 = 0u64;
    for i in 0..10000 {
        sum2 += i;
    }
    let thread_duration = thread_start.elapsed();
    println!("ThreadTime duration: {:?}", thread_duration);
    
    // Test if they return Duration
    let _: Duration = duration;
    let _: Duration = thread_duration;
    
    println!("Sum: {}, Sum2: {}", sum, sum2);
}
