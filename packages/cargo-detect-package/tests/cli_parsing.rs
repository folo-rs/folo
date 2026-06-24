//! Tests for the `cargo-detect-package` clap argument parser.
//!
//! These exercise the library-housed [`Cli`] directly (no subprocess), covering the
//! required `--path`, the optional `--via-env`/`--outside-package` flags, the
//! case-insensitive outside-package values, the greedy trailing subcommand capture
//! (including hyphenated arguments), and the help/error early-exit classification.

use std::path::PathBuf;

use cargo_detect_package::{Cli, EarlyExit, OutsidePackageAction};

fn parse(args: &[&str]) -> Result<Cli, EarlyExit> {
    Cli::from_args(&["cargo-detect-package"], args)
}

#[test]
fn parses_all_options_and_trailing_subcommand() {
    let input = parse(&[
        "--path",
        "src/lib.rs",
        "--via-env",
        "DETECTED_PACKAGE",
        "--outside-package",
        "error",
        "clippy",
        "--fix",
    ])
    .unwrap()
    .into_input();

    assert_eq!(input.path, PathBuf::from("src/lib.rs"));
    assert_eq!(input.via_env.as_deref(), Some("DETECTED_PACKAGE"));
    assert_eq!(input.outside_package, OutsidePackageAction::Error);
    assert_eq!(
        input.subcommand,
        vec!["clippy".to_string(), "--fix".to_string()]
    );
}

#[test]
fn outside_package_defaults_to_workspace_when_omitted() {
    let input = parse(&["--path", "src/lib.rs"]).unwrap().into_input();
    assert_eq!(input.outside_package, OutsidePackageAction::Workspace);
    assert_eq!(input.via_env, None);
    assert!(input.subcommand.is_empty());
}

#[test]
fn outside_package_parsing_is_case_insensitive() {
    let input = parse(&["--path", "x", "--outside-package", "IGNORE"])
        .unwrap()
        .into_input();
    assert_eq!(input.outside_package, OutsidePackageAction::Ignore);
}

#[test]
fn trailing_subcommand_captures_double_dash_and_hyphenated_args() {
    let input = parse(&[
        "--path",
        "x",
        "clippy",
        "--all-features",
        "--",
        "-A",
        "warnings",
    ])
    .unwrap()
    .into_input();
    assert_eq!(
        input.subcommand,
        vec![
            "clippy".to_string(),
            "--all-features".to_string(),
            "--".to_string(),
            "-A".to_string(),
            "warnings".to_string(),
        ]
    );
}

#[test]
fn missing_required_path_is_a_failure_early_exit() {
    let early = parse(&[]).unwrap_err();
    assert!(
        early.status.is_err(),
        "missing required arg is a failure exit"
    );
}

#[test]
fn invalid_outside_package_value_is_a_failure_early_exit() {
    let early = parse(&["--path", "x", "--outside-package", "nope"]).unwrap_err();
    assert!(early.status.is_err());
    assert!(
        early.output.contains("workspace"),
        "error should name the valid options, got: {}",
        early.output
    );
}

#[test]
fn help_request_is_a_success_early_exit() {
    let early = parse(&["--help"]).unwrap_err();
    assert!(early.status.is_ok(), "help should be a success exit");
    assert!(early.output.contains("Usage"));
}
