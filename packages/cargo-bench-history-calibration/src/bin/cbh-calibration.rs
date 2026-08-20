//! Regenerates, checks, or reproduces the selection-adjustment calibration table embedded in
//! `cbh_stats`.
//!
//! Normally driven through `just bench-history-calibration-write` and its `-check` / `-verify`
//! companions rather than invoked directly.

use std::path::PathBuf;
use std::process::ExitCode;

use cargo_bench_history_calibration::{
    PRODUCTION_SAMPLES, TABLE_PATH, check, derive_table, write,
};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    about = "Generates the selection-adjustment calibration table embedded in cbh_stats"
)]
struct Args {
    #[command(subcommand)]
    command: Command,

    /// Workspace root to resolve paths against.
    #[arg(long, default_value = ".", global = true)]
    root: PathBuf,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Regenerates the table, overwriting the checked-in copy.
    Write,

    /// Fails if the checked-in copy differs from freshly generated content.
    Check,

    /// Re-derives the table from scratch and confirms it reproduces the checked-in copy.
    Verify,
}

fn main() -> ExitCode {
    let args = Args::parse();

    // Every subcommand needs the derived table: there is no cheap cached form to compare against,
    // so `check` and `verify` re-derive exactly as `write` does (§6.4).
    println!("Deriving calibration table at {PRODUCTION_SAMPLES} Monte Carlo samples per row...");
    let table = derive_table(PRODUCTION_SAMPLES);

    match args.command {
        Command::Write => {
            if let Err(error) = write(&args.root, &table) {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
            println!("Wrote {TABLE_PATH}.");
            ExitCode::SUCCESS
        }
        Command::Check | Command::Verify => match check(&args.root, &table) {
            Ok(None) => {
                println!("Calibration table is up to date.");
                ExitCode::SUCCESS
            }
            Ok(Some(report)) => {
                eprint!("{report}");
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}
