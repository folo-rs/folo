# Folo

Mechanisms for high-performance hardware-aware programming in Rust.

# What it gives you

![](doc/hardware.png)

To take advantage of hardware-awareness we must first gain that awareness.
[`many_cpus`][many_cpus] informs us about the nature of the system's processors and
[the arrangement of them in relation to main memory][numa], giving us control over what
specific logic runs on which specific processors. No longer do we simply `thread::spawn()`
blindly - now we spawn specific threads for specific processors or groups of processors.

[`many_cpus_benchmarking`][many_cpus_b] provides a benchmark harness to explore the effects of
how work and data are spread between processors, when applied to different algorithms. This
allows you to judge when it matters and by how much.

![](doc/region_cached.png)

[The ability to target our workloads to specific processors and specific memory regions unlocks new optimization opportunities.][structural_changes]
[`region_local`][region_local] and [`region_cached`][region_cached]
provide something like a layer of caching between the processor and main memory, ensuring that
even data sets that do not fit into processor caches still experience high data locality.

![](doc/linked.png)

Designing code for hardware-efficiency often benefits from a thread-isolated mindset, treating
each thread as its own universe. [`linked`][linked] provides valuable concepts, metaphors and
mechanisms to enable objects to present a unique face to each thread, acting as separate objects
on each thread while being connected through internal logic and only permitting opt-in transfers
across thread boundaries. These are the building blocks used to implement `region_local` and
`region_cached`.

Measuring effects of hardware-aware programming sometimes requires benchmarks to be multi-threaded,
which is not something you get out of the box with benchmark frameworks like [Criterion][criterion].
[`par_bench`][par_bench] extends Criterion with a simple harness for multithreaded benchmarking,
running your benchmark logic on a specific processor set obtained from `many_cpus`. It takes care
of all the dirty business involved in coordinating the threads and eliminating any test harness
overhead from the data.

```
Processor time statistics:

| Operation                   | Mean |
|-----------------------------|------|
| futures_oneshot_channel_mt  | 92ns |
| futures_oneshot_channel_st  | 76ns |
| local_once_event_managed    | 38ns |
| pooled_local_once_event_ptr | 27ns |
| pooled_local_once_event_rc  | 31ns |
| pooled_local_once_event_ref | 24ns |
```

When evaluating complex application logic, it can be important to take a holistic view - it does
not only matter how fast the benchmark logic runs but also how much energy (processor time) it
uses. Perhaps the code also runs logic on background threads or perhaps the code just blocks
some threads for a while on syscalls, costing wall clock time without costing processor time.
These are all factors that we must account for in complex scenarios. [`all_the_time`][all_the_time]
allows us to track the processor time spent by the process, in addition to the wall clock time.
It integrates well into Criterion and is natively supported by `par_bench`.

```
Allocation statistics:

| Operation                      | Mean bytes | Mean count |
|--------------------------------|------------|------------|
| futures_oneshot_channel        |        128 |          1 |
| local_once_event_managed       |         48 |          1 |
| pooled_local_once_event_ptr    |          0 |          0 |
| pooled_local_once_event_rc     |          0 |          0 |
| pooled_local_once_event_ref    |          0 |          0 |
```

Memory allocation is the root of all evil. The simplest and most effective way to make a typical
application faster is to eliminate memory allocations from it - this can often multiply performance
several times. Before we can eliminate, we need to measure. [`alloc_tracker`][alloc_tracker] gives
us the ability to measure exactly how much heap memory is allocated by a particular piece of code.
It integrates well into Criterion and is natively supported by `par_bench`.

Once we have knowledge of how much memory we are allocating, we can start making a difference. The
simplest way is to change the algorithms so no memory allocations are necessary. Often, that is
still not enough! The global Rust memory allocator (whichever one might be used) is general-purpose
and it pays a price in performance for that generality. If we are allocating a large number of
objects of specific sizes, we can benefit from special-purpose allocators that keep the memory
around for reuse, so the next allocation is simple and fast.

Another term for special-purpose allocators is object pools and [`infinity_pool`][infinity_pool]
offers several of them, from basic `Vec<T>` style pinned object collections to type-agnostic object
pools that can allocate any type of object. While the safe-API variants come with substantial
overheads compared to the unsafe-API variants, they both can surpass the efficiency of using the
global memory allocator under many conditions. Your mileage may vary - measure 100 times,
cut 10 times.

A surprising source of memory allocations in high-performance code can be signaling. We are used
to thinking of oneshot channels as cheap and efficient things and while this is true, they are
still built upon shared memory allocated from the heap. Every signaling channel you create is a
heap allocation and they can add up fast! [`events`][events] provides you with pooled signaling
channels that take advantage of `infinity_pool` to reuse memory allocations.

