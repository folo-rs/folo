# Naming conventions

This document codifies naming conventions for files and identifiers across the
workspace. See `AGENTS.md` for the broader set of conventions; this file focuses
on naming patterns that need to be consistent across packages.

## Disambiguating names must be distinctive, not merely different

When two related things need different names, choose names that are *distinctive*
— each name states what makes that variant special — rather than names that are
just technically different. A reader should be able to tell the variants apart
without already knowing the system. For example, a test account and a production
account should be `..._TEST_...` and `..._PROD_...`, not `..._ACCOUNT` and
`..._DATA_ACCOUNT` (different, but neither says "test" nor "prod"); two infra
directories should be `azure-bench-history-test` and `azure-bench-history-prod`,
not `azure-bench-history` and `azure-bench-history-data`. Apply this to
variables, files, directories, and any other paired identifiers.

## Benchmark naming

The workspace contains both Criterion (wall-clock) and Callgrind/Gungraun
(simulated instruction-count) benchmarks. Both kinds live in each package's
`benches/` directory. The naming rules below ensure benchmarks are easy to
locate, easy to pair across the two systems, and produce non-colliding binary
names in the shared `target/.../deps/` directory.

### Benchmark file names

Every file in `packages/<pkg>/benches/` must be prefixed with either:

* The full name of the package (e.g. `foo/benches/foo_something.rs`), or
* An abbreviated form of the full package name (e.g.
  `foo_bar_baz/benches/fbb_something.rs`).

For packages that follow the `_impl` crate split pattern (see
[`docs/impl-crate-split.md`](impl-crate-split.md)), the `_impl` suffix is
omitted from the benchmark file prefix (e.g. `nm_impl/benches/nm_observe.rs`,
not `nm_impl_observe.rs`).

The crate-name prefix prevents collisions when the resulting bench binaries
land in the shared `target/.../deps/` directory.

Do not append decorator suffixes like `_bench` or `_benches` — the `benches/`
directory location and `[[bench]]` Cargo table entry already convey that a
file is a benchmark.

### Criterion benchmark groups

Every Criterion `benchmark_group()` call must use a name of the form:

```
<file-basename>/<subgroup>
```

where `<file-basename>` is the benchmark file name without the `.rs`
extension and `<subgroup>` is a short semantic identifier for the group.

Examples for `foo/benches/foo_something.rs`:

```rust
let mut group = c.benchmark_group("foo_something/reads");
// ...
let mut group = c.benchmark_group("foo_something/writes");
```

The `<file-basename>/` prefix means a benchmark's fully qualified Criterion
path (`<group>/<benchmark>`) starts with the file basename, making it
trivial to locate the source file from a benchmark report.

Avoid `::` or other custom separators between segments — Criterion already
uses `/` as the hierarchical separator and that is the only separator that
should appear in group names.

### Tracking-session operation names must equal the Criterion identifier

> **This is the naming rule most easily overlooked. Read it before touching any
> benchmark that tracks allocations or processor time.**

Many Criterion benchmarks pair their timing with an
[`alloc_tracker`](../packages/alloc_tracker) or
[`all_the_time`](../packages/all_the_time) tracking session. Each tracked
operation is registered with a call such as:

```rust
let op = allocs.operation("<name>");
```

The `<name>` string **must be identical to the fully qualified Criterion
benchmark identifier** it measures — that is:

```
<criterion group name>/<bench_function name>
```

which, given the group-naming rule above, always expands to
`<file-basename>/<subgroup>/<bench_function name>`. Note that
`<bench_function name>` is the exact string passed to `bench_function(...)` and
may itself contain `/` separators (for example `events/embedded/AutoResetEvent`);
reproduce it in full rather than appending only its final segment. The tracking
name is not free-form: it is the join key that lets the emitted allocation/time
report line up one-to-one with the Criterion report for the same benchmark. A
mismatched name silently produces two reports that cannot be correlated, which
is worse than useless.

Example for `foo/benches/foo_something.rs`:

```rust
let mut group = c.benchmark_group("foo_something/reads"); // <group>
let op = allocs.operation("foo_something/reads/warm_cache"); // <group>/<bench_function name>
group.bench_function("warm_cache", |b| { /* ... */ });      // <bench_function name>
```

Watch out for these traps:

