#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![allow(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate is publish = false; its `pub` items form a handoff boundary \
              between lib.rs and the in-crate binary plus integration tests, not a \
              stable public API, so exhaustive construction and matching by those \
              in-workspace consumers is intended"
)]

//! Maintain a long-lived history of benchmark results and analyze it for trends
//! that snapshot-only tooling cannot see.
//!
//! Most benchmark tooling only reports the current run, or at best compares
//! against the previous local run. `cargo-bench-history` instead stores *every*
//! run as an immutable result set — on the local filesystem or, with the `azure`
//! feature, in an Azure Blob container — and reconstructs each benchmark's series
//! in git first-parent commit order, so historical trends become analyzable
//! ("benchmark X has been getting incrementally slower over the past 12 months",
//! "scenario Y regressed after commit Z, visible only in hindsight against noisy
//! data").
//!
//! # Quick start
//!
//! ```text
//! cargo bench-history install   # write a starter .cargo/bench_history.toml
//! cargo bench-history run        # bench the current commit and store the results
//! cargo bench-history backfill <from> <to>   # bootstrap history from past commits
//! cargo bench-history analyze    # report regressions and drift over history
//! ```
//!
//! `install` writes a configuration file you point at a storage location; `run`
//! benches the current commit and stores the results there; `backfill` benches a
//! range of past commits so there is a trend to analyze (a single `run` on its own
//! has nothing to compare against); `analyze` reads the accumulated history back
//! and reports what changed. Run these from the repository whose benchmarks you are
//! tracking.
//!
//! # How results are stored
//!
//! Each run is stored as one immutable result set per benchmark engine. The
//! storage is partitioned *only* by the factors that make two results
//! fundamentally incomparable — the **discriminant set**:
//!
//! * **engine** — the benchmark system (`callgrind`, `criterion`, `alloc_tracker`,
//!   `all_the_time`), since their numbers are not comparable to each other.
//! * **target triple** — for example `x86_64-unknown-linux-gnu`.
//! * **machine key** — a hardware fingerprint, but *only* for engines whose
//!   results depend on hardware. Deterministic engines (Callgrind instruction
//!   counts, `alloc_tracker` allocations) are hardware-independent and share a
//!   single `synthetic` partition; noisy engines (Criterion wall-clock,
//!   `all_the_time` processor time) are partitioned by machine key so results from
//!   different machines are never mixed.
//!
//! Everything else — toolchain version, OS, commit, CI provider — is recorded as
//! metadata rather than forking history, so its effect stays *visible* as a step
//! in the timeline. Results are keyed by commit: a **clean** run (committed tree)
//! writes one deterministic object per commit, while a **dirty** run (uncommitted
//! changes present) is stored separately as an ephemeral snapshot that analysis
//! only admits on the branch you are working on.
//!
//! # Commands
//!
//! ## `run`
//!
//! Executes the workspace benches once with `cargo bench`, harvests every
//! supported engine's machine-readable output, and stores one result set per
//! engine. There is nothing to configure about engines: the run enables the
//! combined environment the engines need and detects each engine from the output
//! it produces (off Linux the Callgrind benches compile to no-ops, so only the
//! host-runnable engines are stored). Re-running the same clean commit is refused
//! as a duplicate unless `--overwrite` replaces the stored result; `--no-store`
//! harvests and reports without writing. A run is positioned on the timeline by
//! its commit's committer date, never by when the benchmarks happened to execute.
//!
//! ## `install`
//!
//! Generates a starter `.cargo/bench_history.toml` if absent, printing its path
//! and next steps (including how to `backfill` history for an existing
//! repository); an existing file is never overwritten.
//!
//! ## `backfill`
//!
//! Replays `run` across the inclusive commit range `<from> <to>`, bootstrapping
//! history for a repository that adopted the tool late. Each commit is checked out
//! in a dedicated git **worktree** (the primary checkout is never touched, so a
//! dirty working tree is fine) and benched there, recording the commit's committer
//! date as its timeline position. The range `<from> <to>` only needs to form a
//! first-parent chain; it does not have to lie on the current branch. Commits that
//! already have a stored result are skipped
//! (so backfill is resumable and cheap to re-run); `--overwrite` re-benches the
//! whole range. A commit that fails to build or bench stops the run unless
//! `--ignore-errors` continues past it.
//!
//! ## `analyze`
//!
//! Reconstructs a timeline from git history and reports notable patterns. It
//! requires a repository. Commits up to the merge-base with the base branch
//! contribute clean runs only, while commits unique to the analyzed branch also
//! contribute dirty snapshots unless `--no-dirty` is given. Pass one or more
//! benchmark-id prefixes (matched against the qualified
//! `<package>/<group>/<case>/<value>` identity) to scope the analysis to a subset
//! of benchmarks. Findings are *advisory*: the exit code reflects only whether the
//! analysis ran, never what it found. Downstream automation reads the
//! machine-readable signal from the `json` report — `mode`, the boolean `notable`
//! (any finding survived), each finding's `direction`/`flipped_at`, and the full
//! per-finding `series` for charting. See [Analyze modes](#analyze-modes) below.
//!
//! ## `list`
//!
//! Previews, without analyzing, the data a matching `analyze` would consume. The
//! subject is a bare word:
//!
//! * `list runs` — the run, series, and per-commit counts of the selected runs,
//!   per discriminant set.
//! * `list discriminants` — the discriminant sets present in storage (no
//!   repository required), for discovering which engines, triples, and machine
//!   keys have data before scoping an analysis. This is a discovery catalog, so it
//!   lists *every* stored partition regardless of the current machine; pass a facet
//!   to narrow it.
//! * `list blessings` — the blessings recorded at the current commit, or — with
//!   `--all` — the most recent blessing of every benchmark across the window.
//!
//! ## `prune`
//!
//! Deletes a chosen portion of the stored data, using the same selection pipeline
//! as `analyze`/`list`. A scope is required: `--dirty` removes ephemeral
//! uncommitted-tree snapshots, `--clean` removes clean runs and their blessing
//! sidecars, and `--all` removes both. Pruning preserves base-branch history — only
//! the context branch's own commits (those after the merge-base with `--base`) are
//! removed. When the context resolves onto the base branch itself, the whole
//! selection *is* base-branch history, so the deletion is refused unless
//! `--prune-base` confirms it. Narrow the selection with a facet, a `<commit>`
//! argument, `--since`, or `--until`. `--dry-run` previews without deleting.
//!
//! ## `bless` / `unbless`
//!
//! `bless` accepts an intentional performance change on the base branch so history
//! analysis stops re-flagging it. Pass one or more benchmark-id prefixes to accept
//! (matched against the qualified `<package>/<group>/<case>/<value>` identity, e.g.
//! `bless all_the_time/read_cell`), or `--all` to accept every benchmark recorded
//! at the commit. A blessing re-baselines the benchmark's history from the blessed
//! commit forward, so the accepted step is no longer reported while earlier points
//! stay on the chart for context. Blessing is base-branch-only and requires an
//! existing recorded run at the blessed commit. By default `bless`/`unbless` act on
//! `HEAD`; `--context <ref>` blesses or unblesses another commit instead. `unbless`
//! removes the blessings recorded at that commit — note that any blessings defined
//! at *later* commits remain in force, so the timeline may stay blessed past the
//! unblessed commit.
//!
//! # Selecting data: options shared by the query commands
//!
//! `analyze`, `list`, and `prune` share one selection model, organized in `--help`
//! into named groups:
//!
//! * **Discriminant selection** (`--engine`, `--target-triple`, `--machine-key`) —
//!   chooses which discriminant sets to operate on. Each facet is repeatable
//!   (union of values) and defaults to the current machine's value when omitted
//!   (`--engine` defaults to every engine; `list discriminants` is a catalog and
//!   defaults to every partition). The literal `all` removes the filter
//!   for that dimension, e.g. `--machine-key all` spans every machine.
//! * **Commit selection** (`--context`, `--base`, `--since`, `--until`) —
//!   `--context` is the ref whose history is analyzed (default `HEAD`); `--base` is
//!   the ref it branched from (default: the configured or detected default branch),
//!   which determines the merge-base split. `--since`/`--until` bound the window by
//!   **commit** timestamp and accept an RFC 3339 timestamp, a `YYYY-MM-DD` date, or
//!   a relative duration such as `6 months ago`.
//! * **Data filtering** (`--no-dirty`) — exclude dirty snapshots.
//!
//! # Analyze modes
//!
//! `analyze` runs in one of three modes, chosen automatically (`--mode auto`, the
//! default) or forced with `--mode`:
//!
//! * **history** — long-range trend watch over the base branch (selected when you
//!   analyze a clean checkout of the base branch). It detects sustained
//!   change-points and slow drifts, defaults `--since` to the last six months, and
//!   reports only **regressions** (steady improvement over time is expected) unless
//!   `--include-improvements` is given. A spike that has since recovered is
//!   suppressed by default; `--include-inactive` surfaces such resolved findings.
//! * **branch** — "how does my feature compare" (selected for a feature branch, or
//!   a dirty base checkout). It judges the branch by its **latest** state versus the
//!   base, reporting both regressions and improvements; the finding's `flipped_at`
//!   names the commit a regime began at.
//! * **tip** — a fast guard that compares only the latest commit against the
//!   recently established level, reporting regressions only.
//!
//! # Configuration
//!
//! `install` writes a starter `.cargo/bench_history.toml`. It carries an optional
//! `[project]` id (used to namespace stored data; defaults to the workspace
//! directory name) and a `[storage]` section selecting either the local filesystem
//! or, behind the `azure` feature, an Azure Blob container:
//!
//! ```toml
//! # [project]
//! # id = "my-project"
//! # default_branch = "main"   # base branch for `analyze`; auto-detected by default
//!
//! [storage.local]
//! path = "./bench-history"
//! ```
//!
//! Every command accepts `--config PATH` to point at a non-default file, `--repo
//! PATH` to resolve git state from another directory, and `--verbose` to emit a
//! step-by-step diagnostic trail to standard error.

mod analyze;
mod bench;
mod bench_output;
mod cli;
mod command;
mod commands;
mod config;
mod config_writer;
mod dispatch;
mod git;
mod git_history;
mod host;
mod machine;
mod outcome;
mod probe;
mod process;
mod report;
mod storage;
mod text;
mod wiring;

pub(crate) use cargo_bench_history_core::{bless, comparability, constants, context, model};

pub use cli::{Cli, EarlyExit};
pub use command::{
    AnalyzeOptions, BackfillOptions, BlessOptions, Command, InstallOptions, ListOptions,
    ListSubject, PruneOptions, RunOptions, UnblessOptions,
};
pub use config::{ConfigError, default_template};
pub use context::{EnvironmentInfo, EnvironmentProvider, GitInfo, RunContext, ToolchainInfo};
pub use dispatch::{Overrides, run, run_with_overrides};
pub use model::{BenchmarkId, BenchmarkResult, Metric, MetricKind, Run, SCHEMA_VERSION};
pub use outcome::{RunError, RunOutcome};
pub use storage::StorageError;
