#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the cargo-freeze-deps tool.
//!
//! This module is excluded from mutation testing and coverage because testing process
//! entry/exit behavior is impractical — it requires spawning subprocesses and checking
//! exit codes. The bulk of the logic lives in the library and is tested directly via
//! `cargo_freeze_deps::run`.

use std::path::PathBuf;
use std::process::ExitCode;

use argh::FromArgs;
use cargo_freeze_deps::{RunInput, run};

/// A Cargo subcommand that freezes all floating dependency versions in a Cargo.toml file
/// to their literal values.
#[derive(FromArgs)]
struct Args {
    /// path to the Cargo.toml file to freeze. Defaults to ./Cargo.toml in the current
    /// working directory.
    #[argh(option, short = 'p')]
    path: Option<PathBuf>,

    /// path to write the rewritten Cargo.toml to. Defaults to the input path
    /// (rewriting in place).
    #[argh(option, short = 'o')]
    output: Option<PathBuf>,
}

// Binary entry point — mutations would require subprocess testing which is impractical.
#[cfg_attr(test, mutants::skip)]
fn main() -> ExitCode {
    // When invoked as `cargo freeze-deps`, Cargo passes "freeze-deps" as the first
    // argument after the program name. We strip it so argh sees a normal CLI invocation.
    let mut env_args: Vec<String> = std::env::args().collect();

    if env_args.get(1).is_some_and(|arg| arg == "freeze-deps") {
        env_args.remove(1);
    }

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
        path: args.path.unwrap_or_else(|| PathBuf::from("Cargo.toml")),
        output: args.output,
    };

    match run(&input) {
        Ok(outcome) => {
            println!(
                "Froze {} dependency version(s); left {} unchanged.",
                outcome.frozen_count, outcome.skipped_count
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
