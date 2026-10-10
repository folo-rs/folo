# Measurement stability

`cargo-bench-history` compares a benchmark against its own past. Use a consistent measurement
protocol so changes in compiler layout or sampling do not obscure changes in the measured work.

For native timing benchmarks, **use the compiler configuration below**, keep the repetition
count fixed, and exclude workloads whose results are dominated by environmental decisions.

## Recommended compiler configuration

Use the following configuration for LLVM-based Rust benchmark builds. This is the configuration
used for normal measurements in the Folo workspace:

```sh
# bash / zsh
RUSTFLAGS="-Cllvm-args=-align-all-functions=6 -Cllvm-args=-align-all-nofallthru-blocks=6" cargo bench
```

```powershell
# PowerShell
$env:RUSTFLAGS = "-Cllvm-args=-align-all-functions=6 -Cllvm-args=-align-all-nofallthru-blocks=6"
cargo bench
```

These examples assume no other compiler flags are configured. Preserve unrelated options when
adding this policy to an existing build. `CARGO_ENCODED_RUSTFLAGS` takes precedence over
`RUSTFLAGS`; if it is already set, add the options to that encoded argument list instead.

Apply the configuration through the Cargo-built Rust dependencies as well as the benchmark
executable, and use it consistently in local, CI and backfill measurements. The values are
**log2 exponents**: `6` selects the 64-byte instruction-cache-line size of x86-64.

## Why this configuration

Instruction-fetch and decoding behavior can depend on code placement. The linker orders
functions in the final binary, while the compiler determines the layout inside each function.
An unrelated dependency change can move a function; a change to untimed setup can move a
measured loop within it. A persistent timing shift can therefore occur even when the measured
instructions have not changed.

A **basic block** is a straight-line group of instructions inside a function. A block is reached
by **fall-through** when execution continues into it from the preceding block without a jump.

Function-entry alignment preserves cache-line-relative positions when a byte-identical
function is relocated. It does not freeze internal positions when code generation changes,
which is why the recommendation also includes selective internal-block alignment.

`-align-all-nofallthru-blocks=6` aligns blocks without fall-through predecessors, keeping its
padding off those executed paths while stabilizing selected internal positions.

**Do not substitute `-align-all-blocks=6`.** It also aligns blocks reached by fall-through,
which can execute padding inside the measured path and change its instruction count and timing.

The recommended configuration increases executable code size, especially when applied through
dependencies. Accept that footprint cost as part of the measurement protocol rather than
mixing benchmark-only and whole-build configurations in one history. It reduces layout
sensitivity; it does not stabilize data placement or eliminate every source of native variation.

The options affect the LLVM code being compiled. They do not realign prebuilt libraries,
hand-written assembly, or C dependencies built without equivalent options.

## Use one protocol throughout the history

`cargo-bench-history` measures whatever your benchmarks produce; it does not inject compiler
flags or dictate a build profile. Put the recommended configuration in the build path that
every collection uses, such as a command recipe or CI configuration.

Treat a deliberate compiler-policy change as a measurement-baseline change: build flags are
not part of a result's identity, so measurements from different policies share a series.
Use [`bless`](../commands/bless.md) to record accepted shifts at the introducing commit, with
the benchmark selection and blessing scope matching the expected change. Use `--all` only
when acceptance intentionally covers every benchmark, not to dismiss unrelated findings.

Branch comparisons honor that baseline boundary too. They retain the blessed base commit
and exclude earlier evidence; affected series remain unjudged until enough measurements
exist at or after the boundary.

Keep backfilled ranges within a consistent compiler policy. Flags reach
[`backfill`](../commands/backfill.md) from its invocation, not from the historical commit
being measured, so a current build configuration also applies when collecting older commits.

The alignment recommendation addresses native timing. Instruction-count-only build paths
need not add native-timing stability flags; keep their comparisons consistent separately.

## Keep the repetition count fixed

Choose a repetition count for each machine and project and keep it fixed.
[`collect --best-of N`](../commands/collect.md) runs the suite `N` times and stores the
per-metric **minimum**. The expected minimum falls as the sample count rises, so changing
`N` can shift the recorded level without changing the benchmarked code.

The shift depends on each metric's noise: a deterministic metric does not move, and any
single observation may remain unchanged. Every stored run records the count it was reduced
from, so the measurement protocol remains recoverable from the data.

## Exclude environment-dominated workloads

Concurrent work, thread creation, live system queries and affinity changes can be dominated
by environmental decisions. Repetition and noise gates can reduce random jitter but cannot
identify the cause of a persistent shift. Keep these workloads as manual experiments when
they cannot support useful automated comparisons.

Use [configured benchmark ignores](../commands/analyze.md#ignoring-benchmarks) to exclude
such measurements from analysis while retaining collection and raw inspection. This is a
project policy, not an automatic classification of all multithreaded benchmarks as invalid.
Ignoring excludes a benchmark from comparisons; a blessing instead accepts a baseline
change in a benchmark you still want analyzed.
