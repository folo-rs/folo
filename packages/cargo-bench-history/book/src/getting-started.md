# Getting started

This walkthrough uses local filesystem storage. For the cloud backend, see
[Storage backends](storage.md).

> **Before you begin**
> `cargo-bench-history` records whatever your existing benchmarks emit — it does not create
> benchmarks for you. Your project needs at least one benchmark a supported engine can
> harvest (most commonly [Criterion](https://bheisler.github.io/criterion.rs/book/) benches
> under `benches/`), or `collect` runs but stores nothing. See
> [Benchmark engines](concepts/engines.md) for the full list.

```console
# Write a starter .cargo/bench_history.toml (documents the optional cloud backend).
cargo bench-history install

# Bootstrap history from completed commits. Ending at HEAD~1 leaves the current commit for
# the collect step below; a single run on its own has nothing to compare against.
cargo bench-history backfill --local=./bench-history <from-commit> HEAD~1

# Run the workspace benchmarks for the current commit and add the results to that history.
# Drop --local to store in the cloud backend from the config file.
cargo bench-history collect --local=./bench-history

# On a noisy runner, use this instead of the preceding collect command to run the suite
# three times and keep the best (minimum) value per metric.
# cargo bench-history collect --local=./bench-history --best-of 3

# Analyze the recorded history for regressions and drift.
cargo bench-history analyze --local=./bench-history

# Inspect the raw per-commit data points behind a finding, to correlate a change with the
# commit that caused it (both --benchmark and --metric are required).
cargo bench-history examine --local=./bench-history \
    --benchmark my_pkg/my_group/my_case --metric instruction_count

# Print this machine's hardware fingerprint (the key that hardware-dependent history is
# partitioned by).
cargo bench-history machine-key
```

## What just happened

A single `collect` on its own has nothing to compare against — the value of the tool comes
from *history*. `backfill` seeds that history from existing commits, `collect` adds the
current commit, and `analyze` reconstructs each benchmark's series in git first-parent order
to report level shifts and slow drift.

For why a single run is not enough, and how series are ordered and compared, see
[Comparability and partitioning](concepts/comparability.md) and [Analysis](concepts/analysis.md).
