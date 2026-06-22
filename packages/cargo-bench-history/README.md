# cargo-bench-history

A Cargo subcommand that maintains a long-lived history of benchmark results and
analyzes it for trends that snapshot-only tooling cannot see — for example
"benchmark X has been getting incrementally slower over the past 12 months" or
"scenario Y regressed after commit Z, visible only in hindsight against noisy
data".

Most benchmark tooling only reports the current run, or at best compares against
the previous local run. `cargo-bench-history` instead stores **every** run as an
immutable record — on the local filesystem or, with the `azure` feature, in an
Azure Blob container — and reconstructs per-benchmark series in git first-parent
commit order, so historical trends become analyzable.

```text
# Write a starter .cargo/bench_history.toml.
cargo bench-history install

# Run the workspace benchmarks and store the results.
cargo bench-history run

# Analyze the recorded history for regressions and drift.
cargo bench-history analyze
```

## See also

More details in the [package documentation](https://docs.rs/cargo-bench-history/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
