On many-processor systems with multiple memory regions, there is an extra cost associated with
accessing data in physical memory modules that are in a different memory region than the current
processor:

* Cross-memory-region loads have higher latency (e.g. 100 ns local versus 200 ns remote).
* Cross-memory-region loads have lower throughput (e.g. 50 Gbps local versus 10 Gbps remote).

This crate provides the capability to cache frequently accessed shared data sets in the local memory
region, speeding up reads when the data is not already in the local processor caches. You can think
of it as an extra level of caching between L3 processor caches and main memory.

More details in the [crate documentation](https://docs.rs/region_cached/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.