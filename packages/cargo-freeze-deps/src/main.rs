#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the cargo-freeze-deps tool.
//!
//! This module is excluded from mutation testing and coverage because testing process
//! entry/exit behavior is impractical — it requires spawning subprocesses and checking
//! exit codes. CLI parsing lives in the library's `cli` module and the bulk of the logic
//! lives in `cargo_freeze_deps::run`, both tested directly.

use std::process::ExitCode;

use cargo_freeze_deps::{Cli, run};

// Binary entry point — mutations would require subprocess testing which is impractical.
#[cfg_attr(test, mutants::skip)]
fn main() -> ExitCode {
    // When invoked as `cargo freeze-deps`, Cargo passes "freeze-deps" as the first
    // argument after the program name. We strip it so clap sees a normal CLI invocation.
    let mut env_args: Vec<String> = std::env::args().collect();

    if env_args.get(1).is_some_and(|arg| arg == "freeze-deps") {
        env_args.remove(1);
    }

    let str_args: Vec<&str> = env_args.iter().map(String::as_str).collect();

    let program_name = str_args
        .first()
        .expect("std::env::args() always provides at least the program name");

    let cli = match Cli::from_args(&[program_name], str_args.get(1..).unwrap_or(&[])) {
        Ok(cli) => cli,
        Err(early_exit) => {
            // `status` is `Ok` for a `--help`/usage request (print to stdout, exit
            // success) and `Err` for a parse error (print to stderr, exit failure).
            return match early_exit.status {
                Ok(()) => {
                    println!("{}", early_exit.output);
                    ExitCode::SUCCESS
                }
                Err(()) => {
                    eprintln!("{}", early_exit.output);
                    ExitCode::FAILURE
                }
            };
        }
    };

    match run(&cli.into_input()) {
        Ok(outcome) => {
            let noun = if outcome.frozen_count == 1 {
                "dependency version"
            } else {
                "dependency versions"
            };
            println!(
                "Froze {} {noun}; left {} unchanged.",
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
