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

Re-running Callgrind on one unchanged machine often prints the same count every time — the
counter is deterministic for a fixed binary and fixed input. What is *not* fixed is everything
feeding it across the commits being compared: a different OS or CPU-microcode patch level, a
different compiler patch release, the compiler's own nondeterministic code-generation choices
(inlining, ordering, layout) even at the same version, and Criterion scheduling a different
iteration count under different background load. Any one of these moves the measured number
without the code under test changing.

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
