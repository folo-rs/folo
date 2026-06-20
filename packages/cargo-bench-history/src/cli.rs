//! Argument parsing: the `argh` subcommand surface and its translation into the
//! typed [`Command`](crate::Command) model.

use std::path::PathBuf;

use argh::FromArgs;
use jiff::Timestamp;

use crate::{
    AnalyzeOptions, BackfillOptions, CleanOptions, Command, InstallOptions, ListOptions, RunOptions,
};

/// Maintain a history of benchmark results over time and analyze it for trends.
#[derive(Debug, FromArgs)]
pub struct Cli {
    /// the subcommand to execute.
    #[argh(subcommand)]
    command: Subcommand,
}

impl Cli {
    /// Translates the parsed arguments into the typed command model.
    #[must_use]
    pub fn into_command(self) -> Command {
        match self.command {
            Subcommand::Run(command) => Command::Run(command.into_options()),
            Subcommand::Install(command) => Command::Install(command.into_options()),
            Subcommand::Analyze(command) => Command::Analyze(command.into_options()),
            Subcommand::List(command) => Command::List(command.into_options()),
            Subcommand::Clean(command) => Command::Clean(command.into_options()),
            Subcommand::Backfill(command) => Command::Backfill(command.into_options()),
        }
    }
}

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
enum Subcommand {
    Run(RunCommand),
    Install(InstallCommand),
    Analyze(AnalyzeCommand),
    List(ListCommand),
    Clean(CleanCommand),
    Backfill(BackfillCommand),
}

/// Run the workspace benchmarks (`cargo bench`) and store the results.
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "run")]
struct RunCommand {
    /// path to the configuration file (defaults to `.cargo/bench_history.toml`).
    #[argh(option)]
    config: Option<PathBuf>,

    /// benchmark the entire workspace; overrides --package (this is the default
    /// when no --package is given).
    #[argh(switch)]
    workspace: bool,

    /// benchmark only this package; repeatable, for example `-p nm -p many_cpus`
    /// (default: the whole workspace).
    #[argh(option, short = 'p', long = "package")]
    package: Vec<String>,

    /// benchmark only this bench target; repeatable (default: every bench target).
    #[argh(option)]
    bench: Vec<String>,

    /// override the effective timestamp, in RFC 3339 format, for example
    /// `2024-01-31T14:30:00Z`; used when backfilling history (default: the
    /// commit time for a clean run, otherwise the current time).
    #[argh(option)]
    timestamp: Option<Timestamp>,

    /// override the recorded target triple used for partitioning.
    #[argh(option)]
    target_triple: Option<String>,

    /// override the machine fingerprint used to partition hardware-dependent
    /// results (for example, a CI machine-pool name).
    #[argh(option)]
    machine_key: Option<String>,

    /// harvest and build results without storing them.
    #[argh(switch)]
    no_store: bool,

    /// replace an already-stored result for this run instead of refusing it as
    /// a duplicate.
    #[argh(switch)]
    overwrite: bool,

    /// emit detailed diagnostic notes to standard error (which directories are
    /// scanned, which files are included or skipped, what is stored where).
    #[argh(switch)]
    verbose: bool,

    /// arguments after `--` forwarded verbatim to `cargo bench` after the scope
    /// flags.
    #[argh(positional, greedy)]
    passthrough: Vec<String>,
}

impl RunCommand {
    fn into_options(self) -> RunOptions {
        // An explicit `--workspace` benches the whole workspace, taking precedence
        // over any `--package` filters; otherwise the packages (possibly empty,
        // meaning the whole workspace) select the scope.
        let packages = if self.workspace {
            Vec::new()
        } else {
            self.package
        };
        RunOptions {
            config_path: self.config,
            packages,
            benches: self.bench,
            timestamp: self.timestamp,
            target_triple: self.target_triple,
            machine_key: self.machine_key,
            no_store: self.no_store,
            overwrite: self.overwrite,
            passthrough: strip_separator(self.passthrough),
            verbose: self.verbose,
        }
    }
}

/// Generate a starter configuration file.
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "install")]
struct InstallCommand {
    /// path to the configuration file to generate.
    #[argh(option)]
    config: Option<PathBuf>,

    /// emit detailed diagnostic notes to standard error (which path is written,
    /// or that an existing configuration was left unchanged).
    #[argh(switch)]
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
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "analyze")]
struct AnalyzeCommand {
    /// path to the configuration file (defaults to `.cargo/bench_history.toml`).
    #[argh(option)]
    config: Option<PathBuf>,

    /// repository to resolve git topology from (defaults to the working directory).
    #[argh(option)]
    repo: Option<PathBuf>,

