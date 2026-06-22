//! Argument parsing: the `clap` subcommand surface and its translation into the
//! typed [`Command`](crate::Command) model.
//!
//! The wide option surface is organized into named help groups (Environment,
//! Output, Discriminant selection, Timeline selection, Data filtering, Benchmark
//! scope) so `--help` makes the relationship between options legible. Shared
//! groups are factored into [`clap::Args`] structs and `#[command(flatten)]`ed
//! into each subcommand so the same option means the same thing everywhere.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand as ClapSubcommand, ValueEnum};
use jiff::Timestamp;

use crate::{
    AnalyzeOptions, BackfillOptions, BlessOptions, Command, InstallOptions, ListOptions,
    ListSubject, PruneOptions, RunOptions, UnblessOptions,
};

const HEADING_ENV: &str = "Environment and execution";
const HEADING_OUTPUT: &str = "Output";
const HEADING_DISCRIMINANT: &str = "Discriminant selection";
const HEADING_TIMELINE: &str = "Timeline selection";
const HEADING_FILTER: &str = "Data filtering";
const HEADING_SCOPE: &str = "Benchmark scope";
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
#[non_exhaustive]
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

    /// Restrict to these machine partitions (repeatable; `all` matches every
    /// machine; default: this machine's fingerprint).
    #[arg(long, value_name = "KEY")]
    machine_key: Vec<String>,
}

/// Timeline selection shared by `analyze`/`list`/`prune`.
#[derive(Args, Debug)]
#[command(next_help_heading = HEADING_TIMELINE)]
struct TimelineArgs {
    /// Target ref whose history is used (defaults to HEAD).
    #[arg(long, value_name = "REF")]
    context: Option<String>,

    /// Base ref the target's history is split at (defaults to the default branch).
    #[arg(long, value_name = "REF")]
    base: Option<String>,

    /// Only consider runs on or after this cutoff: an RFC 3339 timestamp, a
    /// `YYYY-MM-DD` date, or a relative duration such as `6 months ago`.
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,

    /// Only consider runs on or before this cutoff (same formats as `--since`).
    #[arg(long, value_name = "WHEN")]
    until: Option<String>,
}

/// Run the workspace benchmarks (`cargo bench`) and store the results.
#[derive(Args, Debug)]
struct RunCommand {
    #[command(flatten)]
    env: EnvArgs,

    /// Benchmark the entire workspace; overrides `--package` (the default when no
    /// `--package` is given).
    #[arg(long, help_heading = HEADING_SCOPE)]
    workspace: bool,

    /// Benchmark only this package; repeatable, e.g. `-p nm -p many_cpus`
    /// (default: the whole workspace).
    #[arg(long = "package", short = 'p', value_name = "NAME", help_heading = HEADING_SCOPE)]
    package: Vec<String>,

    /// Benchmark only this bench target; repeatable (default: every bench target).
    #[arg(long, value_name = "NAME", help_heading = HEADING_SCOPE)]
    bench: Vec<String>,

    /// Override the machine fingerprint used to partition hardware-dependent
    /// results (for example, a CI machine-pool name).
    #[arg(long, value_name = "KEY", help_heading = HEADING_DISCRIMINANT)]
    machine_key: Option<String>,

    /// Override the effective timestamp, in RFC 3339 format
    /// (`2024-01-31T14:30:00Z`); used when backfilling history (default: the
    /// commit time for a clean run, otherwise the current time).
    #[arg(long, value_name = "RFC3339", help_heading = HEADING_ENV)]
    timestamp: Option<Timestamp>,

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
            benches: self.bench,
            timestamp: self.timestamp,
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

    /// Restrict analysis to a single metric name (for example, Ir).
    #[arg(long, value_name = "NAME", help_heading = HEADING_FILTER)]
    metric: Option<String>,

