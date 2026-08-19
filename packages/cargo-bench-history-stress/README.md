# cargo-bench-history-stress

An **on-demand** stress harness for [`cargo-bench-history`](../cargo-bench-history)'s
`analyze` command. It fabricates a giant synthetic benchmark history, seeds it into a
storage backend, then times each analysis mode (`history`, `branch`) over it.

The dataset is *invented*, not measured. The point is to put the real `analyze`
data-loading and detection path under a realistic, large-scale load so the per-mode
wall-clock cost can be observed — against either local-filesystem or Azure Blob
storage.

This package is `publish = false`. A *full-scale* run is on-demand only — you launch
it by hand (`just bench-history-stress` / `just bench-history-stress-azure`) when you
want to know how `analyze`
scales, so it never runs automatically in `just test` or CI. The package's own small
unit and integration tests, which exercise the harness at tiny sizes, do run as a
normal workspace member under `just test`, CI, mutation testing, and coverage.

## What it builds

By default the harness fabricates:

* **1000 benchmarks** per discriminant set,
* across **2000 first-parent `main` commits** spread over the past ~12 months, of which
  roughly **half store a run** — every other commit is a gap, exercising the realistic
  "commit with no run" path,
* in **every supported engine crossed with the platforms it runs on** — `callgrind` on
  the two Linux triples only (it drives Valgrind), and `criterion`, `alloc_tracker`, and
  `all_the_time` on all of `{windows, linux, macos} × {x64, arm}`, for **20 discriminant
  sets** in total,
* plus a short **feature branch** (6 commits) with a few **dirty** (uncommitted-tree)
  snapshots on its tip,
* and **blessings** ~75% of the way back for one benchmark family, applied in some
  discriminant sets but not others.

That is roughly 20000 stored objects whose JSON totals ~5.7 GiB uncompressed.
The storage layer gzip-compresses every object, so the actual on-disk/wire
volume — the quantity the harness measures and reports — is roughly an order of
magnitude smaller. Sizes are all overridable (see flags), so
`--commits 100 --benchmarks 100` gives a quick smoke run.

It then reads the data back through the real public
`cargo_bench_history::run_with_overrides` entry point — the exact production path —
and reports how long each requested mode took.

### Why the values are shaped the way they are

The dataset spans every engine, so it exercises both detection paths. The injected
timeline shapes are sized in *relative* terms, so they read the same whichever metric an
engine records: **deterministic** engines (`callgrind` instruction counts, `alloc_tracker`
allocation counts) store an exact value with no dispersion, so any non-zero step is
detected exactly; **noisy** engines (`criterion` wall time, `all_the_time` processor time)
store the same shape plus a tight confidence band — kept well below the injected step
magnitudes — so the seeded findings still surface while the noise-aware detection path is
exercised too. Each benchmark belongs to a *family* (`index % 5`) that fixes its
timeline shape (gradual drift, mid-step up, step down, blessable step, stable), and a
cross-cutting rule injects a `branch`-only change. The result
is that each mode reports a sensible, explainable *subset* of the seeded shapes
rather than flagging everything or nothing. See the module docs in
`src/scenario.rs` for the exact family/divisor math.

A seeded shape only becomes a *finding* once the history carries the evidence the
analysis gates demand: a series is judged at all only from the detectors' minimum
number of points, and a step is trusted only when a full regime sits on each side of
it. Roughly half the commits carry a run, so a `--commits` below twice the minimum
series length produces no findings at all, and for a stretch above that only part of
the seeded set surfaces. Sizing at eight times the minimum regime (currently
`--commits 40`) sits clear of that stretch, which is what the smoke tests seed.

A given `--seed` and sizing reproduce a **byte-identical** dataset (fixed dataset
anchor + SplitMix64 generator), so timings are comparable across runs. The seeded
shapes are relative to each series' own base value, so the *findings* depend on the
sizing alone — the seed moves every value but changes no verdict.

## Running it

Local filesystem (a temporary directory, removed on exit unless `--keep`):

```powershell
just bench-history-stress
# or a quick scaled-down run:
just bench-history-stress --commits 100 --benchmarks 100
# pass any flags through:
just bench-history-stress --modes history --verbose
```

Real Azure Blob storage (a fresh `bh-stress-<unix>` container, deleted on exit unless
`--keep`):

```powershell
az login
just install-tools          # one-time: installs azcopy, used for the bulk upload
just bench-history-stress-azure           # account from BENCH_HISTORY_TEST_AZURE_ACCOUNT in constants.env
just bench-history-stress-azure myacct --keep   # custom account; keep the container afterwards
```