    /// target ref whose history is analyzed (defaults to HEAD).
    #[argh(option)]
    branch: Option<String>,

    /// base ref the target's history is split at (defaults to the default branch).
    #[argh(option)]
    base: Option<String>,

    /// exclude dirty (uncommitted-tree) snapshots from the analysis.
    #[argh(switch)]
    no_dirty: bool,

    /// only consider runs on or after this cutoff: an RFC 3339 timestamp
    /// (`2024-01-01T00:00:00Z`), a `YYYY-MM-DD` date, or a relative duration
    /// such as `6 months` or `30 days ago` (default: no lower bound in branch
    /// mode; the last 6 months in history mode).
    #[argh(option)]
    since: Option<String>,

    /// restrict analysis to a single engine, criterion or callgrind
    /// (default: every engine).
    #[argh(option)]
    engine: Option<String>,

    /// restrict analysis to a single full target triple (for example,
    /// `x86_64-unknown-linux-gnu`). Mutually exclusive with `--os` /
    /// `--architecture`, which select the same dimension by its derived parts.
    #[argh(option)]
    target_triple: Option<String>,

    /// restrict analysis to a single operating system (for example, windows).
    /// Cannot be combined with `--target-triple`.
    #[argh(option)]
    os: Option<String>,

    /// restrict analysis to a single CPU architecture (for example, `x86_64`).
    /// Cannot be combined with `--target-triple`.
    #[argh(option)]
    architecture: Option<String>,

    /// restrict analysis to a single machine partition.
    #[argh(option)]
    machine_key: Option<String>,

    /// restrict analysis to a single metric name (for example, Ir).
    #[argh(option)]
    metric: Option<String>,

    /// output format: text, json, or markdown (default: text).
    #[argh(option)]
    format: Option<String>,

    /// analysis mode: auto, history, branch, or tip (default: auto). `auto`
    /// infers history mode (long-range trends on the base branch) from a clean
    /// checkout of the base branch, and branch mode (latest state vs the base)
    /// otherwise. `tip` is a fast guard check of the base-branch tip against the
    /// recently established level.
    #[argh(option)]
    mode: Option<String>,

    /// in history mode, also report sustained improvements (by default only
    /// regressions are reported, since improvement over time is expected).
    #[argh(switch)]
    include_improvements: bool,

    /// emit detailed diagnostic notes to standard error (which storage prefix is
    /// listed, which objects are included or excluded and why, the resolved git
    /// topology).
    #[argh(switch)]
    verbose: bool,
}

impl AnalyzeCommand {
    fn into_options(self) -> AnalyzeOptions {
        AnalyzeOptions {
            config_path: self.config,
            repo: self.repo,
            branch: self.branch,
            base: self.base,
            no_dirty: self.no_dirty,
            since: self.since,
            engine: self.engine,
            target_triple: self.target_triple,
            os: self.os,
            architecture: self.architecture,
            machine_key: self.machine_key,
            metric: self.metric,
            format: self.format,
            mode: self.mode,
            include_improvements: self.include_improvements,
            verbose: self.verbose,
        }
    }
}

/// List the data set a matching `analyze` would include, without analyzing it.
///
/// The selection options mirror `analyze` exactly, so the listing previews the
/// exact runs an `analyze` with the same flags would consume — grouped by
/// comparable discriminant set, with the run, series, and commit counts of each.
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "list")]
struct ListCommand {
    /// path to the configuration file (defaults to `.cargo/bench_history.toml`).
    #[argh(option)]
    config: Option<PathBuf>,

    /// repository to resolve git topology from (defaults to the working directory).
    #[argh(option)]
    repo: Option<PathBuf>,

    /// target ref whose history is listed (defaults to HEAD).
    #[argh(option)]
    branch: Option<String>,

    /// base ref the target's history is split at (defaults to the default branch).
    #[argh(option)]
    base: Option<String>,

    /// exclude dirty (uncommitted-tree) snapshots from the listing.
    #[argh(switch)]
    no_dirty: bool,

    /// only list runs on or after this cutoff: an RFC 3339 timestamp
    /// (`2024-01-01T00:00:00Z`), a `YYYY-MM-DD` date, or a relative duration
    /// such as `6 months` or `30 days ago` (default: no lower bound).
    #[argh(option)]
    since: Option<String>,

    /// restrict the listing to a single engine, criterion or callgrind
    /// (default: every engine).
    #[argh(option)]
    engine: Option<String>,

    /// restrict the listing to a single full target triple (for example,
    /// `x86_64-unknown-linux-gnu`). Mutually exclusive with `--os` /
    /// `--architecture`, which select the same dimension by its derived parts.
    #[argh(option)]
    target_triple: Option<String>,

