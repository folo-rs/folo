# collect

`collect` invokes the workspace's benches with `cargo bench` and harvests whichever engines
produced output — there is no engine configuration. It enables the combined environment
every supported engine needs (only Callgrind needs an opt-in variable; the others
auto-emit) and then inspects each output tree to see which engines actually ran. Off-Linux,
the Callgrind benches compile to no-ops and simply produce nothing.

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

`collect` always persists — there is no separate publish step (`--no-store` runs without
writing). A clean point writes a deterministic key, refused by default if it already exists;
overwrite to replace, or skip-existing to treat it as a success and write nothing (the
append-only mode CI uses). A dirty working tree writes a snapshot that coexists with prior
snapshots. An engine that harvests zero cases stores nothing.

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
