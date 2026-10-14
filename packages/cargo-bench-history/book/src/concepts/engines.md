# Benchmark engines

Use an engine your benchmark target already runs; the history tool is not a benchmark
harness. Check the target's Cargo features and native dependencies on each selected
platform before trying historical replay.

## Check native output

`collect --verbose` explains the output roots it scans and the files admitted by the
run's freshness boundary. A successful compile, harness smoke pass, or console table
is not a stored measurement. Check these native outputs under the resolved Cargo
target directory:

| Engine | Measurement output to inspect | Prerequisite check |
| --- | --- | --- |
| Criterion | `criterion/**/new/benchmark.json` paired with `estimates.json` | Run the measurement target, not just `--no-run` or test mode. Criterion writes its JSON during measurement. |
| Gungraun / Callgrind | `gungraun/**/summary.json`, current-run aggregate counts in schema 6 or 7 | Prepare Gungraun's runner and Valgrind on a platform supported by those tools. Collection supplies `GUNGRAUN_SAVE_SUMMARY=pretty-json`; it does not install native prerequisites. |
| `alloc_tracker` | Per-operation JSON under `alloc_tracker/` | Enable the producer's required features, instrument the measured allocator and finish the measurement session so it emits its output. |
| `all_the_time` | Per-operation JSON under `all_the_time/` | Enable the producer's required features and finish the measurement session so it emits its output. |

Follow the producer documentation for
[Criterion](https://bheisler.github.io/criterion.rs/book/),
[Gungraun](https://gungraun.github.io/gungraun/),
[`alloc_tracker`](https://docs.rs/alloc_tracker/) and
[`all_the_time`](https://docs.rs/all_the_time/).
The collector's parser error or verbose harvest diagnostic is the evidence to investigate;
empty output alone does not prove a particular engine or runner problem.

The operation adapters require usable per-iteration slopes; absent or non-finite
required slopes omit an operation. Allocation peaks are optional. The
[data-shape reference](../appendix/shape.md#which-number-is-taken) owns estimator
and metric details.

### Target selection and names

Check Cargo metadata for `required-features`, then run a small explicit `--package`
and `--bench` scope with those features enabled. Do not assume a successful
workspace build ran every feature-gated target. Local Cargo defaults and reusable
workflow defaults differ; [GitHub onboarding](../github-automation.md) makes the
workflow feature choice explicit.

Benchmark IDs are not uniformly package-qualified. Criterion uses its group/function
identity; the operation engines use operation names; Callgrind carries package
attribution. Give unqualified names a workspace-wide convention to avoid collisions.
Renames start new series, rather than transferring old evidence to the new name.
The [identity reference](../appendix/shape.md#benchmark-identity) describes the
engine mappings, and [comparability](comparability.md#benchmark-identity) explains
their effect on history.

## What the engines measure

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

Collection supports Gungraun summary schemas 6 and 7. Both supply the current run's
aggregate counts; baseline-only values and per-part copies are not collected.

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
