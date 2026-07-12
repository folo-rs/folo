# Getting started

This walkthrough uses local filesystem storage. For the cloud backend, see
[Storage backends](storage.md).

```console
# Write a starter .cargo/bench_history.toml (documents the optional cloud backend).
cargo bench-history install

# Run the workspace benchmarks for the current commit and store the results locally.
# Drop --local to store in the cloud backend from the config file.
cargo bench-history collect --local=./bench-history

# On a noisy runner, run the suite a few times and keep the best (minimum) value per
# metric, since benchmark interference only ever makes a case slower.
cargo bench-history collect --local=./bench-history --best-of 3

# Bootstrap history by benching a range of past commits, so analysis has a trend to work
# with (a single run on its own has nothing to compare against).
cargo bench-history backfill --local=./bench-history <from-commit> <to-commit>

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
from *history*. `backfill` seeds that history from existing commits, after which `analyze`
can reconstruct each benchmark's series in git first-parent order and report level shifts
and slow drift.

For why a single run is not enough, and how series are ordered and compared, see
[Comparability and partitioning](concepts/comparability.md) and [Analysis](concepts/analysis.md).
