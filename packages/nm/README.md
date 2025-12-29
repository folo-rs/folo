# nm - nanometer

Collect metrics about observed events with low overhead even in
highly multithreaded applications running on 100+ processors.

Using arbitrary development hardware, we measure between 2 and 20 nanoseconds per
observation, depending on how the event is configured. Benchmarks are included.

```rust
use nm::Event;
use std::time::Duration;

thread_local! {
    static PACKAGES_RECEIVED: Event = Event::builder()
        .name("packages_received")
        .build();
        
    static PACKAGE_SEND_DURATION_MS: Event = Event::builder()
        .name("package_send_duration_ms")
        .build();
}

// observe_once() observes an event with a nominal magnitude of 1
PACKAGES_RECEIVED.with(Event::observe_once);

// observe_millis() observes an event with a magnitude in milliseconds
let send_duration = Duration::from_millis(150);
PACKAGE_SEND_DURATION_MS.with(|e| e.observe_millis(send_duration));
```

# Collected metrics

For each defined event, the following metrics are collected:

* Count of observations (`u64`).
* Mean magnitude of observations (`i64`).
* (Optional) Histogram of magnitudes, with configurable bucket boundaries (`i64`).

## See also

More details in the [package documentation](https://docs.rs/nm/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.