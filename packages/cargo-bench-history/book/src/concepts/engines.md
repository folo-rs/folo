# Benchmark engines

`cargo-bench-history` supports four benchmark engines. It does not run them by name — it
enables the combined environment they all need and harvests whichever produced output. What
drives the data model is **whether each measurement carries a confidence interval** (which
decides how dispersion is gated). No engine is exempt from run-to-run noise, and every
engine's numbers are machine-dependent in practice — even simulated instruction counts and
allocation figures vary with the host, because libraries dispatch to different code paths on
different microarchitectures — so every engine is partitioned by machine key.

| Engine | Measures | Confidence interval |
|---|---|---|
| **Criterion** | Wall-clock time | Yes |
| **Callgrind** (via Gungraun) | Simulated instruction / branch counts | No (single value) |
| **`alloc_tracker`** | Heap allocations (bytes and counts), plus optional peak outstanding bytes | Only when the operation was measured over several spans |
| **`all_the_time`** | Processor (CPU) time | Only when the operation was measured over several spans |

An interval needs something to vary across, so the two operation engines report one only
where an operation was measured more than once. A single-span operation records its value
alone, and analysis then judges it the same way it judges Callgrind's counts — by comparing
across commits rather than by reading a per-measurement interval. Nothing is lost: an
interval can only ever *suppress* a finding, never create one.

## Why no engine is deterministic

Re-running Callgrind with the exact same binary and input often prints the same count. A
history series does not hold those inputs fixed: every commit rebuilds the benchmark, and a
toolchain, dependency, target-feature, profile, or benchmark-input change can produce a
different instruction stream. Even the same compiler version can make different
code-generation choices around inlining, ordering, and layout. Those environmental changes
are valid observations in the timeline, but they mean no engine's historical values should
be treated as mathematically exact.

## What Callgrind persists

For Callgrind, layout-sensitivity is decisive at microbenchmark scale, so only the
build-stable events are persisted: the instruction count (`Ir`) and the two branch-execution
counts (`Bc`, `Bi`) — they count *what the code did*. The cache-simulation counts, the derived
`EstimatedCycles`, and the branch-misprediction counts reflect *where the code and data landed
in memory* and swing by tens of percent between two builds of identical source, so they are
never parsed or persisted.

## Allocation peaks

Available allocation peaks are collected and analyzed automatically as
`peak_outstanding_bytes`. This is allocator-requested memory relative to the start of a span,
not RSS or the simultaneous total across worker threads.

**Benchmarks that accumulate memory between iterations should use
`operation.measure_thread().no_peak().iterations(n)` in `alloc_tracker`.** Their peak depends
on the batch size the harness selects and can produce false regression alerts even with a
narrow confidence interval. Varying iteration workloads can also make the batch maximum
depend on batch size; freeing pre-existing allocations or transferring allocations across
threads can make the reported peak unsuitable for comparison.

Opting out withholds the operation's peak from both human and machine-readable producer
reports, without losing bytes allocated or allocation count. Process-scoped measurements
already withhold peaks. Missing peaks are not zero measurements, and opting out does not
erase historical data. Use the opt-out consistently across repetitions of a `--best-of` run,
which must all report the same metric set.

## Shared shape

Despite differing in units and noise, all four engines reduce to the
same shape: *a stable benchmark identity → a set of named numeric metrics*. Every persisted
metric is **lower-is-better**, so a rise is always a regression and a fall an improvement.
