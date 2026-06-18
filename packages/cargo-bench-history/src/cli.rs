//! Argument parsing: the `argh` subcommand surface and its translation into the
//! typed [`Command`](crate::Command) model.

use std::path::PathBuf;

use argh::FromArgs;
use jiff::Timestamp;

use crate::{AnalyzeOptions, BackfillOptions, Command, InstallOptions, RunOptions};

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
}

impl InstallCommand {
    fn into_options(self) -> InstallOptions {
        InstallOptions {
            config_path: self.config,
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

    /// only consider runs on or after this date, in RFC 3339 format, for
    /// example `2024-01-01T00:00:00Z` (default: no lower bound).
    #[argh(option)]
    since: Option<String>,

    /// restrict analysis to a single engine, criterion or callgrind
    /// (default: every engine).
    #[argh(option)]
    engine: Option<String>,

    /// restrict analysis to a single operating system (for example, windows).
    #[argh(option)]
    os: Option<String>,

    /// restrict analysis to a single CPU architecture (for example, `x86_64`).
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

    /// list the discriminant sets present in storage instead of analyzing.
    #[argh(switch)]
    list_discriminants: bool,

    /// exit with failure if a regression is detected.
    #[argh(switch)]
    fail_on_regression: bool,
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
            os: self.os,
            architecture: self.architecture,
            machine_key: self.machine_key,
            metric: self.metric,
            format: self.format,
            list_discriminants: self.list_discriminants,
            fail_on_regression: self.fail_on_regression,
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
    fn analyze_collects_switches() {
        let command = parse(&["analyze", "--fail-on-regression"]);
        let Command::Analyze(options) = command else {
            panic!("expected analyze command");
        };
        assert!(options.fail_on_regression);
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
            "--list-discriminants",
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
        assert!(options.list_discriminants);
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
