#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for the cargo-detect-package tool.
//!
//! This module is excluded from mutation testing because testing process entry/exit behavior
//! is impractical - it requires spawning subprocesses and checking exit codes. CLI parsing
//! lives in the library's `cli` module and the core logic lives in `cargo_detect_package::run`,
//! both tested directly.

use std::process::ExitCode;

use cargo_detect_package::{Cli, RunOutcome, run};

// mimalloc is a scalable general-purpose allocator: faster small allocations and
// no cross-thread allocator-lock contention (acute on the Windows process heap),
// a broad low-risk win applied uniformly across the workspace's binaries. Miri
// cannot call mimalloc's FFI, so under Miri the default allocator stands in.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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

    // Convert to &str for the parser.
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
