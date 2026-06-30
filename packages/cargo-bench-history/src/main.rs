#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the cargo-bench-history tool.
//!
//! This module is excluded from mutation testing because exercising process
//! entry/exit behavior requires spawning subprocesses and checking exit codes.

use std::process::ExitCode;

use cargo_bench_history::{Cli, run};

// The analyze pipeline fans the per-object gzip-decompress + JSON parse out across
// worker threads. serde_json makes many small allocations, and the system
// allocator's cross-thread contention (acute on the Windows process heap) otherwise
// serializes those threads and erases the parallel win, so the binary installs a
// scalable allocator process-wide. Miri cannot call mimalloc's FFI, so under Miri the
// default allocator stands in (Miri runs single-threaded, so the contention is moot).
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg_attr(test, mutants::skip)]
#[tokio::main]
async fn main() -> ExitCode {
    // When called via `cargo bench-history`, the first argument is "bench-history",
    // which we strip before parsing the rest.
    let mut env_args: Vec<String> = std::env::args().collect();

    if env_args.get(1).is_some_and(|arg| arg == "bench-history") {
        env_args.remove(1);
    }

    let str_args: Vec<&str> = env_args.iter().map(String::as_str).collect();

    let program_name = str_args
        .first()
        .expect("std::env::args() always provides at least the program name");

    let sub_args = str_args.get(1..).unwrap_or(&[]);

    // With no subcommand, show the full help (each command's description) on
    // stderr and exit non-zero, rather than failing with a bare usage error.
    if sub_args.is_empty() {
        eprintln!("{}", Cli::help(program_name));
        return ExitCode::FAILURE;
    }

    let cli: Cli = match Cli::from_args(&[program_name], sub_args) {
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

    let outcome = match run(&cli.into_command()).await {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::FAILURE;
        }
    };

    // `--no-text` suppresses the text report, so `stdout_text` is `None`; emit
    // nothing in that case rather than a blank line (the requested files carry
    // the result).
    if let Some(text) = outcome.stdout_text() {
        println!("{text}");
    }

    if outcome.is_success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
