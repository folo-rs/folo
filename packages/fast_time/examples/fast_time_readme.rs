//! Example code for the `README.md` file.
//!
//! This contains the same code that appears in the `fast_time` package `README.md`.

fn main() {
    use fast_time::Clock;

    // Create a clock for efficient timestamp capture
    let mut clock = Clock::new();

    // Capture timestamps rapidly
    let start = clock.now();

    // Simulate some work
    simulate_work();

    let elapsed = start.elapsed(&mut clock);
    println!("Work completed in: {elapsed:?}");

    // High-frequency timestamp collection
    let mut timestamps = Vec::new();
    for _ in 0..1000 {
        timestamps.push(clock.now());
    }

    // Calculate total collection time
    let total_time = timestamps
        .last()
        .unwrap()
        .saturating_duration_since(*timestamps.first().unwrap());

    println!(
        "Collected {} timestamps in {total_time:?}",
        timestamps.len()
    );

    // Convert to std::time::Instant for interoperability
    let fast_instant = clock.now();
    let std_instant: std::time::Instant = fast_instant.into();
    println!("Converted instant: {std_instant:?}");
}

fn simulate_work() {
    std::thread::sleep(std::time::Duration::from_millis(10));
}