* **Use the `bench_function` name, not a paraphrase.** The operation leaf must
  be the exact string passed to `bench_function(...)`. When the two were
  written to differ (e.g. an operation called `empty_thread_span` measuring a
  bench named `thread_span_empty`), the operation name must be corrected to
  match the bench — the join key is the Criterion identifier, never the old
  operation label.
* **Include every segment.** Missing the `<file-basename>/` prefix (or any
  intermediate segment the `bench_function` name carries) breaks the match.
  The name must reproduce the Criterion identifier in full, even where that
  makes a segment look redundant.

**Exception — operations that name the workload under test.** When a benchmark
exercises a tracking session *as the thing being measured* (for example a
report-lifecycle bench that populates a session with synthetic operations), the
`operation(...)` calls on that inner session name the workload, not the
enclosing Criterion benchmark, and are deliberately independent of the Criterion
identifier (e.g. `session.operation(format!("op_{op_idx}"))`). Only the
*outer* tracking session that correlates with the Criterion report follows the
rule above.

### Callgrind benchmark files

Callgrind benchmark files (suffix `_cg.rs`) live alongside the Criterion
files in the same `benches/` directory.

**Pairing requirement**: every Callgrind benchmark file must have a matching
Criterion benchmark file in the same directory. The Criterion file's name
equals the Callgrind file's name minus the `_cg` suffix:

```
packages/<pkg>/benches/<base>.rs       (Criterion, required)
packages/<pkg>/benches/<base>_cg.rs    (Callgrind, optional)
```

For example, `region_local_cg.rs` pairs with `region_local.rs`. The reverse
is not required: a Criterion file may exist without a Callgrind counterpart,
because Criterion legitimately stands alone for multithreaded contention,
syscalls, allocation, and bulk-throughput scenarios where instruction-count
resolution adds no signal. See [`docs/callgrind-benchmarks.md`](callgrind-benchmarks.md)
for the full Callgrind strategy.

### Callgrind benchmark groups and functions

Callgrind does not surface its `library_benchmark_group!` identifier as a
first-class concept in its output — what matters is the benchmark function
name. Callgrind benchmark function names mirror the equivalent Criterion
benchmark identifier with the filename prefix omitted and `/` substituted by
`_` (Rust identifiers cannot contain `/`).

For a Criterion benchmark identified by `<file-basename>/<subgroup>/<bench>`
in `<base>.rs`, the matching Callgrind benchmark in `<base>_cg.rs` is:

* `library_benchmark_group!(name = <subgroup>, ...)`
* `fn <subgroup>_<bench>() { ... }` (the benchmark function inside that group)

The `library_benchmark_group!` identifier holds the subgroup name (without
the file basename prefix). The function name encodes both the subgroup and
the benchmark name, joined by `_`. This way the function name alone uniquely
identifies the benchmark within the file — the group is just structural
scaffolding required by the Gungraun macros.

Example pairing:

```rust
// Criterion: region_local/benches/region_local.rs
let mut group = c.benchmark_group("region_local/read");
group.bench_function("get_local_warm", |b| { /* ... */ });
group.bench_function("with_local_warm", |b| { /* ... */ });
```

```rust
// Callgrind: region_local/benches/region_local_cg.rs
#[library_benchmark]
fn read_get_local_warm() -> u32 { /* ... */ }

#[library_benchmark]
fn read_with_local_warm() -> u32 { /* ... */ }

library_benchmark_group!(
    name = read,
    benchmarks = [read_get_local_warm, read_with_local_warm]
);
```

The Criterion benchmark `region_local/read/get_local_warm` corresponds
one-to-one with the Callgrind benchmark `read::read_get_local_warm`.

The set of Callgrind subgroup names in a `_cg.rs` file must be a subset
of the Criterion subgroup names in the paired `.rs` file. A Callgrind
file may omit subgroups that do not warrant instruction-count coverage
(for example multithreaded contention or allocation-throughput scenarios
where Callgrind adds no signal), but it must not introduce subgroup
names that do not appear in the Criterion file. This keeps the two
views of the same package navigable side-by-side.

Within a matched subgroup, individual Callgrind benchmark functions need
not correspond 1:1 to Criterion bench functions — Callgrind frequently
isolates additional variants (first-touch vs warm, cache-cold vs hot,
empty vs populated) that the Criterion bench does not measure.
