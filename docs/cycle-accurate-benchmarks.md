# Cycle-accurate benchmarks

This document describes our strategy for adding instruction-count benchmarks
alongside the existing wall-clock Criterion benchmarks. (We use the
colloquial "cycle-accurate" name throughout because that is how the file
suffix and the just recipe are spelled, but the headline measurement is the
instruction count; see "Interpreting results" below.) The doc covers the
why, the when, the how, and the conventions that keep these benchmarks
comparable and maintainable across the workspace.

## Why two kinds of benchmarks

We use two complementary benchmark mechanisms:

* **Wall-clock benchmarks** ([Criterion][criterion], often via `par_bench` for
  multithreaded shapes) measure real-world latency on real hardware. They
  capture cache effects, branch prediction, contention, and operating system
  jitter that matter for actual users. The downside is that the numbers are
  noisy and machine-dependent.
* **Instruction-count benchmarks** run each function exactly once under
  [Valgrind][valgrind]'s [Callgrind][callgrind] CPU simulator (driven by
  the [Gungraun][gungraun] harness). They produce deterministic
  instruction counts and simulated cache-hit counts that are stable
  run-to-run on the same toolchain. The downside is that the simulator is
  single-threaded, uses a fixed cache model with no out-of-order execution
  or prefetcher, and models syscalls as a fixed cost — so it cannot
  reproduce real contention, scheduling, or kernel behavior.

The two are complementary, not redundant. Criterion tells you whether a change
is observably faster or slower for a user. Instruction counts tell you
**why** — pointing at the specific code-path delta that explains the
wall-clock difference, or at a regression that wall-clock noise might be
hiding.

The pairing rule is therefore **asymmetric**:

> Every cycle scenario must have an analogous Criterion scenario covering
> the same operation. The pair gives both signals: "did the wall-clock cost
> change?" and "did the instruction count change?".
>
> The reverse is not required. Criterion scenarios can legitimately exist
> without a cycle counterpart when the operation is dominated by something
> Callgrind cannot meaningfully model: multi-threaded contention, syscall
> behavior, allocation, scheduling, or bulk throughput where per-instruction
> resolution adds no signal.

## When to add a cycle-accurate benchmark

Add a cycle-accurate benchmark when at least one of these is true:

* The package's value proposition is **low overhead** (an allocator, a pool,
  a metrics primitive, a synchronization primitive, a thread-local cache, a
  small queue or stack). The whole point of the package is "this operation
  should take very few instructions"; a deterministic instruction count is
  the highest-fidelity way to enforce that.
* The hot path runs on **every** call from a downstream consumer (event
  observation, executor wake, channel poll, lookup table read). Any
  regression compounds across millions of calls, but may be invisible to
  Criterion because each individual call is well under a microsecond.
* The operation has **branching shape** that matters (first / last / miss,
  empty / one / many, cached / uncached, idle / dirty). Cycle counts make
  these branches legible at a glance, where Criterion runs them all together
  and reports a mean.
* The package documents performance claims in comments or README (e.g. "we
  use SIMD here", "this avoids allocation", "this is faster than `Box::pin`").
  Cycle counts pin those claims to a number.

Do **not** add a cycle-accurate benchmark for:

* Operations dominated by I/O, allocation, syscall, or kernel parking cost
  — the simulator models the cost as essentially-free for these. If you do
  benchmark such an operation anyway, document the scope explicitly: "this
  measures wrapper overhead, not the actual cost of the underlying syscall".
* Operations that only matter at scale (throughput, contention, scheduling
  fairness) — these need Criterion + `par_bench`, not cycle counts.
* Blocking waits or any operation that may park a thread. Cycle scenarios
  must be deterministic and non-blocking. Wait on already-signaled state,
  poll already-ready futures, etc.
* Tests for "is this still correct" — that is what the test suite is for.
* Internal helper functions that are not part of the package's public
  contract; benchmark the public API, not the implementation detail.

## Scenario selection

Each `_cycles.rs` file should cover **2 to 6 logical axes** of the operation
(the product of those axes may produce more than 6 measured cases). Aim for
the smallest set that catches the regressions you care about.

Use this checklist when choosing scenarios:

1. **The default case** — what almost every consumer does. Plain
   `Event::observe_once()`, `pool.insert(v)`, `pool.acquire()`, etc.
2. **Branching extremes** — for any operation with a meaningful branch (hit
   vs miss, first vs last, ready vs pending), include both endpoints.
   Include `miss` cases when applicable, because they are the most common
   regression source.
3. **State / occupancy variants** — clean vs dirty, empty vs populated,
   first-touch vs reused capacity, cached vs invalidated, idle vs updated.
   These are commonly the most regression-sensitive shapes.
4. **Size sensitivity** — when an operation's cost grows with input size (a
   bucket scan, a list walk, a registry scan), include a small and a large
   case to make the per-item cost legible.
