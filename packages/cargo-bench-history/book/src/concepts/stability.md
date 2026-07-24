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

Align **functions only** — for code the LLVM backend emits under deterministic codegen (which is
what a Rust benchmark and its dependencies compile to), this is a *complete* fix for relink-driven
instability, not a partial one. Once every function starts on a cache-line boundary, a loop's
offset from the nearest boundary equals its offset *within its own function*. Relinking only ever
moves whole functions around; it never rewrites a function's internal byte layout. So that
intra-function offset — and therefore the loop's position relative to the cache lines — is fixed
for a given source and can no longer shift when an unrelated dependency reorders the binary. (The
flag aligns only LLVM-emitted functions; machine code from a C dependency, hand-written assembly
or a prebuilt static library is unaffected, and would need its own alignment if it hosted the hot
loop.)

Do **not** additionally pass `-align-all-nofallthru-blocks`. It aligns basic blocks *inside*
functions, which buys no extra relink stability — intra-function layout is already deterministic
under fixed codegen — while injecting NOPs into the measured path that can make small hot loops
dramatically slower. A loop that straddles a line purely because of its offset within its own
function does so deterministically for a given source: that is a fixed performance cost, not a
phantom regression, so it is a performance question rather than a stability one.

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
  identity, aligned and unaligned runs share a series, so turning alignment on shifts wall-clock
  numbers once, at the commit that introduces it. Land that change on its own commit and
  [`bless`](../commands/bless.md) the affected series at it —
  `cargo bench-history bless --all --context <commit>` accepts the whole workspace in one step — so
  history-mode analysis reads the step as an accepted baseline change rather than a regression.
- **Pull-request comparisons re-baseline on their own.** Blessing re-baselines history-mode
  analysis only; a pull request is judged directly against the recent base-branch points, which
  blessing does not adjust. Until enough aligned points accumulate on the base branch, a pull
  request that touches an affected wall-clock benchmark can show a one-time-shift-sized move in its
  performance comment. Those findings are advisory rather than merge gates and clear themselves
  once the aligned baseline fills in.
