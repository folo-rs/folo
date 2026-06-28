//! Argument parsing: the `clap` subcommand surface and its translation into the
//! typed [`Command`](crate::Command) model.
//!
//! The wide option surface is organized into named help groups (Environment,
//! Output, Discriminant selection, Commit selection, Data filtering, Benchmark
//! scope) so `--help` makes the relationship between options legible. Shared
//! groups are factored into [`clap::Args`] structs and `#[command(flatten)]`ed
//! into each subcommand so the same option means the same thing everywhere.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand as ClapSubcommand, ValueEnum};

use crate::model::BenchmarkIdPrefix;
use crate::{
    AnalyzeOptions, BackfillOptions, BlessOptions, Command, InstallOptions, ListOptions,
    ListSubject, PruneOptions, RunOptions, UnblessOptions,
};

const HEADING_ENV: &str = "Environment and execution";
const HEADING_OUTPUT: &str = "Output";
const HEADING_DISCRIMINANT: &str = "Discriminant selection";
const HEADING_COMMIT: &str = "Commit selection";
const HEADING_FILTER: &str = "Data filtering";
const HEADING_SCOPE: &str = "Benchmark scope";
const HEADING_FEATURES: &str = "Feature selection";
const HEADING_ANALYSIS: &str = "Analysis";

/// Maintain a history of benchmark results over time and analyze it for trends.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-bench-history",
    about = "Maintain a history of benchmark results over time and analyze it for trends.",
    disable_help_subcommand = true,
    disable_version_flag = true
)]
pub struct Cli {
    /// The subcommand to execute.
    #[command(subcommand)]
    command: Subcommand,
}

/// A parse outcome that should terminate the program before execution.
///
/// This is either a help/usage request (success, printed to stdout) or a parse
/// error (failure, printed to stderr). Mirrors the shape the binary entry point
/// consumes.
#[derive(Debug)]
pub struct EarlyExit {
    /// The rendered message (help text or error) to print.
    pub output: String,
    /// `Ok` for a help/usage request (exit success), `Err` for a parse error.
    pub status: Result<(), ()>,
}

impl EarlyExit {
    /// Classifies a `clap` parse error into the success/failure early-exit shape.
    fn from_clap(error: &clap::Error) -> Self {
        use clap::error::ErrorKind;
        let success = matches!(
            error.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        Self {
            output: error.to_string(),
            status: if success { Ok(()) } else { Err(()) },
        }
    }
}

impl Cli {
    /// Parses an argument vector (program name followed by its arguments) into the
    /// typed CLI, returning an [`EarlyExit`] for a help request or a parse error.
    ///
    /// # Errors
    ///
    /// Returns an [`EarlyExit`] when the arguments request help/usage or fail to
    /// parse.
    pub fn from_args(command_name: &[&str], args: &[&str]) -> Result<Self, EarlyExit> {
        let argv: Vec<&str> = command_name.iter().chain(args).copied().collect();
        Self::try_parse_from(argv).map_err(|error| EarlyExit::from_clap(&error))
    }

    /// Translates the parsed arguments into the typed command model.
    #[must_use]
    pub fn into_command(self) -> Command {
        match self.command {
            Subcommand::Analyze(command) => Command::Analyze(command.into_options()),
            Subcommand::Backfill(command) => Command::Backfill(command.into_options()),
            Subcommand::Bless(command) => Command::Bless(command.into_options()),
            Subcommand::Install(command) => Command::Install(command.into_options()),
            Subcommand::List(command) => Command::List(command.into_options()),
            Subcommand::Prune(command) => Command::Prune(command.into_options()),
            Subcommand::Run(command) => Command::Run(command.into_options()),
            Subcommand::Unbless(command) => Command::Unbless(command.into_options()),
        }
    }

    /// The top-level help text, including the command list with descriptions.
    ///
    /// Shown when the tool is invoked with no subcommand, so the available
    /// commands and what each one does are immediately visible.
    #[must_use]
    pub fn help(program_name: &str) -> String {
        // `--help` is always reported as an early exit carrying the rendered
        // help text, so the `Ok` arm is never taken in practice.
        Self::from_args(&[program_name], &["--help"])
            .err()
            .map(|early_exit| early_exit.output)
            .unwrap_or_default()
    }
}

#[derive(ClapSubcommand, Debug)]
enum Subcommand {
    /// Analyze stored history for notable patterns.
    Analyze(AnalyzeCommand),
    /// Replay `run` across a range of historical commits.
    Backfill(BackfillCommand),
    /// Accept a benchmark's current level on the base branch as intentional.
    Bless(BlessCommand),
    /// Generate a starter configuration file.
    Install(InstallCommand),
    /// List the data set a matching `analyze` would include, without analyzing it.
    List(ListCommand),
    /// Delete stored runs (and their blessing sidecars) from the resolved data set.
    Prune(PruneCommand),
    /// Run the workspace benchmarks (`cargo bench`) and store the results.
    Run(RunCommand),
    /// Remove blessings recorded at the current commit.
    Unbless(UnblessCommand),
}

/// Environment and execution options shared by every command that reads a
/// repository's git state.
#[derive(Args, Debug)]
#[command(next_help_heading = HEADING_ENV)]
struct EnvArgs {
    /// Path to the configuration file (defaults to `.cargo/bench_history.toml`).
    /// A relative path is resolved against the working directory (not the target
    /// repository).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Repository to resolve git state from (defaults to the working directory).
    #[arg(long, value_name = "PATH")]
    repo: Option<PathBuf>,