5. **Initialization vs steady state** — if the first call differs from later
   calls (lazy init, thread-local first-touch), include both. Use Gungraun's
   `setup` parameter to bring the structure to steady state before the
   measured call.
6. **Sibling variants** — if the same logical operation has multiple
   implementations the user can pick between (sync vs local, pull vs push,
   embedded vs boxed), benchmark each one with the same scenarios so the
   numbers are directly comparable.

What **not** to do:

* Do not benchmark every method of every type. Pick the value-prop hot path
  and skip everything else.
* Do not chain multiple operations into one "realistic workflow" measurement.
  Each benchmark measures **one** operation; if you want to measure a
  sequence, decompose it into per-operation benchmarks plus one composite if
  it adds value.
* Do not measure trivial accessors, `Debug` impls, or constants. The
  compiler is allowed to optimize these to nothing and the noise floor is
  larger than the signal.
* Do not benchmark blocking waits, anything that parks a thread, or anything
  that depends on cross-thread synchronization. The simulator is
  single-threaded and the result will not mean what you think it means.
* Do not benchmark anything that allocates, locks, or syscalls in the hot
  path without first checking that the result is meaningful. The simulator
  models the allocator and the kernel as a fixed cost; comparing benches
  that allocate to benches that do not is misleading.

## Adding a cycle-accurate benchmark

Each Gungraun bench lives alongside the Criterion benches in its package:

```
packages/<pkg>/benches/<name>_cycles.rs
```

The `_cycles.rs` filename suffix is required for `just bench-cycles`
discovery.

### Cargo.toml

In `packages/<pkg>/Cargo.toml`, add a target-gated dev-dependency and a
`[[bench]]` entry:

```toml
[target.'cfg(target_os = "linux")'.dev-dependencies]
gungraun = { workspace = true, features = ["default"] }

[[bench]]
name = "<name>_cycles"
harness = false
```

The `cfg(target_os = "linux")` gate keeps the Gungraun dependency out of
Windows and macOS resolution entirely — without it, `cargo-machete` and
`cargo-udeps` flag the dependency as unused on non-Linux builds. The
`[[bench]]` entry itself is **not** target-gated; bench tables in `Cargo.toml`
do not support `cfg` attributes. Instead, the bench file gates its own
contents (see next section).

### Bench file template

```rust
//! Cycle-accurate benchmarks for <operation> in the `<pkg>` package.
//!
//! Paired with <criterion-bench-name>.rs which covers the same operations
//! under wall-clock measurement.

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
// Gungraun macro expansions trigger a large number of lints we cannot
// control from this file. The list below was assembled empirically and will
// need to grow as either Clippy or Gungraun evolves. We accept the
// maintenance cost in exchange for keeping bench files at the workspace
// lint level for hand-written code; the macro expansion sites are the only
// places these allows actually fire.
#![cfg_attr(
    target_os = "linux",
    allow(
        clippy::absolute_paths,
        clippy::allow_attributes_without_reason,
        clippy::exhaustive_structs,
        clippy::partial_pub_fields,
        clippy::pub_underscore_fields,
        clippy::cognitive_complexity,
        clippy::unnecessary_wraps,
        clippy::ignore_without_reason,
        clippy::default_trait_access,
        clippy::needless_pass_by_value,
        clippy::missing_assert_message,
        clippy::elidable_lifetime_names,
        clippy::needless_pass_by_ref_mut,
        clippy::doc_markdown,
        clippy::needless_for_each,
        clippy::redundant_clone,
        clippy::missing_docs_in_private_items,
        clippy::exit,
        unused_imports,
        unused_qualifications,
        unreachable_pub,
        missing_debug_implementations,
        unnameable_types,
        non_local_definitions,
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only. On other platforms this bench target compiles
    // to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
extern crate gungraun;

#[cfg(target_os = "linux")]
use std::hint::black_box;

#[cfg(target_os = "linux")]
use gungraun::prelude::*;

// ... (your benchmark fns, groups, and `main!` invocation, all
//      `#[cfg(target_os = "linux")]`-gated)
```

A complete worked example lives in
[`packages/nm/benches/nm_observe_cycles.rs`](../packages/nm/benches/nm_observe_cycles.rs).

### Gungraun syntax gotchas

These are easy to get wrong on the first attempt:

* `main!()` generates its own `fn main()`. Invoke it at module top level, not
  wrapped inside another `fn main()`.
* `library_benchmark_group!` requires `benchmarks = [a, b, c]` square
  brackets around the list of benchmark function names.
* `#[bench::id(...)]` accepts either named setup functions or direct value
  expressions. Closure form (`setup = || ...`) is not supported.
* Doc comments (`///`) on `#[library_benchmark]` functions are rejected.
  Use plain `//` comments instead.
* Bench files must include `extern crate gungraun;` even on the 2024
  edition — the macros resolve crate paths at expansion time.

### Pairing with Criterion

