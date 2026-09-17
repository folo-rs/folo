//! Native executable help and argument-error boundaries for reporting commands.
//! Help requests stop before lifecycle dispatch; these tests never access GitHub or write outputs.

use std::process::Command;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-bench-history-github"));
    for name in ["GITHUB_TOKEN", "GH_TOKEN", "GITHUB_REPOSITORY"] {
        command.env_remove(name);
    }
    command
}

#[test]
#[cfg_attr(miri, ignore = "Native child-process help and exit-code coverage.")]
fn every_subcommand_is_present_in_help() {
    let output = command().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for name in [
        "publish-issue-findings",
        "publish-issue-clean",
        "publish-issue-no-data",
        "publish-issue-preflight",
        "publish-issue-failed",
        "alert",
        "publish-comment-findings",
        "publish-comment-clean",
        "publish-comment-no-data",
        "publish-comment-preflight",
        "publish-comment-failed",
        "collection-receipt",
        "prepare-analysis",
        "inspect-report",
        "workflow-matrix",
    ] {
        assert!(help.contains(name));
    }
}

fn assert_removed_global_option(option: &str, value: &str) {
    // Clap reports invalid arguments separately from a lifecycle failure; accepting this
    // option would instead reach the successful offline help path.
    const ARGUMENT_ERROR: i32 = 2;

    let output = command()
        .args([option, value, "workflow-matrix", "--help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(ARGUMENT_ERROR));
}

#[test]
#[cfg_attr(miri, ignore = "Native child-process argument-error coverage.")]
fn issue_title_adoption_is_rejected() {
    assert_removed_global_option("--legacy-issue-title", "Unowned issue");
}

#[test]
#[cfg_attr(miri, ignore = "Native child-process argument-error coverage.")]
fn custom_comment_identity_is_rejected() {
    assert_removed_global_option("--comment-marker", "<!-- custom -->");
}
