//! A stand-in benchmark engine used by the integration tests.
//!
//! It imitates the only parts of a real engine that `cargo bench-history run`
//! observes: it writes Gungraun `summary.json` files into the cargo target tree
//! (so the harvester finds fresh output) and exits with a caller-chosen code (so
//! exit-code handling can be exercised). It performs no real benchmarking.
//!
//! Usage: `cargo-bench-history-mock-engine [--exit-code N] [--summary GROUP=KIND]...`
//! where `KIND` is `single` (an unparametrized summary, `Ir` = 36) or
//! `parametrized` (id `two_instants`, `Ir` = 87). Summaries are written to
//! `<target-root>/gungraun/GROUP/summary.json`, where `<target-root>` honors
//! `CARGO_TARGET_DIR` exactly as the harvester does.

use std::path::PathBuf;
use std::process::ExitCode;

/// A committed Gungraun summary with no `id` (unparametrized), `Ir` = 36.
const SINGLE_SUMMARY: &str =
    include_str!("../fixtures/callgrind/single_unparametrized.summary.json");
/// A committed Gungraun summary with `id` = `two_instants`, `Ir` = 87.
const PARAMETRIZED_SUMMARY: &str = include_str!("../fixtures/callgrind/parametrized.summary.json");

#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode {
    let mut exit_code: u8 = 0;
    let mut summaries: Vec<(String, &'static str)> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-code" => {
                let value = args.next().expect("--exit-code requires a value");
                exit_code = value.parse().expect("--exit-code must be a u8");
            }
            "--summary" => {
                let value = args.next().expect("--summary requires GROUP=KIND");
                let (group, kind) = value
                    .split_once('=')
                    .expect("--summary value must be GROUP=KIND");
                let content = match kind {
                    "single" => SINGLE_SUMMARY,
                    "parametrized" => PARAMETRIZED_SUMMARY,
                    other => panic!("unknown summary kind {other:?}"),
                };
                summaries.push((group.to_owned(), content));
            }
            other => panic!("unknown argument {other:?}"),
        }
    }

    let target_root =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from);

    for (group, content) in &summaries {
        let dir = target_root.join("gungraun").join(group);
        std::fs::create_dir_all(&dir).expect("summary directory should be creatable");
        std::fs::write(dir.join("summary.json"), content).expect("summary should be writable");
    }

    ExitCode::from(exit_code)
}
