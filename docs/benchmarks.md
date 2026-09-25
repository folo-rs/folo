# Benchmarks

This chapter covers the conventions for writing Criterion (wall-clock) and
Callgrind (simulated instruction-count) benchmarks. It is the entry point for
benchmark work; the deep references are
[`docs/callgrind-benchmarks.md`](callgrind-benchmarks.md) for the Callgrind
strategy and [`docs/naming.md`](naming.md) for file and identifier naming.

## Benchmark design

Unless otherwise prompted, create single-threaded synchronous Criterion
benchmarks. Use benchmark groups to group related benchmarks that make sense to
compare to each other.

Focus on benchmarking elementary operations, do not create benchmarks with lots of
long-winded logic. We generally want to benchmark a single API call or at most a
sequence of closely coupled API calls.

Only the functionality being benchmarked should be inside the `.iter()` closure,
with the data setup being either done outside (if not per-iteration) or using the
first "payload preparation" callback of `iter_batched()` (if per-iteration).

If multithreaded benchmarks are truly appropriate, use `bench_on_threadpool()` for
them. When using this for multithreaded benchmarks, also run any single-threaded
benchmarks via `bench_on_threadpool()` to ensure that overheads are comparable.

Inside the benchmark closure, use `std::hint::black_box()` to consume output
values from the code being benchmarked, to avoid unrealistic eager optimizations
due to output values that are discarded.

Benchmarks that are meant to be compared to each other must be in the same
benchmark function and in the same benchmark group.

Do not forget to register benchmarks in `Cargo.toml`.

