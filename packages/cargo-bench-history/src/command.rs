//! The parsed command model the binary and tests operate on: the [`Command`]
//! enum and its per-subcommand option structs.

use std::path::PathBuf;

use crate::model::BenchmarkIdPrefix;

/// How a command selects local filesystem storage, from the `--local` flag.
///
/// Local storage paths are machine-dependent, so they are never carried in the
/// shared (version-controlled) configuration file; they are supplied at run time
/// instead. `None` on a command's `local` field means `--local` was not given, in
/// which case the configured cloud backend is used.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalStorageSelection {
    /// `--local=<path>`: store under this filesystem path (relative paths resolve
    /// against the workspace directory).
    Path(PathBuf),
    /// A bare `--local`: take the path from the `CARGO_BENCH_HISTORY_STORAGE`
    /// environment variable (an unset or empty variable is an error).
    FromEnv,
}

/// How a read command selects a local read-through cache directory, from the
/// `--cache` flag.
///
/// Like a `--local` path, a cache directory is machine-dependent, so it is never
/// carried in the shared configuration file but supplied at run time. `None` on a
/// command's `cache` field means `--cache` was not given (no cache: cloud reads go
/// straight to the backend). A distinct type from [`LocalStorageSelection`] keeps
/// the two unrelated selections from being mixed up, even though their three-state
/// shape is the same.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CacheSelection {
    /// `--cache=<path>`: mirror fetched objects under this filesystem directory
    /// (relative paths resolve against the workspace directory).
    Path(PathBuf),
    /// A bare `--cache`: take the directory from the `CARGO_BENCH_HISTORY_CACHE`
    /// environment variable (an unset or empty variable is an error).
    FromEnv,
}

/// A fully parsed command ready to execute.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// Run the configured benchmark engines and store the results.
    Collect(CollectOptions),
    /// Generate a starter configuration file.
    Install(InstallOptions),
    /// Analyze stored history for notable patterns.
    Analyze(AnalyzeOptions),
    /// List the data set a matching `analyze` pass would include.
    List(ListOptions),
    /// Delete stored runs (and their blessing sidecars) from the data set a
    /// matching `analyze`/`list` pass resolves.
    Prune(PruneOptions),
    /// Show the raw per-commit data points of one `(benchmark, metric)` series
    /// from the data set a matching `analyze`/`list` pass resolves.
    Examine(ExamineOptions),
    /// Replay `collect` across a range of historical commits.
    Backfill(BackfillOptions),
    /// Accept a benchmark's current level on the base branch as intentional.
    Bless(BlessOptions),
    /// Remove blessings recorded at the current commit.
    Unbless(UnblessOptions),
}

