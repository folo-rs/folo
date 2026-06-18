# cargo-bench-history

A Cargo subcommand that maintains a long-lived history of benchmark results and
analyzes it for trends that snapshot-only tooling cannot see — for example
"benchmark X has been getting incrementally slower over the past 12 months" or
"scenario Y regressed after commit Z, visible only in hindsight against noisy
data".

## How it works

Most benchmark tooling only reports the current run, or at best compares against
the previous local run. `cargo-bench-history` instead stores **every** run as an
immutable record (on the local filesystem or, with the `azure` feature, in an
Azure Blob container) and reconstructs per-benchmark series in git first-parent
commit order, so historical trends become analyzable.

Results are partitioned only by what makes them fundamentally incomparable —
project, engine system, target triple, and (for hardware-dependent engines) a
machine key. Everything else (toolchain version, OS, commit, CI provider) is
recorded as metadata so its effect stays visible as a step in the timeline rather
than forking history.

The initial target engines are the ones this workspace uses: Criterion
(wall-clock) and Callgrind via Gungraun (simulated instruction counts).

## Commands

```text
cargo bench-history run [--workspace] [--package NAME] [--bench NAME]
                        [--timestamp RFC3339] [--target-triple TRIPLE]
                        [--machine-key KEY] [--no-store] [--overwrite]
                        [--verbose] [--config PATH]
                        -- <args forwarded to cargo bench>
cargo bench-history install [--config PATH]
cargo bench-history backfill --from REF --to REF [--workspace]
                             [--package NAME] [--bench NAME]
                             [--target-triple TRIPLE] [--machine-key KEY]
                             [--overwrite] [--ignore-errors] [--verbose]
                             [--config PATH]
                             -- <args forwarded to cargo bench>
cargo bench-history analyze [--repo PATH] [--branch REF] [--base REF]
                            [--engine NAME] [--os OS] [--architecture ARCH]
                            [--machine-key KEY] [--no-dirty]
                            [--list-discriminants] [--since DATE]
                            [--metric NAME] [--format text|json|markdown]
                            [--fail-on-regression] [--config PATH]
```

* `run` executes the workspace's benches once with `cargo bench`, harvests every
  supported engine's output, and stores a result set per engine. There is nothing
  to configure about engines: the run enables the combined environment Criterion
  and Callgrind need and detects each engine from the output it produces (off
  Linux the Callgrind benches compile to no-ops, so only Criterion is stored).
  Scope the run with `--workspace` (the default), `--package`/`-p NAME`, or
  `--bench NAME`; everything after `--` is forwarded verbatim to `cargo bench`.
  History is keyed by commit: a clean run writes a single deterministic object per
  commit, so re-running the same commit is refused unless `--overwrite` replaces
  the stored result. `--timestamp` overrides the effective time when backfilling
  history for an old commit; `--machine-key` overrides the hardware fingerprint
  used to partition hardware-dependent (Criterion) results. `--verbose` prints a
  step-by-step diagnostic trail to standard error (the benchmark command and
  injected environment, every directory scanned, which output files were included
  or skipped as stale, and where each result was stored) — useful when a run
  unexpectedly stores nothing.
* `install` generates a starter `.cargo/bench_history.toml` if absent, printing
  its path and next steps (including how to `backfill` history for an existing
  repository); an existing file is never overwritten.
* `backfill` replays `run` across the inclusive commit range `--from..--to`,
  bootstrapping history for a repository that adopted the tool late. Each commit
  is checked out in a dedicated git **worktree** (the primary checkout is never
  touched) and benched there, recording the commit's committer date as the
  effective time. The range must lie on the current branch's first-parent history
  and the working tree must be clean. Already-stored commits are skipped (so
  backfill is resumable) unless `--overwrite` replaces them; a commit that fails to
  build or benchmark stops the run unless `--ignore-errors` continues past it.
  `--workspace`/`--package`/`--bench`/`--target-triple`/`--machine-key`/`--verbose`
  and a `--` passthrough behave as for `run`.
* `analyze` reconstructs a timeline from git history and reports notable patterns.
  It requires a repository (`--repo` selects one other than the current directory).
  `--branch` chooses the line to analyze (default `HEAD`) and `--base` the line to
  branch from (default: the configured or detected default branch); commits up to
  the merge-base contribute clean runs only, while commits unique to the analyzed
  branch also contribute dirty snapshots unless `--no-dirty` is given. The history
  is partitioned into *discriminant sets* (engine, target triple, OS, architecture,
  machine key); `--engine`/`--os`/`--architecture`/`--machine-key` select sets and
  `--list-discriminants` prints the sets present in storage. `--since`/`--metric`
  narrow the data and `--fail-on-regression` enables CI gating.

## Status

Implemented:

* `run` executes Callgrind (via Gungraun) and Criterion, harvests their output
  (`target/gungraun/**/summary.json` and `target/criterion/**/new/*.json`), and
  stores one immutable result set per engine per run. Callgrind results are
  hardware-independent (`synthetic` partition); Criterion results are partitioned
  by the host target triple and a machine-key hardware fingerprint.
* `analyze` reconstructs a project's timeline from git history and reports
  rolling-baseline regressions/improvements in `text`, `json`, or `markdown`,
  grouped by discriminant set, with optional `--fail-on-regression` CI gating.
* `install` writes a starter `.cargo/bench_history.toml` when one is absent.
* `backfill` replays `run` across a commit range in isolated git worktrees,
  bootstrapping history for old commits; it is resumable (skips already-stored
  commits) and supports `--overwrite` and `--ignore-errors`.
* Storage backends: the local filesystem, or Azure Blob storage behind the
  `azure` feature.

See `DESIGN.md` for the full design and iteration plan; noise-aware statistical
findings (change-point and drift detection) are the remaining work.
