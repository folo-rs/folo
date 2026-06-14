//! Argument parsing: the `argh` subcommand surface and its translation into the
//! typed [`Command`](crate::Command) model.

use std::path::PathBuf;

use argh::FromArgs;
use jiff::Timestamp;

use crate::{AnalyzeOptions, Command, InstallOptions, RunOptions};

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
        }
    }
}

#[derive(Debug, FromArgs)]
#[argh(subcommand)]
enum Subcommand {
    Run(RunCommand),
    Install(InstallCommand),
    Analyze(AnalyzeCommand),
}

/// Run the configured benchmark engines and store the results.
#[derive(Debug, FromArgs)]
#[argh(subcommand, name = "run")]
struct RunCommand {
    /// path to the configuration file (defaults to `.cargo/bench_history.toml`).
    #[argh(option)]
    config: Option<PathBuf>,

    /// run only the named engine (for example, callgrind).
    #[argh(option)]
    engine: Option<String>,

    /// override the effective timestamp (RFC 3339) when backfilling history.
    #[argh(option)]
    timestamp: Option<Timestamp>,

    /// override the recorded target triple used for partitioning.
    #[argh(option)]
    target_triple: Option<String>,

    /// harvest and build results without storing them.
    #[argh(switch)]
    no_store: bool,

    /// arguments after `--` forwarded verbatim to each engine command.
    #[argh(positional, greedy)]
    passthrough: Vec<String>,
}

impl RunCommand {
    fn into_options(self) -> RunOptions {
        RunOptions {
            config_path: self.config,
            engine: self.engine,
            timestamp: self.timestamp,
            target_triple: self.target_triple,
            no_store: self.no_store,
            passthrough: strip_separator(self.passthrough),
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

    /// only consider runs on or after this date.
    #[argh(option)]
    since: Option<String>,

    /// restrict analysis to a single engine system (criterion or callgrind).
    #[argh(option)]
    system: Option<String>,

    /// output format (text, json, or markdown).
    #[argh(option)]
    format: Option<String>,

    /// exit with failure if a regression is detected.
    #[argh(switch)]
    fail_on_regression: bool,
}

impl AnalyzeCommand {
    fn into_options(self) -> AnalyzeOptions {
        AnalyzeOptions {
            config_path: self.config,
            since: self.since,
            system: self.system,
            format: self.format,
            fail_on_regression: self.fail_on_regression,
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
    fn run_collects_options_and_passthrough() {
        let command = parse(&["run", "--engine", "callgrind", "--", "-p", "nm"]);
        let Command::Run(options) = command else {
            panic!("expected run command");
        };
        assert_eq!(options.engine.as_deref(), Some("callgrind"));
        assert_eq!(options.passthrough, vec!["-p".to_owned(), "nm".to_owned()]);
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
    fn install_maps_to_install_command() {
        let command = parse(&["install"]);
        assert_eq!(command, Command::Install(InstallOptions::default()));
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
