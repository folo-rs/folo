# nm - nanometer

Collect metrics about observed events with minimal collection overhead even in
highly multithreaded applications running on 100+ processors.

# Collected metrics

For each defined event, the following metrics are collected:

* Count of observations (`u64`).
* Mean magnitude of observations (`i64`).
* (Optional) Histogram of magnitudes, with configurable bucket boundaries (`i64`).

More details in the [crate documentation](https://docs.rs/nm/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.