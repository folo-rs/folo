# Commands

`cargo-bench-history` exposes nine commands. Every option is filed under a named help
heading, and option groups are shared across commands, so a given group looks identical
everywhere it appears. Run `cargo bench-history <command> --help` for the full, current
flag list.

| Command | Purpose |
|---|---|
| [`install`](install.md) | Write a starter `.cargo/bench_history.toml`. |
| [`collect`](collect.md) | Run the workspace benches for the current commit and store the results. |
| [`backfill`](backfill.md) | Reconstruct history by benching a range of past commits. |
| [`analyze`](analyze.md) | Reconstruct series and report regressions and drift. |
| [`examine`](examine.md) | Pivot a single `(benchmark, metric)` series into its raw per-commit points. |
| [`list`](list.md) | Preview the data set an `analyze` pass would consume, or catalog stored partitions. |
| [`prune`](prune.md) | Delete a chosen portion of the stored data set. |
| [`bless` / `unbless`](bless.md) | Accept (or un-accept) an intentional performance change on the base branch. |
| [`machine-key`](machine-key.md) | Print this machine's hardware fingerprint. |

## Shared option groups

The functional groups are: environment and execution, output, benchmark scope, feature
selection, discriminant selection, commit selection, and data filtering. Subjects are bare
positional words, never flags — the `runs|discriminants|blessings` selector for `list`,
prefixes for `bless`, commits for `prune`, and the range endpoints for `backfill`.

## Selection lockstep

`analyze`, `list`, `prune`, and `examine` share one data-set-selection pipeline, so the
same selection flags mean the same thing across all four. The analysis-only flags
(`--include-improvements`, `--include-inactive`) and the analyze-only
condensed Markdown summary are exceptions: only `analyze` detects; the others reuse the
selection but never analyze.
