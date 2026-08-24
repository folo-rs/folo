# nm - nanometer

Collect metrics about observed events with low overhead even in
highly multithreaded applications running on more than 100 logical processors.

Included benchmarks have measured between 2 and 20 nanoseconds per observation,
depending on event configuration. Results vary by hardware.

```rust
use std::time::Duration;

use nm::{Event, Report};

thread_local! {
    static PACKAGES_RECEIVED: Event = Event::builder()
        .name("packages_received")
        .build();

    static PACKAGE_SEND_DURATION_MS: Event = Event::builder()
        .name("package_send_duration_ms")
        .build();
}

fn main() {
    PACKAGES_RECEIVED.with(Event::observe_once);

    // A nonzero sample makes the duration magnitude visible in the rendered report.
    let send_duration = Duration::from_millis(150);
    PACKAGE_SEND_DURATION_MS.with(|event| event.observe_millis(send_duration));

    let report = Report::collect();
    print!("{report}");
}
```

# Collected metrics

Each defined event collects:

* Count of observations (`u64`).
* Mean magnitude of observations (`i64`).
* An optional histogram of magnitudes with configurable bucket boundaries (`[i64]`).

## See also

The [package documentation](https://docs.rs/nm/) provides the complete API walkthrough.
The [design](docs/design.md) explains the behavioral contract and the
[implementation guide](docs/implementation.md) describes the `nm` package family architecture.

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.