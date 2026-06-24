//! Tests for the `cargo-freeze-deps` clap argument parser.
//!
//! These exercise the library-housed [`Cli`] directly (no subprocess), covering the
//! input-path default, the `--path`/`--output` options and their short forms, and the
//! help/error early-exit classification consumed by the binary entry point.

use std::path::PathBuf;

use cargo_freeze_deps::{Cli, EarlyExit};

fn parse(args: &[&str]) -> Result<Cli, EarlyExit> {
    Cli::from_args(&["cargo-freeze-deps"], args)
}

#[test]
fn defaults_to_cargo_toml_when_no_path_given() {
    let input = parse(&[]).unwrap().into_input();
    assert_eq!(input.path, PathBuf::from("Cargo.toml"));
    assert_eq!(input.output, None);
}

#[test]
fn long_options_are_parsed() {
    let input = parse(&["--path", "a/Cargo.toml", "--output", "b/Cargo.toml"])
        .unwrap()
        .into_input();
    assert_eq!(input.path, PathBuf::from("a/Cargo.toml"));
    assert_eq!(input.output, Some(PathBuf::from("b/Cargo.toml")));
}

#[test]
fn short_options_are_parsed() {
    let input = parse(&["-p", "a/Cargo.toml", "-o", "b/Cargo.toml"])
        .unwrap()
        .into_input();
    assert_eq!(input.path, PathBuf::from("a/Cargo.toml"));
    assert_eq!(input.output, Some(PathBuf::from("b/Cargo.toml")));
}

#[test]
fn output_is_optional() {
    let input = parse(&["--path", "a/Cargo.toml"]).unwrap().into_input();
    assert_eq!(input.path, PathBuf::from("a/Cargo.toml"));
    assert_eq!(input.output, None);
}

#[test]
fn help_request_is_a_success_early_exit() {
    let early = parse(&["--help"]).unwrap_err();
    assert!(early.status.is_ok(), "help should be a success exit");
    assert!(early.output.contains("Usage"));
}

#[test]
fn unknown_flag_is_a_failure_early_exit() {
    let early = parse(&["--definitely-not-a-flag"]).unwrap_err();
    assert!(
        early.status.is_err(),
        "a parse error should be a failure exit"
    );
}