```
bagels_cooked_weight_grams: 2300; sum 744000; mean 323
value <=    0 [    0 ]: 
value <=  100 [    0 ]: 
value <=  200 [ 1300 ]: ∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎
value <=  300 [    0 ]: 
value <=  400 [    0 ]: 
value <=  500 [    0 ]: 
value <=  600 [ 1000 ]: ∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎
value <=  700 [    0 ]: 
value <=  800 [    0 ]: 
value <=  900 [    0 ]: 
value <= 1000 [    0 ]: 
value <= +inf [    0 ]: 
```

It is easy to think that performance and efficiency has been achieved once the benchmarks look good. 
Yet time makes fools of us all! Real-world data often shows surprising behaviors - where we thought
we would spawn 500 tasks, a surprise implementation detail from the HTTP stack may end up spawning
500 million! Benchmarks and belief is not enough. [`nm`][nm] provides a very high performance
and minimal metrics framework suitable for taking millions of measurements per second. Only with
the real data can we be assured that we achieve real performance.

Many attempts to instrument high-performance logic are self-defeating because few people expect
that time itself is slow. Measuring the time, that is! `Instant::now()` is a remarkably slow
operation - never use this in high-performance code, as merely capturing the timestamp can massively
degrade performance. Accurate timing information can only ever be measured for large batches of
iterations so that one measurement span covers 100+ milliseconds of work. For situations where you
can sacrifice precision but still want satisfactory performance, [`fast_time`][fast_time] provides
a clock that is much cheaper to query. It will not give you precise numbers but it is safe to
query tens of thousands of times per second.


# Extras

Auxiliary packages developed and published by this project:

* `cargo-detect-package` - cargo subcommand to detect which package is used based on a provided path and to run another subcommand on that package.
* `cpulist` - utilities for parsing and emitting Linux cpulist strings, used by `many_cpus`.
* `new_zealand` - [utilities for working with non-zero integers][nonzero].

Packages present in the repo but not relevant to a general audience:

* `benchmarks` - random pile of benchmarks to explore relevant scenarios and guide Folo development.
* `folo_ffi` - utilities for working with FFI logic; exists for internal use in Folo packages; no stable API surface.
* `folo_utils` - utilities for internal use in Folo packages; exists for internal use in Folo packages; no stable API surface.
* `testing` - private helpers for testing and examples in Folo packages.

Deprecated packages:

* `blind_pool` - deprecated, use `infinity_pool` which offers a similar API but is internally structured in a more maintainable manner.
* `opaque_pool` - deprecated, use `infinity_pool` which offers a similar API but is internally structured in a more maintainable manner.
* `pinned_pool` - deprecated, use `infinity_pool` which offers a similar API but is internally structured in a more maintainable manner.

[all_the_time]: packages/all_the_time/README.md
[alloc_tracker]: packages/alloc_tracker/README.md
[criterion]: https://bheisler.github.io/criterion.rs/book/criterion_rs.html
[events]: packages/events/README.md
[fast_time]: packages/fast_time/README.md
[infinity_pool]: packages/infinity_pool/README.md
[linked]: packages/linked/README.md
[many_cpus]: packages/many_cpus/README.md
[many_cpus_b]: packages/many_cpus_benchmarking/README.md
[nm]: packages/nm/README.md
[nonzero]: https://github.com/rust-lang/rfcs/pull/3786
[numa]: https://www.kernel.org/doc/html/v4.18/vm/numa.html
[par_bench]: packages/par_bench/README.md
[region_cached]: packages/region_cached/README.md
[region_local]: packages/region_local/README.md
[structural_changes]: https://sander.saares.eu/2025/03/31/structural-changes-for-48-throughput-in-a-rust-web-service/

# Development environment setup

See [DEVELOPMENT.md](DEVELOPMENT.md).

# Quality assurance

This project aims for high quality standards:

✅ **Comprehensive testing** - All packages tested with extensive unit tests, integration tests, and doctests  
✅ **Miri validation** - All packages pass strict Rust memory safety validation via Miri  
✅ **Mutation testing** - Code quality verified through comprehensive mutation testing with `cargo-mutants`  
✅ **High test coverage** - Test coverage measured and maintained via `cargo-llvm-cov`  
✅ **Zero warnings policy** - All code must compile without any compiler or Clippy warnings  
✅ **Extensive Clippy rules** - 100+ custom Clippy lint rules enforced across the workspace  
✅ **Cross-platform validation** - All code tested on both Windows and Linux platforms  
✅ **Automated CI/CD** - Continuous integration runs full validation suite on every commit  
✅ **API documentation** - Complete API documentation with inline examples for all public APIs  
✅ **Dependency auditing** - Regular security audits of all dependencies via `cargo-audit`  
✅ **Semver compliance** - API changes validated for semantic versioning compliance via `cargo-semver-checks`