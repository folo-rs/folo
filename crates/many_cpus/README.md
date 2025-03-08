Working on many-processor systems with 100+ logical processors can require you to pay extra
attention to the specifics of the hardware to make optimal use of available compute capacity
and extract the most performance out of the system.

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.

# Why should one care?

Modern operating systems try to distribute work fairly between all processors. Typical Rust
sync and async task runtimes like Rayon and Tokio likewise try to be efficient in occupying all
processors with work, even moving work between processors if one risks becoming idle. This is fine
but we can do better.

Taking direct control over the placement of work on specific processors can yield superior
performance by taking advantage of factors under the service author's control, which are not known
to general-purpose tasking runtimes:

1. A key insight we can use is that most service apps exist to process requests or execute jobs - each
   unit of work being done is related to a specific data set. We can ensure we only process the data
   associated with a specific HTTP/gRPC request on a single processor to ensure optimal data locality.
   This means the data related to the request is likely to be in the caches of that processor, speeding
   up all operations related to that request by avoiding expensive memory accesses.
1. Even when data is intentionally shared across processors (e.g. because one processor is not capable
   enough to do the work and parallelization is required), performance differences exist between
   different pairs of processors because different processors can be connected to different physical
   memory modules. Access to non-cached data is optimal when that data is in the same memory region
   as the current processor (i.e. on the physical memory modules directly wired to the current
   processor).

# How does this crate help?

The `many_cpus` crate provides mechanisms to schedule threads on specific processors and in specific
memory regions, ensuring that work assigned to those threads remains on the same hardware and that
data shared between threads is local to the same memory region, enabling you to achieve high data
locality and processor cache efficiency.

In addition to thread spawning, this crate enables app logic to observe what processor the current
thread is executing on and in which memory region this processor is located, even if the thread is
not bound to a specific processor. This can be a building block for efficiency improvements even
outside directly controlled work scheduling.

Other crates from the [Folo project](https://github.com/folo-rs/folo) build upon this hardware-
awareness functionality to provide higher-level primitives such as thread pools, work schedulers,
region-local cells and more.

# How to use this crate?

More details in the [crate documentation](https://docs.rs/many_cpus/).