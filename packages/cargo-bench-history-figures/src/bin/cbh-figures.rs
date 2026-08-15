//! Regenerates, verifies, or previews the assets embedded in the `cargo-bench-history`
//! book's data pipeline appendix.
//!
//! Normally driven through `just book-figures` and `just book-figures-check` rather than
//! invoked directly.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{fs, io};

use cargo_bench_history_figures::assets::GENERATED_ROOT;
use cargo_bench_history_figures::{assets, preview};
use clap::{Parser, Subcommand};

/// Where the preview page is written.
///
/// Under the build directory because it is a development aid, not a checked-in artifact.
const PREVIEW_PATH: &str = "target/appendix-figures.html";

#[derive(Debug, Parser)]
#[command(
    about = "Generates the figures and tables embedded in the cargo-bench-history book appendix"
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
    /// Regenerates every asset, overwriting the checked-in copies.
    Write,

    /// Verifies the checked-in copies match freshly generated content.
    Check,

    /// Writes a page showing every asset on mdBook's default Light and Navy
    /// background/text-color pairs.
    Preview,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let result = match args.command {
        Command::Write => write(&args.root),
        Command::Check => return check(&args.root),
        Command::Preview => write_preview(&args.root),
    };

    if let Err(error) = result {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Regenerates every asset.
fn write(root: &Path) -> io::Result<()> {
    let target = root.join(GENERATED_ROOT);
    let count = assets::write(&target)?;
    println!("Wrote {count} generated assets into {}", target.display());
    Ok(())
}

/// Verifies the checked-in copies are current.
fn check(root: &Path) -> ExitCode {
    let target = root.join(GENERATED_ROOT);
    match assets::check(&target) {
        Ok(None) => {
            println!("Generated appendix assets are up to date.");
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
    }
}

/// Writes the Light and Navy preview page.
fn write_preview(root: &Path) -> io::Result<()> {
    let target = root.join(PREVIEW_PATH);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| wrap_io(error, "create directory", parent))?;
    }
    fs::write(&target, preview::page(&assets::assets()))
        .map_err(|error| wrap_io(error, "write", &target))?;
    println!("Wrote preview to {}", target.display());
    Ok(())
}

/// Attaches the attempted operation and path to a filesystem failure.
fn wrap_io(error: io::Error, operation: &str, path: &Path) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("failed to {operation} {}: {error}", path.display()),
    )
}
