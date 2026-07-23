# Measurement stability

`cargo-bench-history` compares a benchmark against its own past. That comparison is only
meaningful if a run's number moves when — and only when — the thing being measured actually
changes. Wall-clock benchmarks have a notorious source of movement that has nothing to do with
your code: **instruction-cache layout**.

## The phantom regression

A CPU fetches instructions in fixed-size lines (64 bytes on x86-64). A hot loop that fits
entirely inside one line is fetched cheaply; the same loop straddling two lines costs an extra
fetch on every iteration. Where a loop lands is decided by the linker, which orders functions in
the final binary. That order can shift for reasons entirely outside the benchmarked crate — most
commonly an unrelated dependency version bump that adds or removes code ahead of your function.

The result is a step in the timeline with **no source change**: a byte-identical hot loop that
used to sit inside one cache line now straddles two, and the benchmark reports a regression that
no diff explains. It persists until the layout shifts again, so it looks like a real, lasting
change. This is a property of the machine, not of the tool: the same relink would perturb any
wall-clock measurement.

## Pinning the layout

You can make layout a deterministic function of your source by forcing every function to start on
a cache-line boundary. Then a hot loop's position relative to the cache lines depends only on its
own offset within its function — which your source fixes — and never on what the linker placed
before it.

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

The value is a **log2 exponent**, so `=6` aligns every function to `2^6 = 64` bytes — one full
cache line. `=5` (32 bytes) is cheaper but only *partly* pins the layout: a function can still
start at either the `0` or `32` offset within a line, so a loop long enough to cross the midpoint
can still straddle. Only 64-byte alignment makes each loop's placement fully deterministic.

Align **functions only**. Do not add `-align-all-nofallthru-blocks`: that pads *inside* loops,
injecting NOPs into the measured path, which can make small hot loops dramatically slower rather
than more stable.

The cost is a modest `.text` size increase (padding between functions) and, for a benchmark that
was previously lucky enough to sit inside a line, a one-time shift to its aligned position. After
that, the series stays put.

## `cargo-bench-history` does not impose this

The tool measures whatever your benchmarks produce; it does not inject `RUSTFLAGS` or dictate a
build profile. Stability of the *inputs* is the responsibility of whoever builds and runs the
benchmarks. Apply the flag in your own benchmark build path — a `just` recipe, a CI step, a
`cargo bench` wrapper — so that every run the tool collects is built the same way.

Two consequences worth planning for:

- **Only wall-clock engines need it.** Instruction-count engines (for example Callgrind) count
  executed instructions, which do not depend on where functions land, so alignment neither helps
  nor should be applied there. Allocation-tracking metrics are likewise layout-invariant.
- **Introducing it is a one-time step.** Because the build flag is not part of a result's
  identity, aligned and unaligned runs share a series. Turning alignment on shifts wall-clock
  numbers once, at the commit that introduces it. Land that change on its own and
  [`bless`](../commands/bless.md) the affected series at that commit so the step is not reported
  as a regression.