    /// Analysis mode: auto, history, branch, or tip (default: auto). `auto` infers
    /// history mode (long-range trends on the base branch) from a clean checkout
    /// of the base branch, and branch mode (latest state vs the base) otherwise.
    /// `tip` is a fast guard check of the base-branch tip against the recently
    /// established level.
    #[arg(long, value_name = "MODE", help_heading = HEADING_ANALYSIS)]
    mode: Option<String>,

    /// In history mode, also report sustained improvements (by default only
    /// regressions are reported, since improvement over time is expected).
    #[arg(long, help_heading = HEADING_ANALYSIS)]
    include_improvements: bool,

    /// In history mode, also report inactive findings: a change the current state
    /// no longer reflects (a regression that has since recovered). Hidden by
    /// default since they need no action.
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
            metric: self.metric,
            format: self.format,
            mode: self.mode,
            include_improvements: self.include_improvements,
            include_inactive: self.include_inactive,
            verbose: self.env.verbose,
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

    /// Restrict the listing to a single metric name (for example, Ir).
    #[arg(long, value_name = "NAME", help_heading = HEADING_FILTER)]
    metric: Option<String>,

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
            metric: self.metric,
            format: self.format,
            all: self.all,
            verbose: self.env.verbose,
        }
    }
}

/// Delete stored runs (and their blessing sidecars) from the data set a matching
/// `analyze`/`list` would resolve; pass `--dry-run` to preview without deleting.
#[derive(Args, Debug)]
struct PruneCommand {
    /// Restrict removal to these commits (a full or short SHA, prefix-matched);
    /// repeatable (default: every commit on the resolved history).
    #[arg(value_name = "COMMIT")]
    commit: Vec<String>,

    #[command(flatten)]
    env: EnvArgs,

    /// Output format: text, json, or markdown (default: text).
    #[arg(long, value_name = "FORMAT", help_heading = HEADING_OUTPUT)]
    format: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,

    #[command(flatten)]
    timeline: TimelineArgs,

    /// Remove only dirty (uncommitted-tree) snapshots (mutually exclusive with
    /// `--clean`; exempt from the narrowing guard).
    #[arg(long, help_heading = HEADING_FILTER)]
    dirty: bool,

    /// Remove only clean runs and their blessing sidecars (mutually exclusive with
    /// `--dirty`).
    #[arg(long, help_heading = HEADING_FILTER)]
    clean: bool,

    /// Confirm deleting clean history across an un-narrowed selection (required
    /// when no facet, `<commit>`, `--since`, or `--until` narrows the range).
    #[arg(long, help_heading = HEADING_FILTER)]
    all: bool,

    /// Preview what would be removed without deleting anything.
    #[arg(long, help_heading = HEADING_FILTER)]
    dry_run: bool,
}

impl PruneCommand {
    fn into_options(self) -> PruneOptions {
        PruneOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            context: self.timeline.context,
            base: self.timeline.base,
            commit: self.commit,
            since: self.timeline.since,
            until: self.timeline.until,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            dirty: self.dirty,
            clean: self.clean,
            all: self.all,
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
    /// as `HEAD` (must be on the current branch's first-parent history).
    #[arg(value_name = "TO")]
    to: String,

    #[command(flatten)]
    env: EnvArgs,

    /// Benchmark the entire workspace; overrides `--package` (the default when no
    /// `--package` is given).
    #[arg(long, help_heading = HEADING_SCOPE)]
    workspace: bool,

    /// Benchmark only this package; repeatable, e.g. `-p nm -p many_cpus`
    /// (default: the whole workspace).
    #[arg(long = "package", short = 'p', value_name = "NAME", help_heading = HEADING_SCOPE)]
    package: Vec<String>,

    /// Benchmark only this bench target; repeatable (default: every bench target).
    #[arg(long, value_name = "NAME", help_heading = HEADING_SCOPE)]
    bench: Vec<String>,

    /// Override the machine fingerprint used to partition hardware-dependent
    /// results (for example, a CI machine-pool name).
    #[arg(long, value_name = "KEY", help_heading = HEADING_DISCRIMINANT)]
    machine_key: Option<String>,

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
            benches: self.bench,
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
    /// `overhead/groups_`). At least one is required.
    #[arg(value_name = "PREFIX")]
    prefixes: Vec<String>,

    #[command(flatten)]
    env: EnvArgs,

    /// Base ref the current commit must be on (defaults to the default branch).
    #[arg(long, value_name = "REF", help_heading = HEADING_TIMELINE)]
    base: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,

    /// Optional note recorded with the blessing explaining why the change is
    /// accepted.
    #[arg(long, value_name = "TEXT", help_heading = HEADING_ENV)]
    reason: Option<String>,
}

impl BlessCommand {
    fn into_options(self) -> BlessOptions {
        BlessOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            base: self.base,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            prefixes: self.prefixes,
            reason: self.reason,
            verbose: self.env.verbose,
        }
    }
}