Benchmark executables follow the [executable allocator policy](testing.md#executable-allocators):
mimalloc natively, with allocation instrumentation wrapping that same backend.
When comparing a tracking wrapper against an untracked allocator, use
`testing::DefaultAllocator` for both sides so the difference measures tracking,
not a change of allocator. Allocator changes affect benchmark baselines and must
not be interpreted as changes to the measured library algorithms.

## Bound routine workloads

Routine benchmarks are microbenchmarks, not production-sized data sets or stress
tests. Keep each elementary operation millisecond-scale, preferably microseconds
or a few milliseconds, never seconds. Keep the workload small enough for
Criterion's full default sample count to fit its normal short measurement window.
Prefer a meaningful low and high
size for scaling; additional variants need a distinct algorithmic reason.

Choose sizes from the work being exercised, not from production limits. Preserve
the intended algorithm branch, tie pattern, evidence diversity, and setup
invariants when reducing a fixture. Explain that purpose next to its size
constants, and keep identifiers accurate when the measured workload changes.
Do not claim production-ceiling coverage for a reduced workload. If an essential
branch cannot be measured as bounded meaningful work, report that conflict rather
than weakening production behavior to make the benchmark faster.

Check actual per-iteration estimates and a full-sampling run of every changed high
case. A configured measurement interval is not an execution-time limit: Criterion
can exceed it when even one iteration per sample is too expensive. Do not hide
oversized iterations with `--quick`, fewer samples, or a longer measurement window.
Remove duration overrides supported only by a vague claim of noisy results;
intentional overrides need a specific measurement reason.

For an already bounded elementary operation, explicit flat sampling can retain
the full sample count within the default window when linear sampling's minimum
triangular iteration schedule is too expensive. Explain that choice; flat and
linear sampling do not have identical statistical properties. This is not a
substitute for reducing an oversized fixture.

Also inspect untimed setup, teardown, and `iter_custom` loops. Batching, worker
startup, cache preparation, and report generation can dominate elapsed runtime
even when the reported operation is small. Keep setup bounded and distinguish
these costs from measured work. Measure serially on shared machines, retain the
sampling warnings and per-case estimates, and treat timings as workload-budget
evidence rather than precise performance comparisons.

Manual exploratory benchmarks whose purpose requires large working sets or
production-scale exhaustion are separate from these routine microbenchmarks.

## Avoid syscalls unless I/O is the thing being measured

Unless the benchmark's *purpose* is to measure I/O or some other
operating-system service, keep syscalls out of the measured path. Syscalls
(opening files, accessing the network, spawning processes, and so on) have
unpredictable latency that depends on kernel scheduling, filesystem and device
state, caches, and unrelated processes on the machine. That is noise which has
nothing to do with the code under test and which varies from run to run and from
machine to machine.

Calling a disk-writing API does not by itself make a general report benchmark an
I/O benchmark. Measure available in-memory aggregation, rendering, or serialization
operations instead; remove redundant scenarios rather than adding public API just
for a benchmark. Deliberate I/O benchmarks need an explicit I/O-specific purpose
and workload, separate from ordinary microbenchmarks. Emitting Criterion or tracker
reports after measurement is harness output, not a reason to include file writes
inside each measured iteration.

When a benchmark needs a stand-in "workload" to represent the body of a task,
use a small, deterministic, CPU-only computation seeded with `black_box` (so the
optimizer cannot fold it away) rather than a syscall standing in as busywork.

Some syscalls are genuinely unavoidable — spawning a thread, for example, is
inherently a kernel operation and is the very thing a worker-pool benchmark
exists to measure. Even then, minimize them: perform the unavoidable syscalls
once during setup where possible, and keep them out of the per-iteration
workload.

Benchmark file names, Criterion group names, and Callgrind group/function names
follow strict conventions documented in [`docs/naming.md`](naming.md): the file
basename prefixes Criterion group names, Callgrind files require a paired
Criterion file, and Callgrind identifiers mirror Criterion ones with `/`
substituted by `_`. In particular, when a benchmark tracks allocations or
processor time, the `alloc_tracker`/`all_the_time` operation name **must equal
the full Criterion benchmark identifier** (`<group>/<bench_function name>`, where
the `bench_function` name may itself contain `/` segments) so the two reports
correlate — see the tracking-session operation-name rule in
[`docs/naming.md`](naming.md).

## Stack pin vs. `Box::pin` on the measured path

Do not use `Box::pin(value)` on the measured path. It allocates a `Box` on the
heap on every iteration, which can easily add 100-200 instructions (or 40-50% of
the measurement) of pure allocator overhead that has nothing to do with the
operation under test. Use `std::pin::pin!(value)` instead — it pins on the stack
with zero allocation. Add a brief inline comment justifying the deviation from the
usual `Box::pin` preference (e.g. "stack-pin to avoid allocator noise on the
measured path"). This is an exception to the workspace-wide rule against the
`pin!` macro (see the examples chapter).

`Box::pin` remains correct in benchmark code that is **not** inside the measured
region:

* Criterion `iter_custom` setup (anything before `Instant::now()`).
* The first ("payload preparation") callback of `iter_batched()`.
* Gungraun setup functions referenced via `#[bench::id(setup_fn())]` — these run
  outside the measured region and pass the result into the bench body by value.
* Helper functions that must return `Pin<Box<T>>` across a function boundary (a
  stack pin would dangle).
* Intentional `Box::pin` baselines where the allocation IS what is being
  measured.

## Callgrind benchmarks

For performance-critical hot paths, complement the Criterion benchmarks with
Callgrind-based instruction-count benchmarks. See
[`docs/callgrind-benchmarks.md`](callgrind-benchmarks.md) for the strategy,
scenario selection, bench file template, the Criterion-pairing convention, and
how to interpret results.

## Hash containers and instruction-count determinism

A Callgrind instruction count is only meaningful if byte-identical source
produces the same count across builds. One subtle way to break that is a
`HashMap`/`HashSet` built with a **default, randomly-seeded hasher**.

These default seeds are not fixed at compile time. `std`'s `RandomState` draws
its seed from the operating system RNG, so it varies from run to run.
`foldhash::fast::RandomState` mixes process memory addresses (stack pointers and
code/static segment locations) with the wall-clock time at first use, so it also
varies from run to run, even under Valgrind — which disables ASLR but does not
freeze the clock. Either way the seed is unrelated to the benchmark's logic, so
hash iteration order and probe counts shift, changing the instruction count on
byte-identical source. The result is phantom regressions and improvements in
benchmark history that track nothing but the seed.

The symptom is distinctive: only the benchmarks that **build or iterate a hash
container** jitter across builds, while every other benchmark is bit-stable. A
demonstration showed the same binary swing by ~7% (18260 vs 19607 instructions)
from a stack-address change alone; with a fixed seed the count became immovable
under the same perturbation.

If a benchmarked code path uses a hash container, give it a **fixed-seed
hasher** so the measurement depends only on the code, not on load addresses. In
this workspace, use `foldhash::fast::FixedState`:

```rust
use foldhash::fast::FixedState;

type HashMap<K, V> = std::collections::HashMap<K, V, FixedState>;

let map = HashMap::default(); // FixedState: Default, so this is deterministic
```

Only drop the randomized seed where the keys are trusted rather than
attacker-controlled, since `foldhash` documents a fixed seed as trivially
vulnerable to HashDoS. Keys that reach a container through a public API are not
trusted, even when every caller inside this workspace supplies a constant.

Randomizing the seed is not by itself the property such a container needs.
`foldhash` calls itself only minimally DoS-resistant and tells callers not to
rely on it for security regardless of seeding, whereas the standard library
states that its default `HashMap` hasher is selected to resist HashDoS and is
seeded from the host's secure randomness. A container keyed by names the caller
chooses — `nm`'s event registries and the `nm_otel` export path, which accept
runtime-derived event names — therefore uses `std::hash::RandomState` and accepts
both the extra hashing cost and the instruction-count variance. Reserve the
fixed seed for containers whose keys the benchmarked code chooses itself.