    /// Emit detailed diagnostic notes to standard error describing each step.
    #[arg(long)]
    verbose: bool,
}

/// The repeatable, `all`-aware discriminant facets used by every query command.
///
/// Each facet auto-detects the current machine when omitted (`--engine` has no
/// machine-derived value, so it auto-detects to every engine); repeating a facet
/// unions its values; the literal `all` removes the filter for that dimension.
#[derive(Args, Debug)]
#[command(next_help_heading = HEADING_DISCRIMINANT)]
struct QueryFacetArgs {
    /// Restrict to these engines, e.g. `criterion`/`callgrind` (repeatable; `all`
    /// matches every engine; default: every engine).
    #[arg(long, value_name = "NAME")]
    engine: Vec<String>,

    /// Restrict to these full target triples, e.g. `x86_64-unknown-linux-gnu`
    /// (repeatable; `all` matches every triple; default: this machine's triple).
    #[arg(long, value_name = "TRIPLE")]
    target_triple: Vec<String>,

    /// Restrict to these machine keys (repeatable; `all` matches every
    /// machine; default: this machine's fingerprint).
    #[arg(long, value_name = "KEY")]
    machine_key: Vec<String>,
}

/// Commit selection shared by `analyze`/`list`.
#[derive(Args, Debug)]
#[command(next_help_heading = HEADING_COMMIT)]
struct TimelineArgs {
    /// Target ref whose history is used (defaults to HEAD).
    #[arg(long, value_name = "REF")]
    context: Option<String>,

    /// Base ref the target branched off from (defaults to the default branch).
    #[arg(long, value_name = "REF")]
    base: Option<String>,

    /// Only consider commits made on or after this cutoff: an RFC 3339 timestamp,
    /// a `YYYY-MM-DD` date, or a relative duration such as `6 months ago`.
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,

    /// Only consider commits made on or before this cutoff (same formats as
    /// `--since`).
    #[arg(long, value_name = "WHEN")]
    until: Option<String>,
}

/// Run the workspace benchmarks (`cargo bench`) and store the results.
#[derive(Args, Debug)]
struct RunCommand {
    #[command(flatten)]
    env: EnvArgs,

    /// Override the machine fingerprint used to partition hardware-dependent
    /// results (for example, a CI machine-pool name).
    #[arg(long, value_name = "KEY", help_heading = HEADING_DISCRIMINANT)]
    machine_key: Option<String>,

    /// Benchmark the entire workspace (the default when no `--package` is given);
    /// conflicts with `--package`.
    #[arg(long, help_heading = HEADING_SCOPE, conflicts_with = "package")]
    workspace: bool,

    /// Benchmark only this package; repeatable, e.g. `-p nm -p many_cpus`
    /// (default: the whole workspace).
    #[arg(long = "package", short = 'p', value_name = "NAME", help_heading = HEADING_SCOPE)]
    package: Vec<String>,

    /// Exclude a package from a whole-workspace run; repeatable, e.g.
    /// `--exclude nm --exclude many_cpus`. Conflicts with `--package`.
    #[arg(long, value_name = "NAME", help_heading = HEADING_SCOPE, conflicts_with = "package")]
    exclude: Vec<String>,

    /// Benchmark only this bench target; repeatable (default: every bench target).
    #[arg(long, value_name = "NAME", help_heading = HEADING_SCOPE)]
    bench: Vec<String>,

    /// Activate cargo features for the benchmark build; space- or comma-separated,
    /// repeatable. Forwarded verbatim to `cargo bench` as `--features`.
    #[arg(long, value_name = "FEATURES", help_heading = HEADING_FEATURES)]
    features: Vec<String>,

    /// Activate all cargo features of all selected packages (`--all-features`).
    #[arg(long, help_heading = HEADING_FEATURES)]
    all_features: bool,

    /// Do not activate the `default` cargo feature (`--no-default-features`).
    #[arg(long, help_heading = HEADING_FEATURES)]
    no_default_features: bool,

    /// Harvest and build results without storing them.
    #[arg(long, help_heading = HEADING_ENV)]
    no_store: bool,

    /// Replace an already-stored result for this run instead of refusing it as a
    /// duplicate.
    #[arg(long, help_heading = HEADING_ENV)]
    overwrite: bool,

    /// Arguments after `--` forwarded verbatim to `cargo bench` after the scope
    /// flags.
    #[arg(last = true, value_name = "ARGS")]
    passthrough: Vec<String>,
}

impl RunCommand {
    fn into_options(self) -> RunOptions {
        RunOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            packages: resolve_packages(self.workspace, self.package),
            excludes: self.exclude,
            benches: self.bench,
            features: self.features,
            all_features: self.all_features,
            no_default_features: self.no_default_features,
            machine_key: self.machine_key,
            no_store: self.no_store,
            overwrite: self.overwrite,
            passthrough: self.passthrough,
            verbose: self.env.verbose,
        }
    }
}

/// Generate a starter configuration file.
#[derive(Args, Debug)]
struct InstallCommand {
    /// Path to the configuration file to generate.
    #[arg(long, value_name = "PATH", help_heading = HEADING_ENV)]
    config: Option<PathBuf>,