/// Remove blessings recorded at the current commit.
#[derive(Args, Debug)]
struct UnblessCommand {
    #[command(flatten)]
    env: EnvArgs,

    /// Base ref the current commit must be on (defaults to the default branch).
    #[arg(long, value_name = "REF", help_heading = HEADING_TIMELINE)]
    base: Option<String>,

    #[command(flatten)]
    facets: QueryFacetArgs,
}

impl UnblessCommand {
    fn into_options(self) -> UnblessOptions {
        UnblessOptions {
            config_path: self.env.config,
            repo: self.env.repo,
            base: self.base,
            engine: self.facets.engine,
            target_triple: self.facets.target_triple,
            machine_key: self.facets.machine_key,
            verbose: self.env.verbose,
        }
    }
}

/// Resolves the benchmark scope: an explicit `--workspace` clears any `--package`
/// filters (an empty package list means the whole workspace).
fn resolve_packages(workspace: bool, package: Vec<String>) -> Vec<String> {
    if workspace { Vec::new() } else { package }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        Cli::from_args(&["cargo-bench-history"], args)
            .expect("arguments should parse")
            .into_command()
    }

    #[test]
    fn cli_is_debug_formatted() {
        let cli =
            Cli::from_args(&["cargo-bench-history"], &["run"]).expect("arguments should parse");
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
    fn run_workspace_flag_overrides_package_filters() {
        let command = parse(&["run", "--workspace", "-p", "nm"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        // An explicit --workspace clears the package scope.
        assert!(options.packages.is_empty());
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
    fn run_parses_repo_and_timestamp() {
        let command = parse(&[
            "run",
            "--repo",
            "/work/folo",
            "--timestamp",
            "2024-01-01T00:00:00Z",
        ]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
        let expected: Timestamp = "2024-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(options.timestamp, Some(expected));
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
        let early = parsed.expect_err("bare list must error");
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
            "--metric",
            "Ir",
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
        assert_eq!(options.metric.as_deref(), Some("Ir"));
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
    fn bless_collects_prefixes_facets_and_reason() {
        let command = parse(&[
            "bless",
            "--engine",
            "callgrind",
            "--reason",
            "intentional tradeoff",
            "all_the_time/read_cell",
            "overhead/groups_",
        ]);
        let Command::Bless(options) = command else {
            panic!("expected bless command");
        };
        assert_eq!(
            options.prefixes,
            vec![
                "all_the_time/read_cell".to_owned(),
                "overhead/groups_".to_owned()
            ]
        );
        assert_eq!(options.engine, vec!["callgrind".to_owned()]);
        assert_eq!(options.reason.as_deref(), Some("intentional tradeoff"));
    }

    #[test]
    fn unbless_parses_facets() {
        let command = parse(&[
            "unbless",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--machine-key",
            "ci-pool",
        ]);
        let Command::Unbless(options) = command else {
            panic!("expected unbless command");
        };
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
        assert!(!options.all);
        assert!(options.dry_run);
        assert_eq!(options.format.as_deref(), Some("json"));
        assert!(options.verbose);
    }

    #[test]
    fn prune_parses_all_confirm_flag() {
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
        assert!(options.all);
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
}
