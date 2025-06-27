[Criterion][1] benchmark harness designed to compare different modes of distributing work in a
many-processor system with multiple memory regions. This helps highlight the performance impact of
cross-memory-region data transfers, cross-processor data transfers and multi-threaded logic.

More details in the [crate documentation](https://docs.rs/many_cpus_benchmarking/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.

[1]: https://bheisler.github.io/criterion.rs/book/index.html