# Benchmark engines

`cargo-bench-history` supports four benchmark engines. It does not run them by name — it
enables the combined environment they all need and harvests whichever produced output. The
engines split along two axes that drive the whole data model: **hardware-dependent vs.
hardware-independent** (which decides partitioning), and **whether each measurement carries a
confidence interval** (which decides how dispersion is gated). No engine is exempt from
run-to-run noise.

| Engine | Measures | Hardware | Confidence interval |
|---|---|---|---|
| **Criterion** | Wall-clock time | Dependent | Yes |
| **Callgrind** (via Gungraun) | Simulated instruction / branch counts | Independent | No (single value) |
| **`alloc_tracker`** | Heap allocations (bytes and counts) | Independent | Yes |
| **`all_the_time`** | Processor (CPU) time | Dependent | Yes |

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

## Shared shape

Despite differing in units, noise, and hardware-dependence, all four engines reduce to the
same shape: *a stable benchmark identity → a set of named numeric metrics*. Every persisted
metric is **lower-is-better**, so a rise is always a regression and a fall an improvement.
