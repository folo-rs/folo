# fast_time

An efficient low-precision timestamp source suitable for high-frequency querying.

## Overview

The `fast_time` crate provides a `Clock` that can efficiently capture timestamps with low overhead, making it ideal for high-frequency querying scenarios such as metrics collection and logging. The clock prioritizes efficiency over absolute precision.

## Key Features

- **Low overhead**: Optimized for rapid, repeated timestamp capture
- **Monotonic timestamps**: Guarantees that timestamps always increase
- **Cross-platform**: Works on both Windows and Linux
- **Standard library compatibility**: Converts to/from `std::time::Instant`

## Trade-offs

- May lag behind wall-clock time by a few milliseconds
- May not reflect explicit wall clock adjustments (e.g., NTP synchronization)
- Optimized for frequency over precision

## Example

```rust
use fast_time::Clock;

// Create a clock for efficient timestamp capture
let clock = Clock::new();

// Capture timestamps rapidly
let start = clock.now();

// Simulate some work
simulate_work();

let elapsed = start.elapsed(&clock);
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

fn simulate_work() {
    std::thread::sleep(std::time::Duration::from_millis(10));
}
```

More details in the [package documentation](https://docs.rs/fast_time/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.