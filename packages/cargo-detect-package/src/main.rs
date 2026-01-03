#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the cargo-detect-package tool.
//!
//! This module is excluded from mutation testing because testing process entry/exit behavior
//! is impractical - it requires spawning subprocesses and checking exit codes.

use std::path::PathBuf;
use std::process::ExitCode;

use argh::FromArgs;
use cargo_detect_package::{OutsidePackageAction, RunInput, RunOutcome, run};

/// A Cargo tool to detect the package that a file belongs to, passing the package name to a
/// subcommand.
#[derive(FromArgs)]
struct Args {
    /// path to the file to detect package for
    #[argh(option)]
    path: PathBuf,

    /// pass the detected package as an environment variable instead of as a cargo argument
    #[argh(option)]
    via_env: Option<String>,

    /// action to take when path is not in any package (workspace, ignore, error)
    #[argh(option)]
    outside_package: Option<OutsidePackageAction>,

    /// the subcommand to execute
    #[argh(positional, greedy)]
    subcommand: Vec<String>,
}

// Binary entry point - mutations would require subprocess testing which is impractical.
#[cfg_attr(test, mutants::skip)]
fn main() -> ExitCode {
    // When called via `cargo detect-package`, the first argument will be "detect-package"
    // which we need to skip. We handle this by manually parsing the args.
    let mut env_args: Vec<String> = std::env::args().collect();

    // If the first argument after the program name is "detect-package", remove it.
    if env_args.get(1).is_some_and(|arg| arg == "detect-package") {
        env_args.remove(1);
    }

    // Convert to &str for argh.
    let str_args: Vec<&str> = env_args.iter().map(String::as_str).collect();

    let program_name = str_args
        .first()
        .expect("std::env::args() always provides at least the program name");

    let args: Args = match Args::from_args(&[program_name], str_args.get(1..).unwrap_or(&[])) {
        Ok(args) => args,
        Err(early_exit) => {
            println!("{}", early_exit.output);
            return if early_exit.output.contains("help") {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };

    let input = RunInput {
        path: args.path,
        via_env: args.via_env,
        outside_package: args
            .outside_package
            .unwrap_or(OutsidePackageAction::Workspace),
        subcommand: args.subcommand,
    };

    match run(&input) {
        Ok(outcome) => match outcome {
            RunOutcome::PackageDetected {
                subcommand_succeeded,
                ..
            }
            | RunOutcome::WorkspaceScope {
                subcommand_succeeded,
            } => {
                if subcommand_succeeded {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            RunOutcome::Ignored => ExitCode::SUCCESS,
        },
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