For each scenario in the `_cycles.rs` file, an analogous Criterion benchmark
must exist in the same package's `benches/` directory. The two need not be
in the same file, and they need not use identical names — but if a future
maintainer cannot trivially identify the Criterion counterpart for a given
cycle scenario, the pairing has failed.

Practical guidelines:

* Use the same setup functions (or thread-local initializers) for both, so
  the measured object is in the same state. If the cycle scenario uses a
  fresh single-event registry, do not pair it to a Criterion scenario that
  runs against the full thread-local registry of every other benchmark in
  the same file — add a matching controlled-state Criterion scenario.
* Use the same scenario taxonomy: if the cycle bench has `hit_first`,
  `hit_last`, `miss`, the Criterion bench should have the same three (under
  matching or similar names).
* Document the pairing in a one-line `//!` doc comment at the top of the
  `_cycles.rs` file: "Paired with `<criterion-bench-name>.rs`".
* When you add a new cycle scenario, add the matching Criterion scenario at
  the same time. (The reverse — adding cycle coverage when you add a
  Criterion scenario — is encouraged but not required; see the asymmetric
  pairing rule above.)

## Running

Cycle-accurate benchmarks require Valgrind and run only on Linux (including
WSL on Windows).

Install once:

```bash
sudo apt install -y valgrind
cargo install gungraun-runner --version 0.19.0 --locked
```

The `gungraun-runner` version must match the `gungraun` library version pinned in
the workspace `Cargo.toml` exactly — `gungraun-runner` enforces strict string equality on the
version and any drift surfaces as a `VersionMismatch` runtime error. `just install-tools`
performs the equivalent install for you.

Then:

```bash
# Run all cycle-accurate benchmarks across the workspace.
just bench-cycles

# Scope to a single package.
just package=nm bench-cycles

# Run a specific bench file by name.
just bench-cycles nm_observe_cycles
```

On Windows, run the recipe via WSL from the repo root (WSL inherits the
caller's working directory):

```powershell
wsl -e bash -l -c "just bench-cycles"
```

The recipe enumerates every `packages/*/benches/*_cycles.rs` file and runs
each via `cargo bench -p <pkg> --bench <name>`. Subsequent runs automatically
compare against the previous run's baseline in `target/gungraun/` and exit
non-zero if any regression is detected.

## Interpreting results

Each scenario reports:

* **Instructions** — instructions executed in the measured function. This is
  the headline number. Stable run-to-run on the same toolchain.
* **L1 / LL / RAM hits** — simulated cache hits at each level. Useful for
  spotting changes in memory access patterns.
* **Estimated cycles** — a Callgrind-internal weighted sum of the above
  using a fixed cost model. Useful as a rough ranking; do not over-interpret
  small differences. This is **not** a real CPU cycle count: the simulator
  has no out-of-order execution, no realistic branch predictor, no
  prefetcher, and a fixed cache geometry. Treat it as a secondary signal
  behind the instruction count.

Two important caveats:

1. The cache model is fixed (typically L1: 64KiB, LL: 8MiB). It is
   comparable run-to-run but not realistic. Cache numbers indicate
   "memory access pattern changed", not "real cache misses changed".
2. The simulator is single-threaded. Contention, atomics, and memory
   ordering effects do not show up. Multithreaded behavior must be covered
   by Criterion + `par_bench`.

### Regression handling

Gungraun's auto-diff exits non-zero on any regression, however small. This
is a **trip wire**, not a verdict: a regression may be intentional (a new
feature that costs cycles), benign (a layout change in an unrelated type),
or a genuine bug. The pattern is:

1. Run `just bench-cycles` locally before opening a PR. If there is a
   regression, decide whether to accept it.
2. If the regression is intentional, mention it in the PR description with
   the before/after numbers and the rationale.
3. If unintentional, fix it before merging.

We deliberately do **not** treat the trip wire as a CI gate today, because
performance trade-offs require human evaluation, not automated rejection.

## Baselines

Baselines live in `target/gungraun/` and are local to each developer's
machine. They are intentionally not committed: a developer's machine
produces stable counts run-to-run, but a different toolchain or linked
standard library version can shift the absolute counts by a small constant.
The relative deltas remain meaningful within a single environment; the
absolute numbers are not portable.

A future improvement may add a normalized baseline format that can be
committed. Until then, the human review described above is the regression
mechanism.

## Profile output

Each run writes per-scenario profiles to `target/gungraun/`. To inspect a
specific scenario's hottest functions:

```bash
callgrind_annotate target/gungraun/<package>/<bench>/<scenario>/callgrind.out
```

Or load the same file in [KCachegrind][kcachegrind] for a GUI view of the
call graph.

[criterion]: https://github.com/bheisler/criterion.rs
[valgrind]: https://valgrind.org/
[callgrind]: https://valgrind.org/docs/manual/cl-manual.html
[gungraun]: https://crates.io/crates/gungraun
[kcachegrind]: https://kcachegrind.github.io/