    /// restrict the listing to a single operating system (for example, windows).
    /// Cannot be combined with `--target-triple`.
    #[argh(option)]
    os: Option<String>,

    /// restrict the listing to a single CPU architecture (for example, `x86_64`).
    /// Cannot be combined with `--target-triple`.
    #[argh(option)]
    architecture: Option<String>,

    /// restrict the listing to a single machine partition.
    #[argh(option)]
    machine_key: Option<String>,

    /// restrict the listing to a single metric name (for example, Ir).
    #[argh(option)]
    metric: Option<String>,

    /// output format: text, json, or markdown (default: text).
    #[argh(option)]
    format: Option<String>,

    /// list the discriminant sets present in storage instead of the data set the
    /// analysis would include (does not require a repository).
    #[argh(switch)]
    discriminants: bool,

    /// emit detailed diagnostic notes to standard error (which storage prefix is
    /// listed, which objects are included or excluded and why, the resolved git
    /// topology).
    #[argh(switch)]
    verbose: bool,
}

impl ListCommand {
    fn into_options(self) -> ListOptions {
        ListOptions {
            config_path: self.config,
            repo: self.repo,
            branch: self.branch,
            base: self.base,
            no_dirty: self.no_dirty,
            since: self.since,
            engine: self.engine,
            target_triple: self.target_triple,
            os: self.os,
            architecture: self.architecture,
            machine_key: self.machine_key,
            metric: self.metric,
            format: self.format,
            discriminants: self.discriminants,
            verbose: self.verbose,
        }
    }
}

/// Remove the dirty runs a matching `analyze`/`list` would include.
///
/// The selection options mirror `analyze`/`list` (minus `--no-dirty`, meaningless
/// when only dirty runs are touched, and `--metric`, a series filter), so `clean`
/// removes exactly the dirty runs a `list` with the same flags would show on the
/// target side — plus the base-branch tip's dirty runs, which are removed
/// unconditionally. Pass `--dry-run` to preview without deleting.
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "clean")]
struct CleanCommand {
    /// path to the configuration file (defaults to `.cargo/bench_history.toml`).
    #[argh(option)]
    config: Option<PathBuf>,

    /// repository to resolve git topology from (defaults to the working directory).
    #[argh(option)]
    repo: Option<PathBuf>,

    /// target ref whose history is cleaned (defaults to HEAD).
    #[argh(option)]
    branch: Option<String>,

    /// base ref the target's history is split at (defaults to the default branch).
    #[argh(option)]
    base: Option<String>,

    /// only remove runs on or after this cutoff: an RFC 3339 timestamp
    /// (`2024-01-01T00:00:00Z`), a `YYYY-MM-DD` date, or a relative duration
    /// such as `6 months` or `30 days ago` (default: no lower bound).
    #[argh(option)]
    since: Option<String>,

    /// restrict the cleanup to a single engine, criterion or callgrind
    /// (default: every engine).
    #[argh(option)]
    engine: Option<String>,

    /// restrict the cleanup to a single full target triple (for example,
    /// `x86_64-unknown-linux-gnu`). Mutually exclusive with `--os` /
    /// `--architecture`, which select the same dimension by its derived parts.
    #[argh(option)]
    target_triple: Option<String>,

    /// restrict the cleanup to a single operating system (for example, windows).
    /// Cannot be combined with `--target-triple`.
    #[argh(option)]
    os: Option<String>,

    /// restrict the cleanup to a single CPU architecture (for example, `x86_64`).
    /// Cannot be combined with `--target-triple`.
    #[argh(option)]
    architecture: Option<String>,

    /// restrict the cleanup to a single machine partition.
    #[argh(option)]
    machine_key: Option<String>,

    /// preview what would be removed without deleting anything.
    #[argh(switch)]
    dry_run: bool,

    /// output format: text, json, or markdown (default: text).
    #[argh(option)]
    format: Option<String>,

    /// emit detailed diagnostic notes to standard error (which storage prefix is
    /// listed, which objects are selected or skipped and why, the resolved git
    /// topology, and each deletion).
    #[argh(switch)]
    verbose: bool,
}

impl CleanCommand {
    fn into_options(self) -> CleanOptions {
        CleanOptions {
            config_path: self.config,
            repo: self.repo,
            branch: self.branch,
            base: self.base,
            since: self.since,
            engine: self.engine,
            target_triple: self.target_triple,
            os: self.os,
            architecture: self.architecture,
            machine_key: self.machine_key,
            dry_run: self.dry_run,
            format: self.format,
            verbose: self.verbose,
        }
    }
}