/// Options for the `collect` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollectOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to run benchmarks in and read git state from; defaults to the
    /// working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Restrict the run to these packages (`--package`/`-p`); empty means the
    /// whole workspace.
    pub packages: Vec<String>,
    /// Exclude these packages from a whole-workspace run (`--exclude`); only
    /// meaningful when `packages` is empty (the workspace default).
    pub excludes: Vec<String>,
    /// Restrict the run to these benchmark targets (`--bench`); empty means all.
    pub benches: Vec<String>,
    /// Cargo features to activate (`--features`), forwarded verbatim; empty means
    /// none beyond the default set.
    pub features: Vec<String>,
    /// Activate all cargo features (`--all-features`).
    pub all_features: bool,
    /// Disable the default cargo feature set (`--no-default-features`).
    pub no_default_features: bool,
    /// Override for the machine fingerprint (hardware-dependent engines), if set.
    pub machine_key: Option<String>,
    /// Harvest and build results without storing them.
    pub no_store: bool,
    /// Replace an already-stored result for this run's identity instead of
    /// refusing the run as a duplicate.
    pub overwrite: bool,
    /// Treat an already-stored result for this run's identity as a success that
    /// writes nothing, instead of refusing the run as a duplicate. Mutually
    /// exclusive with `overwrite`; the append-only mode the CI `collect`
    /// recipe uses so collection never overwrites (and so never invalidates the
    /// cloud read-through cache).
    pub skip_existing: bool,
    /// Arguments forwarded verbatim to the benchmark command after the scope flags.
    pub passthrough: Vec<String>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `install` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallOptions {
    /// Path to the configuration file to generate, if overridden.
    pub config_path: Option<PathBuf>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `analyze` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Read-through cache selection from `--cache`; mirrors fetched cloud objects
    /// under a local directory so repeated reads avoid re-downloading the history.
    /// `None` means `--cache` was not given. Ignored with a `--local` backend.
    pub cache: Option<CacheSelection>,
    /// Target ref whose history is analyzed; defaults to `HEAD`.
    pub context: Option<String>,
    /// Base ref the target's history is split at; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Exclude dirty (uncommitted-tree) snapshots from the target side.
    pub no_dirty: bool,
    /// Only consider commits made on or after this cutoff, if set.
    pub since: Option<String>,
    /// Only consider commits made on or before this cutoff, if set.
    pub until: Option<String>,
    /// Restrict analysis to these engines (repeatable). Empty auto-detects every
    /// engine; the `all` keyword is an explicit synonym for no filter.
    pub engine: Vec<String>,
    /// Restrict analysis to these full target triples (repeatable). Empty
    /// auto-detects the current machine's triple; `all` matches every triple.
    pub target_triple: Vec<String>,
    /// Restrict analysis to these machine keys (repeatable). Empty
    /// auto-detects the current machine's fingerprint; `all` matches every
    /// machine.
    pub machine_key: Vec<String>,
    /// Restrict analysis to benchmarks whose qualified identity starts with one of
    /// these prefixes (repeatable). Empty means every benchmark.
    pub prefixes: Vec<BenchmarkIdPrefix>,
    /// Suppress the default text report on standard output (`--no-text`).
    pub no_text: bool,
    /// Write the Markdown report to this path, if set (`--markdown <path>`). A
    /// relative path resolves against the working directory.
    pub markdown: Option<PathBuf>,
    /// Write the JSON report to this path, if set (`--json <path>`). A relative
    /// path resolves against the working directory.
    pub json: Option<PathBuf>,
    /// Write the condensed Markdown summary (the most significant findings only) to
    /// this path, if set (`--markdown-summary <path>`). A relative path resolves
    /// against the working directory. Analyze-only, so a large analysis still fits
    /// within a GitHub issue body.
    pub markdown_summary: Option<PathBuf>,
    /// Analysis-mode selector (`auto`, `history`, `branch`, or `tip`), if set.
    /// `auto` (the default) infers history vs branch mode from the git topology.
    pub mode: Option<String>,
    /// In history mode, also report sustained improvements (regressions only by
    /// default, since improvement over time on the base branch is expected).
    pub include_improvements: bool,
    /// In history mode, also report inactive findings: changes that the current
    /// state no longer reflects (a regression that later recovered). Hidden by
    /// default since they need no action.
    pub include_inactive: bool,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
    /// Emit per-stage wall-clock timings to standard error, independent of
    /// `verbose`. `verbose` implies these as well; this flag exists so a
    /// programmatic caller (the stress harness) can observe the per-stage load
    /// breakdown *without* the per-object note flood that would both bury the
    /// timings and distort them.
    pub timing: bool,
}

impl AnalyzeOptions {
    /// Whether the per-stage wall-clock timings should be emitted to standard
    /// error. `verbose` implies them; the `timing` flag enables them on its own,
    /// so a programmatic caller (the stress harness) can observe the per-stage
    /// load breakdown without the per-object note flood.
    pub(crate) fn stage_timings_enabled(&self) -> bool {
        self.verbose || self.timing
    }
}

/// The kind of thing a `list` invocation enumerates.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ListSubject {
    /// The runs that would enter a matching `analyze` pass.
    #[default]
    Runs,
    /// Every discriminant set present in storage (no repository required),
    /// regardless of the current machine.
    Discriminants,
    /// The blessings recorded at the current commit (or, with `all`, the most
    /// recent blessing of every benchmark in the analysis window).
    Blessings,
}

