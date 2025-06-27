# nm - nanometer

Collect metrics about observed events with low overhead even in
highly multithreaded applications running on 100+ processors.

Using arbitrary development hardware, we measure between 2 and 20 nanoseconds per
observation, depending on how the event is configured. Benchmarks are included.

# Collected metrics

For each defined event, the following metrics are collected:

* Count of observations (`u64`).
* Mean magnitude of observations (`i64`).
* (Optional) Histogram of magnitudes, with configurable bucket boundaries (`i64`).

More details in the [crate documentation](https://docs.rs/nm/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.