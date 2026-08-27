#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Binary entry point for `dure`.

use std::process::ExitCode;

use dure::{Cli, Outcome, run};

// Install mimalloc as a scalable, general-purpose allocator process-wide: faster
// small allocations and no cross-thread allocator-lock contention (acute on the
// Windows process heap), a broad low-risk win applied uniformly across the
// workspace's binaries. Miri cannot call mimalloc's FFI, so under Miri the
// default allocator stands in.
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg_attr(test, mutants::skip)]
fn main() -> ExitCode {
    let env_args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let str_args: Vec<&str> = env_args.iter().map(String::as_str).collect();
    let program_name = str_args.first().map_or("dure", |name| *name);

    let cli = match Cli::from_args(&[program_name], str_args.get(1..).unwrap_or(&[])) {
        Ok(cli) => cli,
        Err(early_exit) => {
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
        Ok(Outcome::Success) => ExitCode::SUCCESS,
        Ok(Outcome::AppExit(status)) => {
            if status == 0 {
                ExitCode::SUCCESS
            } else if let Ok(code) = u8::try_from(status) {
                ExitCode::from(code)
            } else {
                // Windows process statuses are wider than `ExitCode`'s portable
                // `u8`. Forward via `exit` so the original value is preserved.
                std::process::exit(status);
            }
        }
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}