/// Options for the `list` command.
///
/// The data-set-selection options mirror [`AnalyzeOptions`] exactly so a `list`
/// invocation previews the data set the same `analyze` invocation would consume.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListOptions {
    /// What to enumerate (runs, discriminant sets, or blessings).
    pub subject: ListSubject,
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Read-through cache selection from `--cache`; mirrors fetched cloud objects
    /// under a local directory so repeated reads avoid re-downloading the history.
    /// `None` means `--cache` was not given. Ignored with a `--local` backend.
    pub cache: Option<CacheSelection>,
    /// Target ref whose history is listed; defaults to `HEAD`.
    pub context: Option<String>,
    /// Base ref the target's history is split at; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Exclude dirty (uncommitted-tree) snapshots from the target side.
    pub no_dirty: bool,
    /// Only consider commits made on or after this cutoff, if set.
    pub since: Option<String>,
    /// Only consider commits made on or before this cutoff, if set.
    pub until: Option<String>,
    /// Restrict the listing to these engines (repeatable). Empty auto-detects
    /// every engine; the `all` keyword is an explicit synonym for no filter.
    pub engine: Vec<String>,
    /// Restrict the listing to these full target triples (repeatable). Empty
    /// auto-detects the current machine's triple; `all` matches every triple.
    pub target_triple: Vec<String>,
    /// Restrict the listing to these machine keys (repeatable). Empty
    /// auto-detects the current machine's fingerprint; `all` matches every
    /// machine.
    pub machine_key: Vec<String>,
    /// Suppress the default text report on standard output (`--no-text`).
    pub no_text: bool,
    /// Write the Markdown report to this path, if set (`--markdown <path>`). A
    /// relative path resolves against the working directory.
    pub markdown: Option<PathBuf>,
    /// Write the JSON report to this path, if set (`--json <path>`). A relative
    /// path resolves against the working directory.
    pub json: Option<PathBuf>,
    /// With `blessings`, list the most recent blessing of every benchmark across
    /// the whole analysis window rather than only those at the current commit.
    pub all: bool,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `examine` command.
///
/// The data-set-selection options mirror [`AnalyzeOptions`]/[`ListOptions`] so an
/// `examine` invocation drills into exactly the data set the same `analyze`/`list`
/// invocation would resolve. Unlike them, it names a single `(benchmark, metric)`
/// series to pivot into raw per-commit data points.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExamineOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Read-through cache selection from `--cache`; mirrors fetched cloud objects
    /// under a local directory so repeated reads avoid re-downloading the history.
    /// `None` means `--cache` was not given. Ignored with a `--local` backend.
    pub cache: Option<CacheSelection>,
    /// Target ref whose history is examined; defaults to `HEAD`.
    pub context: Option<String>,
    /// Base ref the target's history is split at; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Exclude dirty (uncommitted-tree) snapshots from the target side.
    pub no_dirty: bool,
    /// Only consider commits made on or after this cutoff, if set.
    pub since: Option<String>,
    /// Only consider commits made on or before this cutoff, if set.
    pub until: Option<String>,
    /// Restrict the examination to these engines (repeatable). Empty auto-detects
    /// every engine; the `all` keyword is an explicit synonym for no filter.
    pub engine: Vec<String>,
    /// Restrict the examination to these full target triples (repeatable). Empty
    /// auto-detects the current machine's triple; `all` matches every triple.
    pub target_triple: Vec<String>,
    /// Restrict the examination to these machine keys (repeatable). Empty
    /// auto-detects the current machine's fingerprint; `all` matches every machine.
    pub machine_key: Vec<String>,
    /// The exact qualified benchmark id whose series is examined (required).
    pub benchmark: String,
    /// The metric name (a [`MetricKind`](crate::model::MetricKind) `as_str` value)
    /// whose series is examined (required).
    pub metric: String,
    /// Suppress the default text report on standard output (`--no-text`).
    pub no_text: bool,
    /// Write the Markdown report to this path, if set (`--markdown <path>`). A
    /// relative path resolves against the working directory.
    pub markdown: Option<PathBuf>,
    /// Write the JSON report to this path, if set (`--json <path>`). A relative
    /// path resolves against the working directory.
    pub json: Option<PathBuf>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `prune` command.
