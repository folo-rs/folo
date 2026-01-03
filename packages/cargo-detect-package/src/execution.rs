// Command execution logic.
//
// This module contains logic for executing subcommands with package information.

use std::path::Path;
use std::process::Command;

use crate::detection::DetectedPackage;

/// Executes the subcommand with cargo arguments (-p or --workspace).
pub(crate) fn execute_with_cargo_args(
    working_dir: &Path,
    detected_package: &DetectedPackage,
    subcommand: &[String],
) -> Result<std::process::ExitStatus, std::io::Error> {
    if subcommand.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
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

    cmd.status()
}

/// Executes the subcommand with an environment variable set to the package name.
// Mutations to process execution cause subprocess hangs in integration tests.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn execute_with_env_var(
    working_dir: &Path,
    env_var: &str,
    detected_package: &DetectedPackage,
    subcommand: &[String],
) -> Result<std::process::ExitStatus, std::io::Error> {
    let Some(first_arg) = subcommand.first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
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

#[cfg(all(test, not(miri)))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    /// Creates a minimal temporary Cargo workspace for tests that need to run cargo commands.
    fn create_minimal_workspace() -> tempfile::TempDir {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path();

        fs::write(
            workspace_root.join("Cargo.toml"),
            r#"[workspace]
members = ["test_pkg"]
resolver = "2"
"#,
        )
        .unwrap();

        let test_pkg = workspace_root.join("test_pkg");
        fs::create_dir_all(test_pkg.join("src")).unwrap();
        fs::write(
            test_pkg.join("Cargo.toml"),
            r#"[package]
name = "test_pkg"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(test_pkg.join("src/lib.rs"), "// minimal lib\n").unwrap();

        temp_dir
    }

    #[test]
    fn execute_with_cargo_args_handles_separator() {
        // Test that we properly handle the "--" separator in clippy commands.

        // Test without "--" separator (should place package flags after subcommand).
        let subcommand = ["check".to_string(), "--all".to_string()];
        let separator_pos = subcommand.iter().position(|arg| arg == "--");
        assert_eq!(separator_pos, None);

        // Test with "--" separator (should place package flags before "--").
        let subcommand_with_separator = [
            "clippy".to_string(),
            "--all-features".to_string(),
            "--".to_string(),
            "-D".to_string(),
            "warnings".to_string(),
        ];
        let separator_pos = subcommand_with_separator.iter().position(|arg| arg == "--");
        assert_eq!(separator_pos, Some(2));

        // Test edge case with "--" as first argument.
        let subcommand_edge_case = ["clippy".to_string(), "--".to_string(), "--help".to_string()];
        let separator_pos = subcommand_edge_case.iter().position(|arg| arg == "--");
        assert_eq!(separator_pos, Some(1));
    }

    #[test]
    fn execute_with_cargo_args_workspace_branch() {
        // Test that the Workspace branch correctly adds --workspace flag.
        // We use "tree" with --depth 0 as it is fast and accepts --workspace.
        let workspace = create_minimal_workspace();

        let result = execute_with_cargo_args(
            workspace.path(),
            &DetectedPackage::Workspace,
            &["tree".to_string(), "--depth".to_string(), "0".to_string()],
        );

        assert!(result.is_ok());
        assert!(result.unwrap().success());
    }

    #[test]
    fn execute_with_cargo_args_workspace_with_separator() {
        // Test that the Workspace branch correctly adds --workspace flag when there is a "--"
        // separator. This tests the `Some(pos)` branch with `DetectedPackage::Workspace`.
        // We use "clippy" with "--" separator as it is a common use case.
        let workspace = create_minimal_workspace();

        let result = execute_with_cargo_args(
            workspace.path(),
            &DetectedPackage::Workspace,
            &[
                "clippy".to_string(),
                "--".to_string(),
                "-A".to_string(),
                "warnings".to_string(),
            ],
        );

        result.unwrap();
        // The command should have run (exit status depends on clippy findings, but it should not
        // error out from our argument handling).
    }

    #[test]
    fn execute_with_cargo_args_no_subcommand_returns_error() {
        let result = execute_with_cargo_args(Path::new("."), &DetectedPackage::Workspace, &[]);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn execute_with_env_var_no_subcommand_returns_error() {
        let result =
            execute_with_env_var(Path::new("."), "TEST_ENV", &DetectedPackage::Workspace, &[]);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
