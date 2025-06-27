Utilities for parsing and emitting strings in the the `cpulist` format often used by Linux
utilities that work with processor IDs, memory region IDs and similar numeric hardware
identifiers.

Example cpulist string: `0,1,2-4,5-9:2,6-10:2`

More details in the [package documentation](https://docs.rs/cpulist/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.