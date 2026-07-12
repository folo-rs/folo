# cargo-bench-history

A Cargo subcommand that maintains a long-lived history of benchmark results and
analyzes it for trends that snapshot-only tooling cannot see — for example
"benchmark X has been getting incrementally slower over the past 12 months" or
"scenario Y regressed after commit Z, visible only in hindsight against noisy
data".

Most benchmark tooling only reports the current run, or at best compares against
the previous local run. `cargo-bench-history` instead stores **every** run as an
immutable record — on the local filesystem or in an
Azure Blob container — and reconstructs per-benchmark series in git first-parent
commit order, so historical trends become analyzable.

Where results are stored is chosen at run time: pass `--local=<path>` for local
filesystem storage (or a bare `--local` to take the path from the
`CARGO_BENCH_HISTORY_STORAGE` environment variable), or configure an Azure Blob
backend in `.cargo/bench_history.toml` and omit `--local`. A local path is
machine-dependent, so it is never stored in the shared config file.

## Installation

Install with [`cargo binstall cargo-bench-history`](https://github.com/cargo-bins/cargo-binstall)
to fetch a prebuilt binary on supported targets (transparently building from source
elsewhere), or `cargo install cargo-bench-history` to always build from source.

```text
# Write a starter .cargo/bench_history.toml (documents the optional cloud backend).
cargo bench-history install

# Run the workspace benchmarks for the current commit and store the results
# locally. Drop --local to store in the cloud backend from the config file.
cargo bench-history collect --local=./bench-history

# On a noisy runner, run the suite a few times and keep the best (minimum) value
# per metric, since benchmark interference only ever makes a case slower.
cargo bench-history collect --local=./bench-history --best-of 3

# Bootstrap history by benching a range of past commits, so analysis has a
# trend to work with (a single run on its own has nothing to compare against).
cargo bench-history backfill --local=./bench-history <from-commit> <to-commit>

# Analyze the recorded history for regressions and drift.
cargo bench-history analyze --local=./bench-history

# Inspect the raw per-commit data points behind a finding, to correlate a change
# with the commit that caused it (both --benchmark and --metric are required).
cargo bench-history examine --local=./bench-history \
    --benchmark my_pkg/my_group/my_case --metric instruction_count

# Print this machine's hardware fingerprint (the key hardware-dependent history is
# partitioned by). --verbose additionally reports the factors behind the key on
# standard error, for tracing a key change to the hardware detail that moved.
cargo bench-history machine-key
```

## See also

The [user guide](https://folo-rs.github.io/folo/cargo-bench-history/) walks through every command,
the storage backends and the analysis model in depth.

More details in the [package documentation](https://docs.rs/cargo-bench-history/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