    /// Emit detailed diagnostic notes to standard error (which path is written,
    /// or that an existing configuration was left unchanged).
    #[arg(long, help_heading = HEADING_ENV)]
    verbose: bool,
}

impl InstallCommand {
    fn into_options(self) -> InstallOptions {
        InstallOptions {
            config_path: self.config,
            verbose: self.verbose,
        }
    }
}

/// Analyze stored history for notable patterns.
#[derive(Args, Debug)]
struct AnalyzeCommand {
    /// Benchmark-id prefixes to analyze, matched against the qualified identity
    /// (for example, `all_the_time/read_cell` or a family prefix
    /// `overhead/groups_`); repeatable (default: every benchmark).
    #[arg(value_name = "PREFIX")]
    prefixes: Vec<BenchmarkIdPrefix>,

    #[command(flatten)]
    env: EnvArgs,

    /// Output format: text, json, or markdown (default: text).
    #[arg(long, value_name = "FORMAT", help_heading = HEADING_OUTPUT)]
    format: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,

    #[command(flatten)]
    timeline: TimelineArgs,

    /// Exclude dirty (uncommitted-tree) snapshots from the analysis.
    #[arg(long, help_heading = HEADING_FILTER)]
    no_dirty: bool,

    /// Analysis mode: auto, history, branch, or tip (default: auto). `auto` infers
    /// history mode (long-range base-branch trends) from a clean base-branch
    /// checkout, and branch mode (this branch's latest state vs the base)
    /// otherwise. Use `tip` as a fast post-merge guard that only checks whether the
    /// base-branch tip just regressed against its recently established level,
    /// skipping the full-history scan.
    #[arg(long, value_name = "MODE", help_heading = HEADING_ANALYSIS)]
    mode: Option<String>,

    /// In history mode, also report sustained improvements (by default only
    /// regressions are reported, since improvement over time is expected). Branch
    /// and tip modes always report all findings, so this flag has no effect there.
    #[arg(long, help_heading = HEADING_ANALYSIS)]
    include_improvements: bool,

    /// In history mode, also report inactive findings: a change the current state
    /// no longer reflects (a regression that has since recovered). Hidden by
    /// default since they need no action. Branch and tip modes always report all
    /// findings, so this flag has no effect there.
    #[arg(long, help_heading = HEADING_ANALYSIS)]
    include_inactive: bool,
}

impl AnalyzeCommand {
    fn into_options(self) -> AnalyzeOptions {
        AnalyzeOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            context: self.timeline.context,
            base: self.timeline.base,
            no_dirty: self.no_dirty,
            since: self.timeline.since,
            until: self.timeline.until,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            prefixes: self.prefixes,
            format: self.format,
            mode: self.mode,
            include_improvements: self.include_improvements,
            include_inactive: self.include_inactive,
            verbose: self.env.verbose,
            timing: false,
        }
    }
}

/// What a `list` invocation enumerates.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ListSubjectArg {
    /// The runs that would enter a matching `analyze` pass.
    Runs,
    /// Every discriminant set present in storage (no repository required); a
    /// discovery catalog that lists all partitions regardless of the current
    /// machine. Pass a facet to narrow it.
    Discriminants,
    /// The blessings recorded at the current commit (or, with `--all`, across the
    /// whole analysis window).
    Blessings,
}

impl From<ListSubjectArg> for ListSubject {
    fn from(subject: ListSubjectArg) -> Self {
        match subject {
            ListSubjectArg::Runs => Self::Runs,
            ListSubjectArg::Discriminants => Self::Discriminants,
            ListSubjectArg::Blessings => Self::Blessings,
        }
    }
}

/// List the data set a matching `analyze` would include, without analyzing it.
#[derive(Args, Debug)]
struct ListCommand {
    /// What to list: `runs`, `discriminants`, or `blessings`.
    #[arg(value_name = "runs|discriminants|blessings")]
    subject: ListSubjectArg,

    #[command(flatten)]
    env: EnvArgs,

    /// Output format: text, json, or markdown (default: text).
    #[arg(long, value_name = "FORMAT", help_heading = HEADING_OUTPUT)]
    format: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,

    #[command(flatten)]
    timeline: TimelineArgs,

    /// Exclude dirty (uncommitted-tree) snapshots from the listing.
    #[arg(long, help_heading = HEADING_FILTER)]
    no_dirty: bool,

    /// With the `blessings` subject, list the most recent blessing of every
    /// benchmark across the whole analysis window rather than only those at the
    /// current commit. Errors if given with any other subject.
    #[arg(long, help_heading = HEADING_FILTER)]
    all: bool,
}

impl ListCommand {
    fn into_options(self) -> ListOptions {
        ListOptions {
            subject: self.subject.into(),
            config_path: self.env.config,
            repo: self.env.repo,
            context: self.timeline.context,
            base: self.timeline.base,
            no_dirty: self.no_dirty,
            since: self.timeline.since,
            until: self.timeline.until,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            format: self.format,
            all: self.all,
            verbose: self.env.verbose,
        }
    }
}

/// Delete stored runs (and their blessing sidecars) from the data set a matching
/// `analyze`/`list` would resolve.
///
/// You must say which kinds of run to delete with `--clean`, `--dirty`, or
/// `--all`. Pruning walks the selected commits from `--context` back to `--base`;
/// deleting the base branch's own data set requires the `--prune-base` guard.
#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("prune-kind")
        .args(["clean", "dirty", "all"])
        .required(true)
))]
struct PruneCommand {
    /// Restrict removal to these commits (a full or short SHA, prefix-matched);
    /// repeatable (default: every one of the selected commits).
    #[arg(value_name = "COMMIT")]
    commit: Vec<String>,