`just bench-history-stress-azure` requires `az login` (the harness and `azcopy` authenticate as
your Entra user via the Azure CLI) and the `azcopy` binary on `PATH`. It uses the same
account contract as the `test-azure` job; provision the account with the Bicep in
[`infra/azure-bench-history-test/`](../../infra/azure-bench-history-test/).

The equivalent raw invocations are:

```powershell
cargo run --release -p cargo-bench-history-stress -- --storage local
cargo run --release -p cargo-bench-history-stress -- --storage azure --account <name>
```

Always build `--release`: both seeding and analysis are CPU-bound and a debug build
distorts the timings badly.

## Flags

| Flag | Default | Meaning |
| --- | --- | --- |
| `--storage <local\|azure>` | `local` | Storage backend to seed and analyze against. |
| `--benchmarks <N>` | `1000` | Benchmark cases per discriminant set. |
| `--commits <N>` | `2000` | First-parent commits on the synthetic `main` history (~half store a run). |
| `--branch-commits <N>` | `6` | Commits on the synthetic feature branch. |
| `--dirty-runs <N>` | `3` | Dirty (uncommitted-tree) snapshots on the feature tip. |
| `--dir <PATH>` | temp dir | Local-storage root (local only). |
| `--account <NAME>` | `$BENCH_HISTORY_TEST_AZURE_ACCOUNT` | Azure storage account (Azure only). |
| `--container <NAME>` | `bh-stress-<unix>` | Azure container (Azure only). |
| `--modes <list>` | `history,branch` | Modes to measure (comma-separated). |
| `--repeat <N>` | `1` | Runs per mode (fastest is reported). |
| `--keep` | off | Keep the seeded data instead of cleaning up. |
| `--verbose` | off | Explanatory diagnostics on stderr. |
| `--seed <U64>` | fixed | Seed for the deterministic value generator. |

Stdout carries only the final report (a summary block plus a per-mode timing table),
so it can be redirected to a file cleanly; all progress logging goes to stderr.

## Output

The report is a summary block naming the dataset that was seeded, followed by one
timing row per measured mode. Its shape is fixed; its numbers are not, so they are
left as placeholders here rather than pinned to a run that would go stale:

```
cargo-bench-history stress results
==================================
storage:          local filesystem
discriminant sets: 20
benchmarks / set: 1000
main commits:     2000
  with a run:     1002
branch commits:   6
dirty snapshots:  3
objects seeded:   <count>
series defined:   <count>
data seeded:      <compressed size>
repo build:       <seconds>
generate + write: <seconds>
upload:           <seconds>

mode        duration        cpu   cpu%   objects   series  regressions  improvements  notable
----        -------- ----------   ----   -------   ------  -----------  ------------  -------
history       <secs>     <secs>  <pct>   <count>  <count>      <count>           n/a   yes/no
branch        <secs>     <secs>  <pct>   <count>  <count>      <count>       <count>   yes/no
```

The summary block restates the sizing — the flags, plus the discriminant-set count
the engine × triple matrix in `src/scenario.rs` fixes — then what seeding produced
and what each seeding phase cost. Each mode row then reports:

| Column | Meaning |
| --- | --- |
| `duration` | Wall-clock time of the fastest `analyze` run in that mode. |
| `cpu` | Processor time consumed across all threads during that run. |
| `cpu%` | Processor time as a fraction of `duration × cores` — full saturation is 100%. |
| `objects` | Stored objects the run loaded. |
| `series` | Reconstructed `(set, benchmark, metric)` series. |
| `regressions` / `improvements` | Findings the detectors reported. `improvements` reads `n/a` in `history` mode, which watches for regressions only. |
| `notable` | Whether the run reported anything at all. |

Finding counts are a joint property of the sizing and the detectors' evidence gates,
so they move whenever either does — treat them as an observation of the run in front
of you, not a figure to compare against this page. What is fixed is their ceiling:
only a series whose seeded shape actually moves can be flagged, and in `branch` mode
that is just the benchmarks the cross-cutting rules touch (indices divisible by
`BRANCH_DIVISOR` or by `DIRTY_DIVISOR`, in `src/scenario.rs`) — a minority of the
seeded series. A count near that ceiling means the gates are letting nearly every
seeded shape through; a count of zero means the sizing is below the evidence the
detectors demand.
