An efficient low-precision timestamp source suitable for high-frequency querying.

The `fast_time` crate provides a `Clock` that can efficiently capture timestamps with low overhead,
making it ideal for high-frequency querying scenarios such as metrics collection and logging.
The clock prioritizes efficiency over precision and provides monotonic timestamps.

```rust
use fast_time::Clock;

// Create a clock for efficient timestamp capture
let mut clock = Clock::new();

// Capture timestamps rapidly
let start = clock.now();

// Simulate some work
std::thread::sleep(std::time::Duration::from_millis(10));

let elapsed = start.elapsed(&mut clock);
println!("Work completed in: {elapsed:?}");

// High-frequency timestamp collection
let mut timestamps = Vec::new();
for _ in 0..1000 {
    timestamps.push(clock.now());
}

// Convert to std::time::Instant for interoperability
let fast_instant = clock.now();
let std_instant: std::time::Instant = fast_instant.into();
```

## See also

More details in the [package documentation](https://docs.rs/fast_time/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.