# Folo

Mechanisms for high-performance hardware-aware programming in Rust.

# Getting started

Folo is a collection of libraries and command-line tools. Choose the packages you need;
you do not need to adopt the whole workspace or clone this repository to use them.
Follow the package links below for usage examples and documentation.

Add a library to your Rust project with Cargo, for example:

```text
cargo add many_cpus
```

Install a command-line tool separately:

```text
cargo install cargo-bench-history
```

If you use [`cargo-binstall`][cargo_binstall], `cargo binstall cargo-bench-history`
downloads a prebuilt binary on supported targets, falling back to a source build elsewhere.
Check the tool's README for platform requirements; in particular, [`dure`][dure] runs only
on Windows.

To contribute to Folo itself, see [DEVELOPMENT.md](DEVELOPMENT.md).

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

[`vicinal`][vicinal] allows you to schedule synchronous tasks on the same processor, providing
an easy way to preserve data locality for work that must be detached from the primary application
threads (e.g. because it is blocking code not suitable for an async application thread).

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

| Operation                   | Bytes/iter | Allocations/iter | Peak bytes |
|-----------------------------|------------|------------------|------------|
| futures_oneshot_channel     |        128 |                1 |        128 |
| local_once_event_managed    |         48 |                1 |         48 |
| pooled_local_once_event_ptr |          0 |                0 |          0 |
| pooled_local_once_event_rc  |          0 |                0 |          0 |
| pooled_local_once_event_ref |          0 |                0 |          0 |
```

Memory allocation is the root of all evil. The simplest and most effective way to make a typical
application faster is to eliminate memory allocations from it - this can often multiply performance
several times. Before we can eliminate, we need to measure. [`alloc_tracker`][alloc_tracker] gives
us the ability to measure exactly how much heap memory is allocated by a particular piece of code.
It integrates well into Criterion and is natively supported by `par_bench`.

Once we have knowledge of how much memory we are allocating, we can start making a difference. The
simplest way is to change the algorithms so no memory allocations are necessary but sometimes that
is impractical. Nevertheless, the global Rust memory allocator (whichever one might be used) is a
general-purpose mechanism and it pays a price in performance for that generality. If we are
allocating a large number of objects of specific sizes, we can benefit from special-purpose
allocators that keep the memory around for reuse, so the next allocation is simple and fast.

While allocator APIs are still an unstable Rust feature, object pools provide stable alternatives.
[`plurality`][plurality] offers `Pool<T>` for one concrete type and `MultiPool` for heterogeneous
types, retaining storage for per-object slot reuse. [`infinity_pool`][infinity_pool] receives
maintenance only and remains available where its raw manual-lifetime handles or macro-generated
trait-object casting are required. Special-purpose pools can surpass the efficiency of the global
memory allocator under many conditions. Your mileage may vary - measure 100 times, cut 10 times.

A surprising source of memory allocations in high-performance code can be signaling. We are used
to thinking of oneshot channels as cheap and efficient things and while this is true, they are
still built upon shared memory allocated from the heap. Every signaling channel you create is a
heap allocation and they can add up fast! [`events_once`][events_once] provides you with pooled
signaling channels that reuse memory allocations, as well as providing single-threaded and
unsafe-code-managed events for lower overhead in specialized scenarios.

When signaling must be reused rather than consumed once, [`events`][events] provides
manual-reset events that release all awaiters until reset and auto-reset events that
wake individual awaiters. Both offer thread-safe and single-threaded variants.
For coordinating groups of futures, [`future_deque`][future_deque] provides deque
collections with explicit control over polling order and result retrieval, with variants
for thread-mobile and single-threaded use.

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
the real data can we be assured that we achieve real performance. [`nm_otel`][nm_otel] bridges
nm metrics to OpenTelemetry for export to any compatible backend.

Many attempts to instrument high-performance logic are self-defeating because few people expect
that time itself is slow. Measuring the time, that is! `Instant::now()` is a remarkably slow
operation - never use this in high-performance code, as merely capturing the timestamp can massively
degrade performance. Accurate timing information can only ever be measured for large batches of
iterations so that one measurement span covers 100+ milliseconds of work. For situations where you
can sacrifice precision but still want satisfactory performance, [`fast_time`][fast_time] provides
a clock that is much cheaper to query. It will not give you precise numbers but it is safe to
query tens of thousands of times per second.

A single benchmark run is only a snapshot. The numbers that matter emerge over time: a hot
path that slowly regresses across dozens of commits, or a "harmless" refactor that doubles the
allocation count. [`cargo-bench-history`][cargo_bench_history] maintains a long-lived history of
your benchmark results - from [Criterion][criterion], [`all_the_time`][all_the_time],
[`alloc_tracker`][alloc_tracker] and [Callgrind][gungraun] - and analyzes it to detect
regressions and improvements across commits, so a
performance change cannot slip by unnoticed between one release and the next.


# Extras

Auxiliary packages developed and published by this project:

* [`awaiter_set`][awaiter_set] - zero-allocation awaiter tracking for async synchronization primitives.
* [`cargo-detect-package`][cargo_detect_package] - cargo subcommand to detect which package is used based on a provided path and to run another subcommand on that package.
* [`cargo-freeze-deps`][cargo_freeze_deps] - cargo subcommand that freezes floating dependency versions in a `Cargo.toml` to their resolved literal values.
* [`cargo-release-plan`][cargo_release_plan] - cargo subcommand that detects changes to released package content and prepares version increment plans with their dependency effects.
* [`cpulist`][cpulist] - utilities for parsing and emitting Linux cpulist strings, used by `many_cpus`.
* [`dure`][dure] - detachable Windows console sessions that keep running after the launching terminal closes and can be reattached from another terminal.
* [`new_zealand`][new_zealand] - utilities for working with non-zero integers.

The workspace also contains internal utilities, benchmark and test harnesses, and
implementation-only crates. These are not intended as general-purpose dependencies, even
when published for distribution. Use the public packages introduced above rather than
depending directly on their implementation crates.

[all_the_time]: packages/all_the_time/README.md
[alloc_tracker]: packages/alloc_tracker/README.md
[awaiter_set]: packages/awaiter_set/README.md
[cargo_bench_history]: packages/cargo-bench-history/README.md
[cargo_binstall]: https://github.com/cargo-bins/cargo-binstall
[cargo_detect_package]: packages/cargo-detect-package/README.md
[cargo_freeze_deps]: packages/cargo-freeze-deps/README.md
[cargo_release_plan]: packages/cargo-release-plan/README.md
[cpulist]: packages/cpulist/README.md
[criterion]: https://bheisler.github.io/criterion.rs/book/criterion_rs.html
[dure]: packages/dure/README.md
[events]: packages/events/README.md
[events_once]: packages/events_once/README.md
[fast_time]: packages/fast_time/README.md
[future_deque]: packages/future_deque/README.md
[gungraun]: https://crates.io/crates/gungraun
[infinity_pool]: packages/infinity_pool/README.md
[linked]: packages/linked/README.md
[many_cpus]: packages/many_cpus/README.md
[many_cpus_b]: packages/many_cpus_benchmarking/README.md
[new_zealand]: packages/new_zealand/README.md
[nm]: packages/nm/README.md
[nm_otel]: packages/nm_otel/README.md
[numa]: https://www.kernel.org/doc/html/v4.18/vm/numa.html
[par_bench]: packages/par_bench/README.md
[plurality]: https://crates.io/crates/plurality
[region_cached]: packages/region_cached/README.md
[region_local]: packages/region_local/README.md
[structural_changes]: https://sander.saares.eu/2025/03/31/structural-changes-for-48-throughput-in-a-rust-web-service/
[vicinal]: packages/vicinal/README.md

# Development environment setup

See [DEVELOPMENT.md](DEVELOPMENT.md).

# Quality assurance

**Standard validation** runs shallow checks on pull requests, merge-queue entries and
pushes to `main`. Pull requests and merge-queue entries select affected packages and
tooling checks; pushes to `main` run the full shallow set.

**Deep validation** runs the full deep suite against merged `main` nightly and on manual
dispatch. It covers Miri, many-seed Miri, mutation testing, careful checking, release
builds, example execution, dependency default-feature policy, feature combinations,
unused dependencies, and ARM64 tests and benchmark smoke checks.

Checks apply according to package, tool and platform support. Our quality practices include:

* ✅ **Behavioral testing** - Unit tests, integration tests, doctests and example execution
* ✅ **Memory safety checks** - Miri, including many-seed runs, and `cargo-careful` exercise supported tests
* ✅ **Mutation testing** - `cargo-mutants` checks whether tests detect changes to program behavior
* ✅ **Coverage requirements** - `cargo-llvm-cov` measures coverage, with Codecov targets for both the project and changed lines
* ✅ **Wall-clock benchmarks** - [Criterion][criterion] benchmarks measure hot paths, with `par_bench` for multithreaded scenarios and smoke checks for benchmark execution
* ✅ **Callgrind benchmarks** - Selected hot paths measure instruction counts and simulated cache behavior under Valgrind/Callgrind. See [docs/callgrind-benchmarks.md](docs/callgrind-benchmarks.md)
* ✅ **Zero warnings policy** - Compiler and Clippy warnings are rejected, with extensive workspace lint rules
* ✅ **Cross-platform validation** - Windows, Linux and macOS checks run where applicable; ARM64 validation is best-effort under the [platform support policy](docs/build-and-tooling.md#platform-support-and-validation)
* ✅ **API documentation** - Rustdoc checks and executable documentation examples, including documentation builds with default and all features
* ✅ **Dependency checks** - Security audits with `cargo-audit`, minimum-version compilation, feature-combination checks and unused-dependency analysis
* ✅ **Release compatibility** - `cargo-semver-checks` checks library API compatibility, external-type checks guard public API dependencies, and release-plan validation enforces required version increments

See [build and tooling](docs/build-and-tooling.md) for local commands and
[scheduled validation](docs/scheduled-validation.md) for deep-check execution and failure handling.