/// Replay `run` across a range of historical commits.
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "backfill")]
struct BackfillCommand {
    /// path to the configuration file (defaults to `.cargo/bench_history.toml`).
    #[argh(option)]
    config: Option<PathBuf>,

    /// oldest commit of the range to backfill, inclusive; a SHA, tag, or ref
    /// such as `HEAD~20`.
    #[argh(option)]
    from: String,

    /// newest commit of the range to backfill, inclusive; a SHA, tag, or ref
    /// such as `HEAD` (must be on the current branch's first-parent history).
    #[argh(option)]
    to: String,

    /// benchmark the entire workspace; overrides --package (this is the default
    /// when no --package is given).
    #[argh(switch)]
    workspace: bool,

    /// benchmark only this package; repeatable, for example `-p nm -p many_cpus`
    /// (default: the whole workspace).
    #[argh(option, short = 'p', long = "package")]
    package: Vec<String>,

    /// benchmark only this bench target; repeatable (default: every bench target).
    #[argh(option)]
    bench: Vec<String>,

    /// override the recorded target triple used for partitioning.
    #[argh(option)]
    target_triple: Option<String>,

    /// override the machine fingerprint used to partition hardware-dependent
    /// results (for example, a CI machine-pool name).
    #[argh(option)]
    machine_key: Option<String>,

    /// replace already-stored results for the backfilled commits instead of
    /// skipping them as duplicates.
    #[argh(switch)]
    overwrite: bool,

    /// continue past commits whose build or benchmark fails instead of stopping.
    #[argh(switch)]
    ignore_errors: bool,

    /// emit detailed diagnostic notes to standard error for each commit's run
    /// (which directories are scanned, which files are included or skipped).
    #[argh(switch)]
    verbose: bool,

    /// arguments after `--` forwarded verbatim to `cargo bench` after the scope
    /// flags.
    #[argh(positional, greedy)]
    passthrough: Vec<String>,
}

impl BackfillCommand {
    fn into_options(self) -> BackfillOptions {
        let packages = if self.workspace {
            Vec::new()
        } else {
            self.package
        };
        BackfillOptions {
            config_path: self.config,
            from: self.from,
            to: self.to,
            packages,
            benches: self.bench,
            target_triple: self.target_triple,
            machine_key: self.machine_key,
            overwrite: self.overwrite,
            ignore_errors: self.ignore_errors,
            passthrough: strip_separator(self.passthrough),
            verbose: self.verbose,
        }
    }
}

