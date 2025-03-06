# Folo

Mechanisms for high-performance hardware-aware programming in Rust.

The design tenets this project aims to satisfy are the following:

* In services, keep the processing of each request on a single processor to ensure both that data
  is locally cached for fast access and to avoid polluting caches of many processors with data of
  a single request.
* Be aware of [memory region boundaries](https://www.kernel.org/doc/html/v4.18/vm/numa.html)
  when scheduling work and placing data. Avoid moving data across these boundaries because it can
  be very slow.
* Use single-threaded logic without synchronization - even atomics and "lock-free" synchronization
  primitives are expensive compared to single-threaded logic. Whenever feasible, use `!Send` types
  to avoid accidental multithreading. Maintain separate mutable data sets per thread or memory
  region instead of maintaining global data sets.
* Use asynchronous logic in the app, in library code and when communicating with the operating
  system, ensuring that a thread is never blocked from doing useful work.

# Contents

This is an umbrella project that covers multiple largely independent crates:

| Crate                                             | Description                                                                              |
|---------------------------------------------------|------------------------------------------------------------------------------------------|
| [`many_cpus`](crates/many_cpus/README.md)         | Efficiently schedule work and inspect the hardware environment on many-processor systems |
| [`region_cached`](crates/region_cached/README.md) | Add a layer of cache between L3 and main memory                                          |
| [`region_local`](crates/region_local/README.md)   | Isolate variable storage per memory region, similar to `thread_local!`                   |

Some auxiliary crates are also published because the primary crates above require their
functionality. They only indirectly contribute to the Folo mission, so are listed separately:

| Crate                                 | Description                                              |
|---------------------------------------|----------------------------------------------------------|
| [`cpulist`](crates/cpulist/README.md) | Utilities for parsing and emitting Linux cpulist strings |

There are also some development-only crates in this repo, which are not published:

| Crate                                                               | Description                                                                        |
|---------------------------------------------------------------------|------------------------------------------------------------------------------------|
| [`benchmarks`](crates/benchmarks)                                   | Random pile of benchmarks to explore relevant scenarios and guide Folo development |
| [`many_cpus_benchmarking`](crates/many_cpus_benchmarking/README.md) | Criterion benchmark harness to easily compare different processor configurations   |

# Development environment setup

See [DEVELOPMENT.md](DEVELOPMENT.md).