# collect

`collect` runs the selected benches with `cargo bench`, automatically harvests every
supported engine that produced output, and stores one result for the current commit. You
select packages, bench targets, and Cargo features rather than an engine; engines that did
not run simply contribute no data. Outside Linux, Callgrind benches compile to no-ops and
produce nothing.

```console
# Store locally.
cargo bench-history collect --local=./bench-history

# Run the suite N times and keep the per-metric minimum (noise reduction).
cargo bench-history collect --local=./bench-history --best-of 3

# Dry run: benchmark everything but write nothing.
cargo bench-history collect --no-store
```

## `--best-of N`

`--best-of N` reruns the whole suite `N` times (default `1`) and stores, per metric, the
**minimum** observed value. Benchmark interference on a shared runner is one-sided — it only
ever makes a case slower — so the minimum discards a transient slowdown. Every run must
measure the same set of cases and the same metrics per case; any cross-run mismatch is a
hard error.

Two caveats: a runner that is slow for the *entire* job is not corrected by the minimum, and
Callgrind's deterministic counts make min-of-N a costly no-op for that engine.

Because the reduction keeps a minimum, `N` is part of the measurement protocol, not just a
speed/quality knob: the expected minimum of `N` samples falls as `N` rises, so changing `N`
can shift the recorded level of the whole suite at once. It moves a metric only to the extent
that metric is noisy — a deterministic one is unaffected, as with Callgrind above, and any
individual observation may land unchanged. Keep `N` fixed for a given machine
and project if you want the history to stay a like-for-like record. Every stored run records
the count it was reduced from, so a value's protocol is always recoverable from the stored
data even if the setting changes.

## Storage behavior

By default, `collect` persists immediately — there is no separate publish step.
`--no-store` is the explicit dry-run exception. A clean point writes a deterministic key and
is refused by default if it already exists. Use `--overwrite` to replace it, or
`--skip-existing` to treat the duplicate as a success and write nothing (the append-only
mode CI uses). A dirty working tree writes a snapshot that coexists with prior snapshots. An
engine that harvests zero cases stores nothing.

Two non-overlapping partial runs at one commit do **not** merge — each writes the same clean
key and the second collides. Coverage gaps are expected to come from *different commits*
covering different subsets, not from multiple partial runs at one commit.

## Capturing this execution for analysis

`--collection-output PATH` writes a self-contained JSON snapshot of this successful
collection's exact measurements and identities. It requires a clean commit and a new output
file, and is incompatible with `--no-store`. Keep the output outside the measured checkout
or in an ignored directory.

Collection always executes the selected benchmarks. With `--skip-existing`, an existing
shared-history object remains unchanged, while the snapshot contains the newly measured
values, not the stored values. A successful collection with no measurements still writes
an empty snapshot.

Pass snapshots to [`analyze --current-collection`](analyze.md#exact-current-collections)
when analysis must describe this execution rather than any other collection at the same commit.

## Scope and passthrough

Scope flags (`--workspace`, `--package`, `--exclude`, `--bench`) and cargo feature flags
translate directly to `cargo bench` arguments, and everything after `--` is forwarded
verbatim.

## Effective partition line

Regardless of `--verbose`, `collect` prints a one-line effective-partition summary to
stderr naming the storage partition its results land in: the target triple and the auto-detected
machine key every engine is partitioned by.
