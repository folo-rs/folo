# Measurement stability

`cargo-bench-history` compares a benchmark against its own past. That comparison is only
meaningful if a run's number moves when — and only when — the thing being measured actually
changes. Wall-clock benchmarks have a notorious source of movement that has nothing to do with
your code: **instruction-cache layout**.

## Workloads outside automated analysis

Concurrent work, thread creation, live system queries and affinity changes can be dominated
by environmental decisions. Repetition and noise gates can reduce random jitter but cannot
identify the cause of a persistent shift. A benchmark may remain useful for manual
experiments without being suitable for automated history analysis.

Use [configured benchmark ignores](../commands/analyze.md#ignoring-benchmarks) to keep such
measurements out of analysis while retaining collection and raw inspection. This is a
project policy, not an automatic classification of all multithreaded benchmarks as invalid.
Ignoring excludes a benchmark from comparisons; a blessing instead accepts a baseline
change in a benchmark you still want analyzed.

## The phantom regression

A CPU fetches instructions in fixed-size lines (64 bytes on x86-64), and instruction-fetch
and decoding behavior can depend on code placement. The linker orders functions in the final
binary, while the compiler determines the layout inside each function. An unrelated dependency
change can move a function; a change to untimed setup can move a measured loop within it.

The result is a step in the timeline with **no source change**: a byte-identical hot loop that
used to sit inside one cache line now straddles two, and the benchmark reports a regression that
no diff explains. It persists until the layout shifts again, so it looks like a real, lasting
change. This is a property of the machine, not of the tool: the same relink would perturb any
wall-clock measurement.

## Pinning the layout

Forcing every function to start on a cache-line boundary preserves its cache-line-relative
layout when a byte-identical function is relocated. A hot loop's position then depends on its
offset within that generated function, not on what the linker placed before it.

With the LLVM backend (stable Rust), set the flag through the `RUSTFLAGS` environment variable
when building benchmarks — for example:

```sh
# bash / zsh
RUSTFLAGS="-Cllvm-args=-align-all-functions=6" cargo bench
```

```powershell
# PowerShell
$env:RUSTFLAGS = "-Cllvm-args=-align-all-functions=6"; cargo bench
```

The value is a **log2 exponent**, so `=6` selects a 64-byte boundary. A smaller boundary does
not fully fix the function entry's offset within a cache line.

Function alignment does not freeze a function's internal layout when code generation changes.
The measured instructions can remain identical while different allocation or cleanup code
around them changes their placement. Nor does alignment stabilize data placement or every
other source of native timing variation.

The option applies to the LLVM code being compiled, not to prebuilt libraries, hand-written
assembly, or C dependencies built without equivalent options. Padding increases executable
code size and can shift the measured baseline.

## Internal-block alignment

A **basic block** is a straight-line group of instructions inside a function. Internal
alignment options address these blocks rather than only function entry points. The LLVM
backend provides options with different execution costs:

- `-align-all-nofallthru-blocks=6` aligns blocks that have no fall-through predecessors.
  Its padding is not executed by falling through from a preceding block. It can stabilize
  selected internal positions, but does not align every block.
- `-align-all-blocks=6` aligns every block, including those reached by fall-through.
  That padding can execute inside a measured path, changing its instruction count and timing.

Both use the same log2 convention as function alignment. Avoiding executed padding does not
make the selective option free: its larger code footprint can still affect native behavior.
Adding either option is a benchmark-build policy decision, not a universal stability fix.

Compare the affected benchmark and neighboring controls before adopting a policy. Distinguish
applying the option only to benchmark code from applying it through all Cargo-built Rust
dependencies; the code-size and timing effects can differ substantially. Keep allocators,
sampling settings, source and other compiler options matched. Faster results in one case
do not establish better stability for the whole suite.

## `cargo-bench-history` does not impose this

The tool measures whatever your benchmarks produce; it does not inject `RUSTFLAGS` or dictate a
build profile. Stability of the *inputs* is the responsibility of whoever builds and runs the
benchmarks. Apply the flag in your own benchmark build path — a `just` recipe, a CI step, a
`cargo bench` wrapper — so that every run the tool collects is built the same way.

Consequences worth planning for:

- **Interpret each engine separately.** Function-entry alignment addresses native timing,
  not executed instruction counts or allocation counts. Internal padding that executes can
  affect an instruction-count engine such as Callgrind, so do not treat every alignment
  option as equivalent or automatically apply it to every measurement path.
- **Introducing it is a one-time step.** Because the build flag is not part of a result's
  identity, aligned and unaligned runs share a series, so turning alignment on shifts wall-clock
  numbers once, at the commit that introduces it. Land that change on its own commit and
  [`bless`](../commands/bless.md) the affected series at it —
  `cargo bench-history bless --all --context <commit>` accepts the whole workspace in one step — so
  history-mode analysis reads the step as an accepted baseline change rather than a regression.
  The step *will* be reported until you do: noticing that the measured level moved is the
  detector's job, and recognizing that a build-flag change caused it is yours.
- **Pull-request comparisons honor the same boundary.** Branch mode keeps the blessed commit and
  excludes every earlier base observation. Until enough aligned points accumulate at or after the
  blessing, an affected wall-clock series is reported as unjudged rather than compared across the
  configuration change.
- **Backfilling across the change mixes the two configurations.** `RUSTFLAGS` reaches
  [`backfill`](../commands/backfill.md) from the invocation, not from the commit being measured,
  so backfilling commits that predate the alignment change measures them *with* alignment while
  their originally-collected neighbors lack it. Keep backfilled ranges recent enough to stay
  inside one configuration, or expect an extra step where the two meet.

## The repetition count is part of the protocol

[`collect --best-of N`](../commands/collect.md) is a second source of movement with no source
change, and this one is self-inflicted. It runs the suite `N` times and stores the per-metric
**minimum**, and the expected value of a minimum falls as the sample count rises — so raising
or lowering `N` can shift the recorded level of the whole suite at once. The shift reaches a
metric only in proportion to that metric's noise: a deterministic one does not move, and any
single observation may come out unchanged. Pick a value per machine
and project and keep it fixed. Every stored run records the count it was reduced from, so the
protocol behind a value stays recoverable from the stored data.