///
/// The data-set-selection options mirror [`AnalyzeOptions`]/[`ListOptions`] so a
/// `analyze`/`list` invocation would resolve. The caller must say which kinds of
/// run to delete: `clean` removes clean runs (plus the blessing sidecars on every
/// commit whose clean run is removed), `dirty` removes dirty (uncommitted-tree)
/// snapshots, and setting both removes everything. Pruning the base branch's own
/// data set (when `context` resolves to the same commit as `base`) requires
/// `prune_base` as a safety guard.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PruneOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Read-through cache selection from `--cache`; mirrors fetched cloud objects
    /// under a local directory so the pre-prune load avoids re-downloading the
    /// history. `None` means `--cache` was not given. Ignored with a `--local`
    /// backend.
    pub cache: Option<CacheSelection>,
    /// Target ref whose history is pruned; defaults to `HEAD`.
    pub context: Option<String>,
    /// Base ref the context branched off from; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Restrict removal to specific commits (case-insensitive commit-ID prefix match);
    /// repeatable. Empty means every one of the selected commits.
    pub commit: Vec<String>,
    /// Only prune commits made on or after this cutoff, if set.
    pub since: Option<String>,
    /// Only prune commits made on or before this cutoff, if set.
    pub until: Option<String>,
    /// Restrict removal to these engines (repeatable). Empty auto-detects every
    /// engine; the `all` keyword is an explicit synonym for no filter.
    pub engine: Vec<String>,
    /// Restrict removal to these full target triples (repeatable). Empty
    /// auto-detects the current machine's triple; `all` matches every triple.
    pub target_triple: Vec<String>,
    /// Restrict removal to these machine keys (repeatable). Empty
    /// auto-detects the current machine's fingerprint; `all` matches every
    /// machine.
    pub machine_key: Vec<String>,
    /// Remove clean runs (and their blessing sidecars).
    pub clean: bool,
    /// Remove dirty (uncommitted-tree) snapshots.
    pub dirty: bool,
    /// Confirm pruning the base branch's own data set (when `context` resolves to
    /// the same commit as `base`).
    pub prune_base: bool,
    /// Preview what would be removed without deleting anything.
    pub dry_run: bool,
    /// Suppress the default text report on standard output (`--no-text`).
    pub no_text: bool,
    /// Write the Markdown report to this path, if set (`--markdown <path>`). A
    /// relative path resolves against the working directory.
    pub markdown: Option<PathBuf>,
    /// Write the JSON report to this path, if set (`--json <path>`). A relative
    /// path resolves against the working directory.
    pub json: Option<PathBuf>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `backfill` command.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to run benchmarks in and read git history from; defaults to the
    /// working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Oldest commit of the range to backfill (inclusive).
    pub from: String,
    /// Newest commit of the range to backfill (inclusive).
    pub to: String,
    /// Restrict the runs to these packages (`--package`/`-p`); empty means the
    /// whole workspace.
    pub packages: Vec<String>,
    /// Exclude these packages from a whole-workspace run (`--exclude`); only
    /// meaningful when `packages` is empty (the workspace default).
    pub excludes: Vec<String>,
    /// Restrict the runs to these benchmark targets (`--bench`); empty means all.
    pub benches: Vec<String>,
    /// Cargo features to activate (`--features`), forwarded verbatim; empty means
    /// none beyond the default set.
    pub features: Vec<String>,
    /// Activate all cargo features (`--all-features`).
    pub all_features: bool,
    /// Disable the default cargo feature set (`--no-default-features`).
    pub no_default_features: bool,
    /// Override for the machine fingerprint (hardware-dependent engines), if set.
    pub machine_key: Option<String>,
    /// Replace already-stored results for the backfilled commits instead of
    /// skipping them as duplicates.
    pub overwrite: bool,
    /// Continue past commits whose build or benchmark fails instead of stopping.
    pub ignore_errors: bool,
    /// Arguments forwarded verbatim to the benchmark command after the scope flags.
    pub passthrough: Vec<String>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `bless` command.