    #[command(flatten)]
    env: EnvArgs,

    /// Preview what would be removed without deleting anything.
    #[arg(long, help_heading = HEADING_ENV)]
    dry_run: bool,

    /// Confirm pruning the base branch's own data set (required when `--context`
    /// resolves to the same commit as `--base`).
    #[arg(long, help_heading = HEADING_ENV)]
    prune_base: bool,

    /// Output format: text, json, or markdown (default: text).
    #[arg(long, value_name = "FORMAT", help_heading = HEADING_OUTPUT)]
    format: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,

    #[command(flatten)]
    commit_selection: PruneCommitArgs,

    /// Remove only clean runs and their blessing sidecars.
    #[arg(long, help_heading = HEADING_FILTER)]
    clean: bool,

    /// Remove only dirty (uncommitted-tree) snapshots.
    #[arg(long, help_heading = HEADING_FILTER)]
    dirty: bool,

    /// Remove both clean runs (with their blessing sidecars) and dirty snapshots
    /// (the same as `--clean --dirty`).
    #[arg(long, help_heading = HEADING_FILTER)]
    all: bool,
}

/// Commit selection for `prune`: the range of commits whose data is removed.
#[derive(Args, Debug)]
#[command(next_help_heading = HEADING_COMMIT)]
struct PruneCommitArgs {
    /// Target ref whose data set is pruned, walking back until the base ref
    /// (defaults to HEAD).
    #[arg(long, value_name = "REF")]
    context: Option<String>,

    /// Base ref that the context branched off from; pruning stops on reaching it
    /// (defaults to the default branch).
    #[arg(long, value_name = "REF")]
    base: Option<String>,

    /// Only prune commits made on or after this cutoff: an RFC 3339 timestamp, a
    /// `YYYY-MM-DD` date, or a relative duration such as `6 months ago`.
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,

    /// Only prune commits made on or before this cutoff (same formats as
    /// `--since`).
    #[arg(long, value_name = "WHEN")]
    until: Option<String>,
}

impl PruneCommand {
    fn into_options(self) -> PruneOptions {
        // `--all` is the union of the two specific kinds.
        let clean = self.clean || self.all;
        let dirty = self.dirty || self.all;
        PruneOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            context: self.commit_selection.context,
            base: self.commit_selection.base,
            commit: self.commit,
            since: self.commit_selection.since,
            until: self.commit_selection.until,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            clean,
            dirty,
            prune_base: self.prune_base,
            dry_run: self.dry_run,
            format: self.format,
            verbose: self.env.verbose,
        }
    }
}

/// Replay `run` across a range of historical commits.
#[derive(Args, Debug)]
struct BackfillCommand {
    /// Oldest commit of the range to backfill, inclusive; a SHA, tag, or ref such
    /// as `HEAD~20`.
    #[arg(value_name = "FROM")]
    from: String,

    /// Newest commit of the range to backfill, inclusive; a SHA, tag, or ref such
    /// as `HEAD`. Must be reachable from `<FROM>` along first-parent history.
    #[arg(value_name = "TO")]
    to: String,

    #[command(flatten)]
    env: EnvArgs,

    /// Override the machine fingerprint used to partition hardware-dependent
    /// results (for example, a CI machine-pool name).
    #[arg(long, value_name = "KEY", help_heading = HEADING_DISCRIMINANT)]
    machine_key: Option<String>,

    /// Benchmark the entire workspace (the default when no `--package` is given);
    /// conflicts with `--package`.
    #[arg(long, help_heading = HEADING_SCOPE, conflicts_with = "package")]
    workspace: bool,

    /// Benchmark only this package; repeatable, e.g. `-p nm -p many_cpus`
    /// (default: the whole workspace).
    #[arg(long = "package", short = 'p', value_name = "NAME", help_heading = HEADING_SCOPE)]
    package: Vec<String>,

    /// Exclude a package from a whole-workspace run; repeatable, e.g.
    /// `--exclude nm --exclude many_cpus`. Conflicts with `--package`.
    #[arg(long, value_name = "NAME", help_heading = HEADING_SCOPE, conflicts_with = "package")]
    exclude: Vec<String>,

    /// Benchmark only this bench target; repeatable (default: every bench target).
    #[arg(long, value_name = "NAME", help_heading = HEADING_SCOPE)]
    bench: Vec<String>,

    /// Activate cargo features for the benchmark build; space- or comma-separated,
    /// repeatable. Forwarded verbatim to `cargo bench` as `--features`.
    #[arg(long, value_name = "FEATURES", help_heading = HEADING_FEATURES)]
    features: Vec<String>,

    /// Activate all cargo features of all selected packages (`--all-features`).
    #[arg(long, help_heading = HEADING_FEATURES)]
    all_features: bool,

    /// Do not activate the `default` cargo feature (`--no-default-features`).
    #[arg(long, help_heading = HEADING_FEATURES)]
    no_default_features: bool,

    /// Replace already-stored results for the backfilled commits instead of
    /// skipping them as duplicates.
    #[arg(long, help_heading = HEADING_ENV)]
    overwrite: bool,

