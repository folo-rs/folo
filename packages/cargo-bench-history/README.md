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
cargo bench-history install [--verbose] [--config PATH]
cargo bench-history backfill --from REF --to REF [--workspace]
                             [--package NAME] [--bench NAME]
                             [--target-triple TRIPLE] [--machine-key KEY]
                             [--overwrite] [--ignore-errors] [--verbose]
                             [--config PATH]
                             -- <args forwarded to cargo bench>
cargo bench-history analyze [--repo PATH] [--branch REF] [--base REF]
                            [--engine NAME] [--target-triple TRIPLE]
                            [--os OS] [--architecture ARCH]
                            [--machine-key KEY] [--no-dirty] [--since WHEN]
                            [--metric NAME] [--format text|json|markdown]
                            [--mode auto|history|branch|tip]
                            [--include-improvements] [--verbose] [--config PATH]
cargo bench-history list [--discriminants] [--repo PATH] [--branch REF]
                         [--base REF] [--engine NAME] [--target-triple TRIPLE]
                         [--os OS] [--architecture ARCH] [--machine-key KEY]
                         [--no-dirty] [--since DATE] [--metric NAME]
                         [--format text|json|markdown] [--verbose] [--config PATH]
cargo bench-history clean [--dry-run] [--repo PATH] [--branch REF]
                          [--base REF] [--engine NAME] [--target-triple TRIPLE]
                          [--os OS] [--architecture ARCH] [--machine-key KEY]
                          [--since DATE] [--format text|json|markdown]
                          [--verbose] [--config PATH]
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
  touched, so a dirty working tree is fine) and benched there, recording the
  commit's committer date as the effective time. The range must lie on the current
  branch's first-parent history. Before benchmarking, commits that already have a
  stored result are skipped without re-running their benchmarks (so backfill is
  resumable and cheap to re-run); `--overwrite` re-benchmarks and replaces every
  commit in the range. A commit that fails to build or benchmark stops the run
  unless `--ignore-errors` continues past it.
  `--workspace`/`--package`/`--bench`/`--target-triple`/`--machine-key`/`--verbose`
  and a `--` passthrough behave as for `run`.
* `analyze` reconstructs a timeline from git history and reports notable patterns.
  It requires a repository (`--repo` selects one other than the current directory).
  `--branch` chooses the line to analyze (default `HEAD`) and `--base` the line to
  branch from (default: the configured or detected default branch); commits up to
  the merge-base contribute clean runs only, while commits unique to the analyzed
  branch also contribute dirty snapshots unless `--no-dirty` is given (one
  exception: if your working tree is currently dirty while you are on the base
  branch, that branch tip's dirty snapshots are included, and the report warns that
  this data is ephemeral — switch to a feature branch to persist it). The history
  is partitioned into *discriminant sets* (engine, target triple, OS, architecture,
  machine key); `--engine`/`--os`/`--architecture`/`--machine-key` select sets.
  `--target-triple`
  selects by the whole triple instead of `--os`/`--architecture` and cannot be
  combined with them.
* `analyze` runs in one of three **modes**, chosen automatically (`--mode auto`,
  the default) or forced with `--mode`:
  * **history** — long-range trend watch over the base branch (selected when you
    analyze a clean checkout of the base branch). It detects sustained
    change-points and slow drifts, defaults `--since` to the last six months so a
    scheduled watch does not silently widen as history grows, and reports only
    **regressions** (steady improvement over time is expected) unless
    `--include-improvements` is given.
  * **branch** — "how does my feature compare" (selected for a feature branch, or a
    dirty base checkout). It judges the branch by its **latest** state versus the
    base, reporting both regressions and improvements; if the branch got better and
    then worse, the latest (worse) state is what is reported, and the finding's
    `flipped_at` names the commit that regime began at.
  * **tip** — a fast guard that compares only the latest commit against the
    recently established level, reporting regressions only. Useful as a cheap
    "did the last commit make things worse" check.
  Findings are advisory: the exit code reflects only whether the analysis *ran*,
  never what it found. Downstream automation reads the machine-readable signal from
  the `json` report — `mode`, the boolean `notable` (any finding survived), each
  finding's `direction`/`flipped_at`, and the full per-finding `series` for
  charting. The `text` report renders one paragraph per finding, leading with the
  relative-change percent, and in **history** mode draws a small colored line chart
  of the series over commits; values in the `text`/`markdown` reports are rounded to
  four significant figures while the `json` report keeps full precision. `--since`
  accepts an RFC 3339 timestamp, a `YYYY-MM-DD` date, or a
  relative duration such as `6 months` or `30 days ago`; `--metric` narrows to one
  metric. When stored runs exist but none enter the analysis (for example because
  every run is a dirty snapshot on the base branch), the report explains why;
  `--verbose` adds a per-object diagnostic trail to standard error. Every command
  accepts `--verbose`.
* `list` previews the data set that an `analyze` pass would consume without
  analyzing it: it accepts the same data-set-selection flags as `analyze`
  (`--repo`/`--branch`/`--base`/`--engine`/`--target-triple`/`--os`/
  `--architecture`/`--machine-key`/`--no-dirty`/`--since`/`--metric`) and reports,
  per discriminant set, the run, series, and per-commit counts of the selected
  runs. `--discriminants` instead lists the discriminant sets present in storage
  (no repository required) — useful for discovering which engines, triples, and
  machine keys have data before scoping an analysis.
* `clean` removes the dirty (uncommitted-tree) runs from the same commits a
  matching `analyze`/`list` pass would draw them from: the commits unique to the
  analyzed branch, or the base branch's tip commit when you are on the base
  branch. It accepts the same data-set-selection flags as `analyze` (except
  `--no-dirty`/`--metric`, which do not apply) and deletes nothing else — clean
  runs are always left intact. Unlike `analyze`/`list`, the base-branch tip's
  dirty runs are removed unconditionally (not only when the working tree is
  currently dirty), so `clean` reclaims ephemeral snapshots regardless of the
  current tree state. `--dry-run` previews exactly what would be removed without
  deleting anything.

## Status

Implemented:

* `run` executes Callgrind (via Gungraun) and Criterion, harvests their output
  (`target/gungraun/**/summary.json` and `target/criterion/**/new/*.json`), and
  stores one immutable result set per engine per run. Callgrind results are
  hardware-independent (`synthetic` partition); Criterion results are partitioned
  by the host target triple and a machine-key hardware fingerprint.
* `analyze` reconstructs a project's timeline from git history and reports
  engine-aware, noise-resistant findings — sustained **change-points** and slow
  **drifts**, separated from measurement jitter — in `text`, `json`, or `markdown`,
  grouped by discriminant set. It runs in `history`, `branch`, or `tip` mode
  (auto-detected, or forced with `--mode`); findings are advisory and never affect
  the exit code (the `json` report's `mode`/`notable`/`flipped_at`/`series` fields
  are the downstream signal).
* `list` previews the data set an `analyze` pass would consume (run/series/commit
  counts per discriminant set), or lists the discriminant sets present in storage
  with `--discriminants`.
* `clean` removes dirty runs from the analyzed branch's commits (or the base
  branch tip), with a `--dry-run` preview.
* `install` writes a starter `.cargo/bench_history.toml` when one is absent.
* `backfill` replays `run` across a commit range in isolated git worktrees,
  bootstrapping history for old commits; it is resumable (skips already-stored
  commits) and supports `--overwrite` and `--ignore-errors`.
* Storage backends: the local filesystem, or Azure Blob storage behind the
  `azure` feature.

See `DESIGN.md` for the full design and iteration plan.
