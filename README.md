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

This is an umbrella project that covers multiple largely independent packages:

| Crate                                                                 | Description                                                                                                                                 |
|-----------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------|
| [`blind_pool`](packages/blind_pool/README.md)                         | A pinned object pool that can store objects of any type                                                                                     |
| [`events`](packages/events/README.md)                                 | (DRAFT) High-performance signaling primitives for concurrent environments                                                                   |
| [`linked`](packages/linked/README.md) + siblings                      | Create families of linked objects that can collaborate across threads while being internally single-threaded                                |
| [`many_cpus`](packages/many_cpus/README.md)                           | Efficiently schedule work and inspect the hardware environment on many-processor systems                                                    |
| [`many_cpus_benchmarking`](packages/many_cpus_benchmarking/README.md) | Criterion benchmark harness to easily compare different processor configurations                                                            |
| [`nm`](packages/nm/README.md)                                         | Collect metrics about observed events with minimal collection overhead even in highly multithreaded applications running on 100+ processors |
| [`opaque_pool`](packages/opaque_pool/README.md)                       | A pinned object pool that contains any type of object as long as it has a compatible memory layout                                          |
| [`pinned_pool`](packages/pinned_pool/README.md)                       | An object pool that guarantees pinning of its items                                                                                         |
| [`region_cached`](packages/region_cached/README.md)                   | Add a layer of cache between L3 and main memory                                                                                             |
| [`region_local`](packages/region_local/README.md)                     | Isolate variable storage per memory region, similar to `thread_local_rc!`                                                                   |

Some auxiliary packages are also published because the primary packages above require their
functionality. They only indirectly contribute to the Folo mission, so are listed separately:

| Crate                                           | Description                                                                                                  |
|-------------------------------------------------|--------------------------------------------------------------------------------------------------------------|
| [`cpulist`](packages/cpulist/README.md)         | Utilities for parsing and emitting Linux cpulist strings                                                     |
| [`folo_ffi`](packages/folo_ffi/README.md)       | Utilities for working with FFI logic; exists for internal use in Folo packages; no stable API surface        |
| [`folo_utils`](packages/folo_utils/README.md)   | Utilities for internal use in Folo packages; exists for internal use in Folo packages; no stable API surface |
| [`new_zealand`](packages/new_zealand/README.md) | Utilities for working with non-zero integers                                                                 |

There are also some development-only packages in this repo, which are not published:

| Crate                                         | Description                                                                        |
|-----------------------------------------------|------------------------------------------------------------------------------------|
| [`benchmark_utils`](packages/benchmark_utils) | Common benchmarking logic used across the packages in this project                 |
| [`benchmarks`](packages/benchmarks)           | Random pile of benchmarks to explore relevant scenarios and guide Folo development |
| [`testing`](packages/testing)                 | Private helpers for testing and examples in Folo packages                          |

# Development environment setup

See [DEVELOPMENT.md](DEVELOPMENT.md).