# Commands

The user-facing commands follow the lifecycle of benchmark history: set up storage, record
runs, analyze what changed, inspect the evidence, and maintain the stored data. Run
`cargo bench-history <command> --help` for the full, current flag list.

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

Option names stay consistent across commands: storage and repository selection, output
formats, benchmark scope, Cargo features, discriminant sets, commit ranges, and data
filters mean the same thing wherever they appear. Some commands also take a bare subject:
`runs|discriminants|blessings` for `list`, benchmark prefixes for `bless`, commits for
`prune`, and range endpoints for `backfill`.

## Selection lockstep

`analyze`, `list runs`, `prune`, and `examine` apply the same selection rules. You can
therefore preview a range with `list runs`, inspect one of its series with `examine`, or
prune it without learning a second set of filters. The analysis-only flags
(`--include-improvements`, `--include-inactive`) and condensed Markdown summary are
exceptions because the other commands select data but never detect findings.