    /// Continue past commits whose build or benchmark fails instead of stopping.
    #[arg(long, help_heading = HEADING_ENV)]
    ignore_errors: bool,

    /// Arguments after `--` forwarded verbatim to `cargo bench` after the scope
    /// flags.
    #[arg(last = true, value_name = "ARGS")]
    passthrough: Vec<String>,
}

impl BackfillCommand {
    fn into_options(self) -> BackfillOptions {
        BackfillOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            from: self.from,
            to: self.to,
            packages: resolve_packages(self.workspace, self.package),
            excludes: self.exclude,
            benches: self.bench,
            features: self.features,
            all_features: self.all_features,
            no_default_features: self.no_default_features,
            machine_key: self.machine_key,
            overwrite: self.overwrite,
            ignore_errors: self.ignore_errors,
            passthrough: self.passthrough,
            verbose: self.env.verbose,
        }
    }
}

/// Accept a benchmark's current level on the base branch as intentional.
#[derive(Args, Debug)]
struct BlessCommand {
    /// Benchmark-id prefixes to accept, matched against the qualified identity
    /// (for example, `all_the_time/read_cell` or a family prefix
    /// `overhead/groups_`). At least one is required unless `--all` is given.
    #[arg(value_name = "PREFIX")]
    prefixes: Vec<BenchmarkIdPrefix>,

    #[command(flatten)]
    env: EnvArgs,

    /// Accept every benchmark recorded at the context commit, with no prefixes.
    #[arg(long, conflicts_with = "prefixes")]
    all: bool,

    /// Commit to bless (defaults to HEAD). Use this to bless a commit other than
    /// the one currently checked out.
    #[arg(long, value_name = "REF", help_heading = HEADING_COMMIT)]
    context: Option<String>,

    /// Base ref the context commit must be on (defaults to the default branch).
    #[arg(long, value_name = "REF", help_heading = HEADING_COMMIT)]
    base: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,
}

impl BlessCommand {
    fn into_options(self) -> BlessOptions {
        BlessOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            context: self.context,
            base: self.base,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            prefixes: self.prefixes,
            all: self.all,
            verbose: self.env.verbose,
        }
    }
}

/// Remove blessings recorded at the context commit.
///
/// Only blessings recorded at the context commit are removed. Blessings issued
/// at later commits remain in effect, so the timeline may still be blessed past
/// the context commit.
#[derive(Args, Debug)]
struct UnblessCommand {
    #[command(flatten)]
    env: EnvArgs,

    /// Commit to unbless (defaults to HEAD). Use this to unbless a commit other
    /// than the one currently checked out.
    #[arg(long, value_name = "REF", help_heading = HEADING_COMMIT)]
    context: Option<String>,

    /// Base ref the context commit must be on (defaults to the default branch).
    #[arg(long, value_name = "REF", help_heading = HEADING_COMMIT)]
    base: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,
}

impl UnblessCommand {
    fn into_options(self) -> UnblessOptions {
        UnblessOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            context: self.context,
            base: self.base,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            verbose: self.env.verbose,
        }
    }
}