/// Removes a single leading `--` separator from forwarded arguments, if present.
///
/// `argh` strips the separator that ends its own option parsing, but this guards
/// the case where one is captured by the greedy positional regardless.
fn strip_separator(mut passthrough: Vec<String>) -> Vec<String> {
    if passthrough.first().is_some_and(|arg| arg == "--") {
        passthrough.remove(0);
    }
    passthrough
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
    fn run_parses_timestamp_override() {
        let command = parse(&["run", "--timestamp", "2024-01-01T00:00:00Z"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
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
    fn analyze_collects_topology_and_facet_options() {
        let command = parse(&[
            "analyze",
            "--repo",
            "/work/folo",
            "--branch",
            "feature",
            "--base",
            "master",
            "--no-dirty",
            "--engine",
            "callgrind",
            "--os",
            "windows",
            "--architecture",
            "x86_64",
            "--machine-key",
            "ci-pool",
        ]);
        let Command::Analyze(options) = command else {
            panic!("expected analyze command");
        };
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
        assert_eq!(options.branch.as_deref(), Some("feature"));
        assert_eq!(options.base.as_deref(), Some("master"));
        assert!(options.no_dirty);
        assert_eq!(options.engine.as_deref(), Some("callgrind"));
        assert_eq!(options.os.as_deref(), Some("windows"));
        assert_eq!(options.architecture.as_deref(), Some("x86_64"));
        assert_eq!(options.machine_key.as_deref(), Some("ci-pool"));
    }

    #[test]
    fn analyze_parses_target_triple_facet() {
        let command = parse(&["analyze", "--target-triple", "x86_64-unknown-linux-gnu"]);
        let Command::Analyze(options) = command else {
            panic!("expected analyze command");
        };
        assert_eq!(
            options.target_triple.as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(options.os, None);
        assert_eq!(options.architecture, None);
    }

    #[test]
    fn list_collects_selection_and_discriminants() {
        let command = parse(&[
            "list",
            "--repo",
            "/work/folo",
            "--branch",
            "feature",
            "--base",
            "master",
            "--no-dirty",
            "--engine",
            "callgrind",
            "--os",
            "windows",
            "--architecture",
            "x86_64",
            "--machine-key",
            "ci-pool",
            "--metric",
            "Ir",
            "--format",
            "json",
            "--discriminants",
            "--verbose",
        ]);
        let Command::List(options) = command else {
            panic!("expected list command");
        };
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
        assert_eq!(options.branch.as_deref(), Some("feature"));
        assert_eq!(options.base.as_deref(), Some("master"));
        assert!(options.no_dirty);
        assert_eq!(options.engine.as_deref(), Some("callgrind"));
        assert_eq!(options.os.as_deref(), Some("windows"));
        assert_eq!(options.architecture.as_deref(), Some("x86_64"));
        assert_eq!(options.machine_key.as_deref(), Some("ci-pool"));
        assert_eq!(options.metric.as_deref(), Some("Ir"));
        assert_eq!(options.format.as_deref(), Some("json"));
        assert!(options.discriminants);
        assert!(options.verbose);
    }

    #[test]
    fn list_parses_target_triple_facet() {
        let command = parse(&["list", "--target-triple", "x86_64-unknown-linux-gnu"]);
        let Command::List(options) = command else {
            panic!("expected list command");
        };
        assert_eq!(
            options.target_triple.as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(options.os, None);
        assert_eq!(options.architecture, None);
    }

    #[test]
    fn clean_collects_selection_and_dry_run() {
        let command = parse(&[
            "clean",
            "--repo",
            "/work/folo",
            "--branch",
            "feature",
            "--base",
            "master",
            "--since",
            "2024-01-01T00:00:00Z",
            "--engine",
            "callgrind",
            "--os",
            "windows",
            "--architecture",
            "x86_64",
            "--machine-key",
            "ci-pool",
            "--dry-run",
            "--format",
            "json",
            "--verbose",
        ]);
        let Command::Clean(options) = command else {
            panic!("expected clean command");
        };
        assert_eq!(options.repo, Some(PathBuf::from("/work/folo")));
        assert_eq!(options.branch.as_deref(), Some("feature"));
        assert_eq!(options.base.as_deref(), Some("master"));
        assert_eq!(options.since.as_deref(), Some("2024-01-01T00:00:00Z"));
        assert_eq!(options.engine.as_deref(), Some("callgrind"));
        assert_eq!(options.os.as_deref(), Some("windows"));
        assert_eq!(options.architecture.as_deref(), Some("x86_64"));
        assert_eq!(options.machine_key.as_deref(), Some("ci-pool"));
        assert!(options.dry_run);
        assert_eq!(options.format.as_deref(), Some("json"));
        assert!(options.verbose);
    }

    #[test]
    fn clean_parses_target_triple_facet() {
        let command = parse(&["clean", "--target-triple", "x86_64-unknown-linux-gnu"]);
        let Command::Clean(options) = command else {
            panic!("expected clean command");
        };
        assert_eq!(
            options.target_triple.as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(options.os, None);
        assert_eq!(options.architecture, None);
        assert!(!options.dry_run);
    }

    #[test]
    fn backfill_collects_range_options_and_passthrough() {
        let command = parse(&[
            "backfill",
            "--from",
            "abc123",
            "--to",
            "def456",
            "--package",
            "nm",
            "--bench",
            "nm_observe",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
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
        assert_eq!(
            options.target_triple.as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(options.machine_key.as_deref(), Some("ci-pool"));
        assert!(options.overwrite);
        assert!(options.ignore_errors);
        assert_eq!(options.passthrough, vec!["--noplot".to_owned()]);
    }

    #[test]
    fn backfill_requires_from_and_to() {
        let parsed = Cli::from_args(&["cargo-bench-history"], &["backfill", "--from", "abc123"]);
        assert!(parsed.is_err(), "missing --to must be rejected");
    }

    #[test]
    fn backfill_parses_verbose_switch() {
        let Command::Backfill(options) = parse(&[
            "backfill",
            "--from",
            "abc123",
            "--to",
            "def456",
            "--verbose",
        ]) else {
            panic!("expected backfill command");
        };
        assert!(options.verbose);

        let Command::Backfill(options) = parse(&["backfill", "--from", "abc123", "--to", "def456"])
        else {
            panic!("expected backfill command");
        };
        assert!(!options.verbose);
    }

    #[test]
    fn strip_separator_removes_only_leading_marker() {
        assert_eq!(
            strip_separator(vec!["--".to_owned(), "a".to_owned()]),
            vec!["a".to_owned()]
        );
        assert_eq!(
            strip_separator(vec!["a".to_owned(), "--".to_owned()]),
            vec!["a".to_owned(), "--".to_owned()]
        );
    }
}
