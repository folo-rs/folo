use cpu_time::*;

pub fn test_cpu_time_api() {
    let start = ProcessTime::now();
    
    // Simulate some CPU work
    let mut sum = 0;
    for i in 0..1000000 {
        sum += i;
    }
    
    let duration = start.elapsed();
    println!("CPU time: {:?}", duration);
    
    // Test thread time
    let thread_start = ThreadTime::now();
    let thread_duration = thread_start.elapsed();
    println!("Thread CPU time: {:?}", thread_duration);
}
