#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the cargo-bench-history tool.
//!
//! This module is excluded from mutation testing because exercising process
//! entry/exit behavior requires spawning subprocesses and checking exit codes.

use std::process::ExitCode;

use argh::FromArgs;
use cargo_bench_history::{Cli, RunOutcome, run};

#[cfg_attr(test, mutants::skip)]
#[tokio::main]
async fn main() -> ExitCode {
    // When called via `cargo bench-history`, the first argument is "bench-history",
    // which we strip before handing the rest to argh.
    let mut env_args: Vec<String> = std::env::args().collect();

    if env_args.get(1).is_some_and(|arg| arg == "bench-history") {
        env_args.remove(1);
    }

    let str_args: Vec<&str> = env_args.iter().map(String::as_str).collect();

    let program_name = str_args
        .first()
        .expect("std::env::args() always provides at least the program name");

    let cli: Cli = match Cli::from_args(&[program_name], str_args.get(1..).unwrap_or(&[])) {
        Ok(cli) => cli,
        Err(early_exit) => {
            println!("{}", early_exit.output);
            return if early_exit.output.contains("help") {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };

    match run(&cli.into_command()).await {
        Ok(RunOutcome::Completed { message }) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Ok(RunOutcome::NotImplemented { command }) => {
            println!("`{command}` is recognized but not yet implemented");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}