///
/// The data-set-selection options mirror the facet subset of [`AnalyzeOptions`]
/// (engine, target triple, and machine key) so a `bless` writes its sidecars into
/// exactly the discriminant sets a matching `analyze` would consume. It always
/// acts at the current commit (`HEAD`), so it has no `context` / `since` /
/// `metric` selectors.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlessOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Commit to bless; defaults to `HEAD`. The blessing is recorded against the
    /// `clean.json` stored at this commit.
    pub context: Option<String>,
    /// Base ref the context commit must be on; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Restrict the blessing to these engines (repeatable). Empty auto-detects
    /// every engine; the `all` keyword is an explicit synonym for no filter.
    pub engine: Vec<String>,
    /// Restrict the blessing to these full target triples (repeatable). Empty
    /// auto-detects the current machine's triple; `all` matches every triple.
    pub target_triple: Vec<String>,
    /// Restrict the blessing to these machine keys (repeatable). Empty
    /// auto-detects the current machine's fingerprint; `all` matches every
    /// machine.
    pub machine_key: Vec<String>,
    /// Benchmark-id prefixes to accept (matched against the qualified identity).
    /// At least one is required unless `all` is set.
    pub prefixes: Vec<BenchmarkIdPrefix>,
    /// Accept every benchmark at the context commit (no prefixes required).
    pub all: bool,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

/// Options for the `unbless` command.
///
/// Mirrors [`BlessOptions`]' selection facets but takes no prefixes: an unbless
/// removes every blessing recorded at the current commit in the selected sets
/// (sidecars are immutable, so editing a blessing means unblessing then
/// re-blessing the subset to keep).
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnblessOptions {
    /// Path to the configuration file, if overridden.
    pub config_path: Option<PathBuf>,
    /// Repository to resolve git topology from; defaults to the working directory.
    pub repo: Option<PathBuf>,
    /// Local-storage selection from `--local`; overrides the configured cloud
    /// backend. `None` means `--local` was not given (use the configured backend).
    pub local: Option<LocalStorageSelection>,
    /// Commit to unbless; defaults to `HEAD`. Only blessings recorded at this
    /// commit are removed.
    pub context: Option<String>,
    /// Base ref the context commit must be on; defaults to the detected (or
    /// configured) default branch.
    pub base: Option<String>,
    /// Restrict the unblessing to these engines (repeatable). Empty auto-detects
    /// every engine; the `all` keyword is an explicit synonym for no filter.
    pub engine: Vec<String>,
    /// Restrict the unblessing to these full target triples (repeatable). Empty
    /// auto-detects the current machine's triple; `all` matches every triple.
    pub target_triple: Vec<String>,
    /// Restrict the unblessing to these machine keys (repeatable). Empty
    /// auto-detects the current machine's fingerprint; `all` matches every
    /// machine.
    pub machine_key: Vec<String>,
    /// Emit detailed diagnostic notes to standard error describing each step.
    pub verbose: bool,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn stage_timings_enabled_follows_verbose_or_timing() {
        let options = |verbose, timing| AnalyzeOptions {
            verbose,
            timing,
            ..AnalyzeOptions::default()
        };
        // `verbose` implies the timings, the `timing` flag enables them on its
        // own, and neither flag leaves them off — so the policy is the logical
        // OR of the two, not their AND.
        assert!(!options(false, false).stage_timings_enabled());
        assert!(options(true, false).stage_timings_enabled());
        assert!(options(false, true).stage_timings_enabled());
        assert!(options(true, true).stage_timings_enabled());
    }
}
