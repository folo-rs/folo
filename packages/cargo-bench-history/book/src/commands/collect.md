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

## Storage behaviour

By default, `collect` persists immediately — there is no separate publish step.
`--no-store` is the explicit dry-run exception. A clean point writes a deterministic key and
is refused by default if it already exists. Use `--overwrite` to replace it, or
`--skip-existing` to treat the duplicate as a success and write nothing (the append-only
mode CI uses). A dirty working tree writes a snapshot that coexists with prior snapshots. An
engine that harvests zero cases stores nothing.

Two non-overlapping partial runs at one commit do **not** merge — each writes the same clean
key and the second collides. Coverage gaps are expected to come from *different commits*
covering different subsets, not from multiple partial runs at one commit.

## Scope and passthrough

Scope flags (`--workspace`, `--package`, `--exclude`, `--bench`) and cargo feature flags
translate directly to `cargo bench` arguments, and everything after `--` is forwarded
verbatim.

## Effective partition line

Regardless of `--verbose`, `collect` prints a one-line effective-partition summary to
stderr naming the storage partition its results land in: the target triple and the machine
key that hardware-dependent engines use, marked as auto-detected or as coming from
`--machine-key`.
