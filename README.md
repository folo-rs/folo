# Folo

Mechanisms for high-performance hardware-aware programming in Rust.

# Contents

This is an umbrella project that covers some largely independent crates:

| Crate                                             | Description                                                                              |
|---------------------------------------------------|------------------------------------------------------------------------------------------|
| [`many_cpus`](crates/many_cpus/README.md)         | Efficiently schedule work and inspect the hardware environment on many-processor systems |
| [`region_cached`](crates/region_cached/README.md) | Add a layer of cache between L3 and main memory                                          |
| [`region_local`](crates/region_local/README.md)   | Isolate variable storage per memory region, similar to `thread_local!`                   |

# Development environment setup

See [DEVELOPMENT.md](DEVELOPMENT.md).