/// Resolves the benchmark scope. `--workspace` and `--package` are mutually
/// exclusive at the CLI level, so `--workspace` simply yields the empty package
/// list that means "the whole workspace" — the same as passing no scope at all.
fn resolve_packages(workspace: bool, package: Vec<String>) -> Vec<String> {
    if workspace { Vec::new() } else { package }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        Cli::from_args(&["cargo-bench-history"], args)
            .unwrap()
            .into_command()
    }

    #[test]
    fn cli_is_debug_formatted() {
        let cli = Cli::from_args(&["cargo-bench-history"], &["run"]).unwrap();
        assert!(format!("{cli:?}").contains("Run"), "{cli:?}");
    }

    #[test]
    fn help_lists_every_command() {
        let help = Cli::help("cargo-bench-history");
        assert!(!help.is_empty(), "help text is non-empty");
        for command in [
            "analyze", "backfill", "bless", "install", "list", "prune", "run", "unbless",
        ] {
            assert!(help.contains(command), "help lists {command}: {help}");
        }
    }

    #[test]
    fn run_collects_scope_and_passthrough() {
        let command = parse(&[
            "run",
            "--package",
            "nm",
            "-p",
            "many_cpus",
            "--bench",
            "nm_observe",
            "--",
            "--noplot",
        ]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(
            options.packages,
            vec!["nm".to_owned(), "many_cpus".to_owned()]
        );
        assert_eq!(options.benches, vec!["nm_observe".to_owned()]);
        assert_eq!(options.passthrough, vec!["--noplot".to_owned()]);
        assert!(!options.overwrite);
    }

    #[test]
    fn run_workspace_and_package_conflict() {
        let error = Cli::from_args(
            &["cargo-bench-history"],
            &["run", "--workspace", "-p", "nm"],
        )
        .unwrap_err();
        assert_eq!(error.status, Err(()));
        assert!(
            error.output.contains("cannot be used with"),
            "{}",
            error.output
        );
    }

    #[test]
    fn run_collects_exclude_filters() {
        let command = parse(&["run", "--exclude", "nm", "--exclude", "many_cpus"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert!(
            options.packages.is_empty(),
            "exclude implies workspace scope"
        );
        assert_eq!(
            options.excludes,
            vec!["nm".to_owned(), "many_cpus".to_owned()]
        );
    }

    #[test]
    fn run_collects_feature_selection() {
        let command = parse(&[
            "run",
            "--features",
            "foo,bar",
            "--features",
            "baz",
            "--no-default-features",
        ]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(
            options.features,
            vec!["foo,bar".to_owned(), "baz".to_owned()]
        );
        assert!(!options.all_features);
        assert!(options.no_default_features);
    }

    #[test]
    fn run_collects_all_features() {
        let command = parse(&["run", "--all-features"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert!(options.all_features);
        assert!(options.features.is_empty());
    }

    #[test]
    fn run_exclude_and_package_conflict() {
        let error = Cli::from_args(
            &["cargo-bench-history"],
            &["run", "--exclude", "nm", "-p", "many_cpus"],
        )
        .unwrap_err();
        assert_eq!(error.status, Err(()));
        assert!(
            error.output.contains("cannot be used with"),
            "{}",
            error.output
        );
    }

    #[test]
    fn backfill_workspace_and_package_conflict() {
        let error = Cli::from_args(
            &["cargo-bench-history"],
            &["backfill", "abc", "def", "--workspace", "-p", "nm"],
        )
        .unwrap_err();
        assert_eq!(error.status, Err(()));
        assert!(
            error.output.contains("cannot be used with"),
            "{}",
            error.output
        );
    }

    #[test]
    fn backfill_collects_exclude_filters() {
        let command = parse(&["backfill", "abc", "def", "--exclude", "nm"]);
        let Command::Backfill(options) = command else {
            panic!("expected backfill command");
        };
        assert!(
            options.packages.is_empty(),
            "exclude implies workspace scope"
        );
        assert_eq!(options.excludes, vec!["nm".to_owned()]);
    }

    #[test]
    fn backfill_collects_feature_selection() {
        let command = parse(&[
            "backfill",
            "abc",
            "def",
            "--features",
            "foo",
            "--all-features",
        ]);
        let Command::Backfill(options) = command else {
            panic!("expected backfill command");
        };
        assert_eq!(options.features, vec!["foo".to_owned()]);
        assert!(options.all_features);
        assert!(!options.no_default_features);
    }

    #[test]
    fn backfill_exclude_and_package_conflict() {
        let error = Cli::from_args(
            &["cargo-bench-history"],
            &[
                "backfill",
                "abc",
                "def",
                "--exclude",
                "nm",
                "-p",
                "many_cpus",
            ],
        )
        .unwrap_err();
        assert_eq!(error.status, Err(()));
        assert!(
            error.output.contains("cannot be used with"),
            "{}",
            error.output
        );
    }

    #[test]
    fn run_parses_overwrite_switch() {
        let command = parse(&["run", "--overwrite"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert!(options.overwrite);
    }

    #[test]
    fn run_parses_repo() {
        let command = parse(&["run", "--repo", "/work/folo"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
    }

    #[test]
    fn run_parses_machine_key_override() {
        let command = parse(&["run", "--machine-key", "ci-pool-a"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(options.machine_key.as_deref(), Some("ci-pool-a"));
    }

    #[test]
    fn run_parses_verbose_switch() {
        let Command::Run(options) = parse(&["run", "--verbose"]) else {
            panic!("expected run command");
        };
        assert!(options.verbose);

        let Command::Run(options) = parse(&["run"]) else {
            panic!("expected run command");
        };
        assert!(!options.verbose);
    }

    #[test]
    fn install_maps_to_install_command() {
        let command = parse(&["install"]);
        assert_eq!(command, Command::Install(InstallOptions::default()));
    }

    #[test]
    fn install_captures_config_path() {
        let command = parse(&["install", "--config", "custom/bench.toml"]);
        let Command::Install(options) = command else {
            panic!("expected install command");
        };
        assert_eq!(
            options.config_path,
            Some(PathBuf::from("custom/bench.toml"))
        );
    }

    #[test]
    fn install_parses_verbose_switch() {
        let Command::Install(options) = parse(&["install", "--verbose"]) else {
            panic!("expected install command");
        };
        assert!(options.verbose);

        let Command::Install(options) = parse(&["install"]) else {
            panic!("expected install command");
        };
        assert!(!options.verbose);
    }

    #[test]
    fn analyze_parses_verbose_switch() {
        let Command::Analyze(options) = parse(&["analyze", "--verbose"]) else {
            panic!("expected analyze command");
        };
        assert!(options.verbose);

        let Command::Analyze(options) = parse(&["analyze"]) else {
            panic!("expected analyze command");
        };
        assert!(!options.verbose);
    }

    #[test]
    fn analyze_collects_switches() {
        let command = parse(&["analyze", "--include-improvements", "--mode", "history"]);
        let Command::Analyze(options) = command else {
            panic!("expected analyze command");
        };
        assert!(options.include_improvements);
        assert_eq!(options.mode.as_deref(), Some("history"));
    }

    #[test]
    fn analyze_collects_topology_and_repeatable_facets() {
        let command = parse(&[
            "analyze",
            "--repo",
            "/work/folo",
            "--context",
            "feature",
            "--base",
            "master",
            "--until",
            "2024-06-01T00:00:00Z",
            "--no-dirty",
            "--engine",
            "callgrind",
            "--engine",
            "criterion",
            "--target-triple",
            "all",
            "--machine-key",
            "ci-pool",
        ]);
        let Command::Analyze(options) = command else {
            panic!("expected analyze command");
        };
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
        assert_eq!(options.context.as_deref(), Some("feature"));
        assert_eq!(options.base.as_deref(), Some("master"));
        assert_eq!(options.until.as_deref(), Some("2024-06-01T00:00:00Z"));
        assert!(options.no_dirty);
        assert_eq!(
            options.engine,
            vec!["callgrind".to_owned(), "criterion".to_owned()]
        );
        assert_eq!(options.target_triple, vec!["all".to_owned()]);
        assert_eq!(options.machine_key, vec!["ci-pool".to_owned()]);
    }

    #[test]
    fn analyze_facets_default_to_empty() {
        let Command::Analyze(options) = parse(&["analyze"]) else {
            panic!("expected analyze command");
        };
        assert!(options.engine.is_empty());
        assert!(options.target_triple.is_empty());
        assert!(options.machine_key.is_empty());
        assert!(options.until.is_none());
    }

    #[test]
    fn analyze_parses_include_inactive_switch() {
        let Command::Analyze(options) = parse(&["analyze", "--include-inactive"]) else {
            panic!("expected analyze command");
        };
        assert!(options.include_inactive);

        let Command::Analyze(options) = parse(&["analyze"]) else {
            panic!("expected analyze command");
        };
        assert!(!options.include_inactive);
    }

    #[test]
    fn list_requires_a_subject() {
        let parsed = Cli::from_args(&["cargo-bench-history"], &["list"]);
        let early = parsed.unwrap_err();
        assert!(early.status.is_err(), "a missing subject is a parse error");
        for subject in ["runs", "discriminants", "blessings"] {
            assert!(
                early.output.contains(subject),
                "the error names the {subject} subject: {}",
                early.output
            );
        }
    }

    #[test]
    fn list_runs_collects_selection() {
        let command = parse(&[
            "list",
            "runs",
            "--repo",
            "/work/folo",
            "--context",
            "feature",
            "--base",
            "master",
            "--no-dirty",
            "--engine",
            "callgrind",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "ci-pool",
            "--format",
            "json",
            "--verbose",
        ]);
        let Command::List(options) = command else {
            panic!("expected list command");
        };
        assert_eq!(options.subject, ListSubject::Runs);
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
        assert_eq!(options.context.as_deref(), Some("feature"));
        assert_eq!(options.base.as_deref(), Some("master"));
        assert!(options.no_dirty);
        assert_eq!(options.engine, vec!["callgrind".to_owned()]);
        assert_eq!(
            options.target_triple,
            vec!["x86_64-unknown-linux-gnu".to_owned()]
        );
        assert_eq!(options.machine_key, vec!["ci-pool".to_owned()]);
        assert_eq!(options.format.as_deref(), Some("json"));
        assert!(options.verbose);
    }

    #[test]
    fn list_discriminants_selects_the_subject() {
        let Command::List(options) = parse(&["list", "discriminants"]) else {
            panic!("expected list command");
        };
        assert_eq!(options.subject, ListSubject::Discriminants);
    }

    #[test]
    fn list_blessings_collects_all_switch() {
        let Command::List(options) = parse(&["list", "blessings", "--all"]) else {
            panic!("expected list command");
        };
        assert_eq!(options.subject, ListSubject::Blessings);
        assert!(options.all);

        let Command::List(options) = parse(&["list", "blessings"]) else {
            panic!("expected list command");
        };
        assert!(!options.all);
    }

    #[test]
    fn bless_collects_prefixes_facets_and_context() {
        let command = parse(&[
            "bless",
            "--engine",
            "callgrind",
            "--context",
            "abc123",
            "all_the_time/read_cell",
            "overhead/groups_",
        ]);
        let Command::Bless(options) = command else {
            panic!("expected bless command");
        };
        assert_eq!(
            options.prefixes,
            vec![
                BenchmarkIdPrefix::new("all_the_time/read_cell").unwrap(),
                BenchmarkIdPrefix::new("overhead/groups_").unwrap()
            ]
        );
        assert_eq!(options.engine, vec!["callgrind".to_owned()]);
        assert_eq!(options.context.as_deref(), Some("abc123"));
        assert!(!options.all);
    }

    #[test]
    fn bless_all_switch_needs_no_prefixes() {
        let Command::Bless(options) = parse(&["bless", "--all"]) else {
            panic!("expected bless command");
        };
        assert!(options.all);
        assert!(options.prefixes.is_empty());
    }

    #[test]
    fn bless_all_conflicts_with_prefixes() {
        let error =
            Cli::from_args(&["cargo-bench-history"], &["bless", "--all", "foo/bar"]).unwrap_err();
        assert_eq!(error.status, Err(()));
        assert!(
            error.output.contains("cannot be used with"),
            "{}",
            error.output
        );
    }

    #[test]
    fn bless_rejects_an_empty_prefix() {
        let error = Cli::from_args(&["cargo-bench-history"], &["bless", ""]).unwrap_err();
        assert_eq!(error.status, Err(()));
        assert!(
            error.output.contains("benchmark-id prefix"),
            "{}",
            error.output
        );
    }

    #[test]
    fn unbless_parses_facets() {
        let command = parse(&[
            "unbless",
            "--context",
            "abc123",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "ci-pool",
        ]);
        let Command::Unbless(options) = command else {
            panic!("expected unbless command");
        };
        assert_eq!(options.context.as_deref(), Some("abc123"));
        assert_eq!(
            options.target_triple,
            vec!["x86_64-unknown-linux-gnu".to_owned()]
        );
        assert_eq!(options.machine_key, vec!["ci-pool".to_owned()]);
    }

    #[test]
    fn prune_collects_commits_selection_and_dry_run() {
        let command = parse(&[
            "prune",
            "abc123",
            "def456",
            "--repo",
            "/work/folo",
            "--context",
            "feature",
            "--base",
            "master",
            "--since",
            "2024-01-01T00:00:00Z",
            "--until",
            "2024-06-01T00:00:00Z",
            "--engine",
            "callgrind",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "ci-pool",
            "--dirty",
            "--dry-run",
            "--format",
            "json",
            "--verbose",
        ]);
        let Command::Prune(options) = command else {
            panic!("expected prune command");
        };
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
        assert_eq!(options.context.as_deref(), Some("feature"));
        assert_eq!(options.base.as_deref(), Some("master"));
        assert_eq!(
            options.commit,
            vec!["abc123".to_owned(), "def456".to_owned()]
        );
        assert_eq!(options.since.as_deref(), Some("2024-01-01T00:00:00Z"));
        assert_eq!(options.until.as_deref(), Some("2024-06-01T00:00:00Z"));
        assert_eq!(options.engine, vec!["callgrind".to_owned()]);
        assert_eq!(
            options.target_triple,
            vec!["x86_64-unknown-linux-gnu".to_owned()]
        );
        assert_eq!(options.machine_key, vec!["ci-pool".to_owned()]);
        assert!(options.dirty);
        assert!(!options.clean);
        assert!(options.dry_run);
        assert_eq!(options.format.as_deref(), Some("json"));
        assert!(options.verbose);
    }

    #[test]
    fn prune_all_expands_to_clean_and_dirty() {
        let command = parse(&[
            "prune",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--all",
        ]);
        let Command::Prune(options) = command else {
            panic!("expected prune command");
        };
        assert_eq!(
            options.target_triple,
            vec!["x86_64-unknown-linux-gnu".to_owned()]
        );
        assert!(options.commit.is_empty());
        assert!(options.clean, "--all enables clean removal");
        assert!(options.dirty, "--all enables dirty removal");
        assert!(!options.dry_run);
    }

    #[test]
    fn backfill_collects_range_and_passthrough() {
        let command = parse(&[
            "backfill",
            "abc123",
            "def456",
            "--package",
            "nm",
            "--bench",
            "nm_observe",
            "--machine-key",
            "ci-pool",
            "--overwrite",
            "--ignore-errors",
            "--",
            "--noplot",
        ]);
        let Command::Backfill(options) = command else {
            panic!("expected backfill command");
        };
        assert_eq!(options.from, "abc123");
        assert_eq!(options.to, "def456");
        assert_eq!(options.packages, vec!["nm".to_owned()]);
        assert_eq!(options.benches, vec!["nm_observe".to_owned()]);
        assert_eq!(options.machine_key.as_deref(), Some("ci-pool"));
        assert!(options.overwrite);
        assert!(options.ignore_errors);
        assert_eq!(options.passthrough, vec!["--noplot".to_owned()]);
    }

    #[test]
    fn backfill_requires_from_and_to() {
        let parsed = Cli::from_args(&["cargo-bench-history"], &["backfill", "abc123"]);
        assert!(parsed.is_err(), "a missing `to` must be rejected");
    }

    #[test]
    fn backfill_parses_verbose_switch() {
        let Command::Backfill(options) = parse(&["backfill", "abc123", "def456", "--verbose"])
        else {
            panic!("expected backfill command");
        };
        assert!(options.verbose);

        let Command::Backfill(options) = parse(&["backfill", "abc123", "def456"]) else {
            panic!("expected backfill command");
        };
        assert!(!options.verbose);
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        Cli::from_args(&["cargo-bench-history"], &["frobnicate"]).unwrap_err();
    }

    #[test]
    fn run_rejects_unknown_flag() {
        Cli::from_args(&["cargo-bench-history"], &["run", "--frobnicate"]).unwrap_err();
    }

    #[test]
    fn help_request_lists_subcommands() {
        let early_exit = Cli::from_args(&["cargo-bench-history"], &["--help"]).unwrap_err();
        assert!(
            early_exit.output.contains("run"),
            "help should list subcommands: {}",
            early_exit.output
        );
        assert!(
            early_exit.output.contains("install"),
            "{}",
            early_exit.output
        );
    }

    #[test]
    fn help_text_describes_each_command_in_alphabetical_order() {
        let help = Cli::help("cargo-bench-history");

        // Each command is accompanied by its description, not just its bare name.
        assert!(
            help.contains("Analyze stored history"),
            "help should describe `analyze`: {help}"
        );
        assert!(
            help.contains("Replay `run` across a range"),
            "help should describe `backfill`: {help}"
        );

        // The commands appear in alphabetical order. Each marker is a distinct,
        // non-overlapping substring, so the offsets are strictly increasing
        // exactly when they are sorted.
        let order = ["analyze", "backfill", "install", "list", "prune", "run"];
        let positions: Vec<usize> = order
            .iter()
            .map(|name| help.find(&format!("\n  {name} ")).unwrap())
            .collect();
        assert!(
            positions.is_sorted(),
            "commands should be listed alphabetically: {help}"
        );
    }
}
