// Command execution logic.
//
// This module contains logic for executing subcommands with package information.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

use crate::detection::DetectedPackage;

/// Executes the subcommand with cargo arguments (-p or --workspace).
pub(crate) fn execute_with_cargo_args(
    working_dir: &Path,
    detected_package: &DetectedPackage,
    subcommand: &[String],
) -> Result<ExitStatus, io::Error> {
    cargo_command(working_dir, detected_package, subcommand)?.status()
}

/// Builds the Cargo invocation independently of spawning it.
fn cargo_command(
    working_dir: &Path,
    detected_package: &DetectedPackage,
    subcommand: &[String],
) -> io::Result<Command> {
    if subcommand.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No subcommand provided",
        ));
    }

    let mut cmd = Command::new("cargo");
    cmd.current_dir(working_dir);

    // Find the position of "--" separator if it exists.
    let separator_pos = subcommand.iter().position(|arg| arg == "--");

    match separator_pos {
        Some(pos) => {
            // Add subcommand arguments before "--".
            if let Some(before_sep) = subcommand.get(..pos) {
                cmd.args(before_sep);
            }

            // Add package selection arguments before "--".
            match detected_package {
                DetectedPackage::Package(package_name) => {
                    cmd.arg("-p").arg(package_name);
                }
                DetectedPackage::Workspace => {
                    cmd.arg("--workspace");
                }
            }

            // Add "--" and arguments after it.
            if let Some(after_sep) = subcommand.get(pos..) {
                cmd.args(after_sep);
            }
        }
        None => {
            // No "--" separator, add subcommand first then package flags.
            cmd.args(subcommand);

            // Add package selection arguments after the subcommand.
            match detected_package {
                DetectedPackage::Package(package_name) => {
                    cmd.arg("-p").arg(package_name);
                }
                DetectedPackage::Workspace => {
                    cmd.arg("--workspace");
                }
            }
        }
    }

    Ok(cmd)
}

/// Executes the subcommand with an environment variable set to the package name.
// Mutations to process execution cause subprocess hangs in integration tests.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn execute_with_env_var(
    working_dir: &Path,
    env_var: &str,
    detected_package: &DetectedPackage,
    subcommand: &[String],
) -> Result<ExitStatus, io::Error> {
    let Some(first_arg) = subcommand.first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No subcommand provided",
        ));
    };

    let mut cmd = Command::new(first_arg);
    cmd.current_dir(working_dir);

    if let Some(remaining_args) = subcommand.get(1..) {
        cmd.args(remaining_args);
    }

    match detected_package {
        DetectedPackage::Package(package_name) => {
            cmd.env(env_var, package_name);
        }
        DetectedPackage::Workspace => {
            // For workspace, we do not set the environment variable.
            // This allows the target command to handle the "no package specified" case.
        }
    }

    cmd.status()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn assert_cargo_args(selection: &DetectedPackage, input: &[&str], expected: &[&str]) {
        let working_dir = Path::new("workspace");
        let input = input.iter().map(ToString::to_string).collect::<Vec<_>>();
        let command = cargo_command(working_dir, selection, &input).unwrap();
        assert_eq!(command.get_program(), OsStr::new("cargo"));
        assert_eq!(command.get_current_dir(), Some(working_dir));
        assert!(command.get_args().eq(expected.iter().map(OsStr::new)));
    }

    #[test]
    fn cargo_args_select_package_with_and_without_separator() {
        let package = DetectedPackage::Package("test-package".to_owned());
        assert_cargo_args(
            &package,
            &["check", "--all-targets"],
            &["check", "--all-targets", "-p", "test-package"],
        );
        assert_cargo_args(
            &package,
            &["clippy", "--all-features", "--", "-D", "warnings"],
            &[
                "clippy",
                "--all-features",
                "-p",
                "test-package",
                "--",
                "-D",
                "warnings",
            ],
        );
        assert_cargo_args(
            &package,
            &["clippy", "--", "--help"],
            &["clippy", "-p", "test-package", "--", "--help"],
        );
    }

    #[test]
    fn cargo_args_select_workspace_without_separator() {
        assert_cargo_args(
            &DetectedPackage::Workspace,
            &["tree", "--depth", "0"],
            &["tree", "--depth", "0", "--workspace"],
        );
    }

    #[test]
    fn cargo_args_select_workspace_before_separator() {
        assert_cargo_args(
            &DetectedPackage::Workspace,
            &["clippy", "--", "-A", "warnings"],
            &["clippy", "--workspace", "--", "-A", "warnings"],
        );
    }

    #[test]
    fn execute_with_cargo_args_no_subcommand_returns_error() {
        let result = execute_with_cargo_args(Path::new("."), &DetectedPackage::Workspace, &[]);
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn execute_with_env_var_no_subcommand_returns_error() {
        let result =
            execute_with_env_var(Path::new("."), "TEST_ENV", &DetectedPackage::Workspace, &[]);
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
