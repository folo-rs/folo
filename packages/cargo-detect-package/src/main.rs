//! A Cargo tool to detect the package that a file belongs to, passing the package name to a subcommand.
//!
//! This tool automatically detects which Cargo package a given file belongs to within a workspace,
//! and then executes a subcommand with the appropriate package scope. It supports two operating modes:
//! cargo integration mode (default) and environment variable mode.
//!
//! # Usage
//!
//! ```text
//! cargo-detect-package --path <PATH> [--via-env <ENV_VAR>] <SUBCOMMAND>...
//! ```
//!
//! ## Arguments
//!
//! - `--path <PATH>`: Path to the file for which to detect the package
//! - `--via-env <ENV_VAR>`: Optional. Pass the package name via environment variable instead of cargo arguments
//! - `<SUBCOMMAND>...`: The command to execute with the detected package information
//!
//! # Operating Modes
//!
//! ## Cargo Integration Mode (Default)
//!
//! In this mode, the tool automatically adds the appropriate cargo package arguments to the subcommand:
//! - If a specific package is detected: adds `-p <package_name>`
//! - If no package is detected (workspace scope): adds `--workspace`
//!
//! ### Examples
//!
//! ```bash
//! # Build the package containing src/lib.rs
//! cargo-detect-package --path packages/events/src/lib.rs build
//! # Executes: cargo build -p events
//!
//! # Test the package containing a specific test file
//! cargo-detect-package --path packages/many_cpus/tests/integration.rs test
//! # Executes: cargo test -p many_cpus
//!
//! # Check a file in the workspace root (falls back to workspace)
//! cargo-detect-package --path README.md check
//! # Executes: cargo check --workspace
//!
//! # Run clippy with additional arguments
//! cargo-detect-package --path packages/events/src/lib.rs clippy -- -D warnings
//! # Executes: cargo clippy -p events -- -D warnings
//! ```
//!
//! ## Environment Variable Mode
//!
//! In this mode, the tool sets an environment variable with the detected package name
//! and executes a non-cargo command. This is useful for integration with build tools
//! like `just`, `make`, or custom scripts.
//!
//! ### Examples
//!
//! ```bash
//! # Use with just command runner
//! cargo-detect-package --path packages/events/src/lib.rs --via-env package just build
//! # Executes: just build (with package=events environment variable)
//!
//! # Use with custom script
//! cargo-detect-package --path packages/many_cpus/src/lib.rs --via-env PKG_NAME ./build.sh
//! # Executes: ./build.sh (with PKG_NAME=many_cpus environment variable)
//!
//! # Workspace scope with environment variable
//! cargo-detect-package --path nonexistent.rs --via-env package just test
//! # Executes: just test (no environment variable set, allowing just to handle workspace scope)
//! ```
//!
//! # Package Detection Logic
//!
//! The tool uses the following logic to detect packages:
//!
//! 1. **Path Resolution**: Attempts to canonicalize the input path
//!    - If the path cannot be resolved (file doesn't exist), falls back to workspace scope
//!
//! 2. **Workspace Root Discovery**: Walks up the directory tree to find the workspace root
//!    - Looks for a `Cargo.toml` file containing a `[workspace]` section
//!    - If no workspace root is found, falls back to workspace scope
//!
//! 3. **Package Detection**: Starting from the file's directory, walks up toward the workspace root
//!    - Looks for the nearest `Cargo.toml` file that is not the workspace root
//!    - Extracts the package name from the `[package]` section
//!    - If no package-level `Cargo.toml` is found, uses workspace scope
//!
//! # Fallback Behavior
//!
//! The tool gracefully handles various edge cases by falling back to workspace scope:
//! - Non-existent files or directories
//! - Files outside the workspace
//! - Files in the workspace root that don't belong to any specific package
//! - Invalid or missing workspace configuration
//!
//! This ensures that the tool never fails due to ambiguous package detection and always
//! provides a reasonable default behavior.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use argh::FromArgs;

/// A Cargo tool to detect the package that a file belongs to, passing the package name to a subcommand.
#[derive(FromArgs)]
struct Args {
    /// path to the file to detect package for
    #[argh(option)]
    path: PathBuf,

    /// pass the detected package as an environment variable instead of as a cargo argument
    #[argh(option)]
    via_env: Option<String>,

    /// the subcommand to execute
    #[argh(positional, greedy)]
    subcommand: Vec<String>,
}

fn main() -> ExitCode {
    let args: Args = argh::from_env();

    let detected_package = match detect_package(&args.path) {
        Ok(package) => package,
        Err(e) => {
            eprintln!("Error detecting package: {e}");
            return ExitCode::FAILURE;
        }
    };

    let exit_status = match args.via_env {
        Some(env_var) => execute_with_env_var(&env_var, &detected_package, &args.subcommand),
        None => execute_with_cargo_args(&detected_package, &args.subcommand),
    };

    match exit_status {
        Ok(status) => {
            if status.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("Error executing command: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Represents the result of package detection.
#[derive(Clone, Debug, Eq, PartialEq)]
enum DetectedPackage {
    /// A specific package was detected.
    Package(String),
    /// No specific package was detected, use the entire workspace.
    Workspace,
}

/// Detects which Cargo package the given file belongs to.
fn detect_package(file_path: &Path) -> Result<DetectedPackage, Box<dyn std::error::Error>> {
    // Try to canonicalize the path, but if it fails (e.g., file doesn't exist),
    // fall back to workspace mode
    let Ok(absolute_path) = file_path.canonicalize() else {
        // Path doesn't exist or can't be resolved, use workspace mode
        return Ok(DetectedPackage::Workspace);
    };

    let Ok(workspace_root) = find_workspace_root(&absolute_path) else {
        // Can't find workspace root, use workspace mode
        return Ok(DetectedPackage::Workspace);
    };

    // Start from the file's directory and walk up to find the nearest Cargo.toml
    let mut current_dir = if absolute_path.is_file() {
        absolute_path.parent().unwrap()
    } else {
        &absolute_path
    };

    while current_dir.starts_with(&workspace_root) {
        let cargo_toml = current_dir.join("Cargo.toml");
        if cargo_toml.exists() && current_dir != workspace_root {
            // Found a package-level Cargo.toml, extract the package name
            return extract_package_name(&cargo_toml);
        }

        match current_dir.parent() {
            Some(parent) => current_dir = parent,
            None => break,
        }
    }

    // No package found, use workspace
    Ok(DetectedPackage::Workspace)
}

/// Finds the workspace root by looking for the workspace-level Cargo.toml.
fn find_workspace_root(start_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut current_dir = start_path;

    loop {
        let cargo_toml = current_dir.join("Cargo.toml");
        if cargo_toml.exists() {
            // Check if this is a workspace root
            let contents = std::fs::read_to_string(&cargo_toml)?;
            if contents.contains("[workspace]") {
                return Ok(current_dir.to_path_buf());
            }
        }

        match current_dir.parent() {
            Some(parent) => current_dir = parent,
            None => break,
        }
    }

    Err("Could not find workspace root".into())
}

/// Extracts the package name from a Cargo.toml file.
fn extract_package_name(
    cargo_toml_path: &Path,
) -> Result<DetectedPackage, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(cargo_toml_path)?;

    // Simple TOML parsing to extract package name
    for line in contents.lines() {
        let line = line.trim();
        if let Some(name) = parse_name_line(line, "name = \"", '"') {
            return Ok(DetectedPackage::Package(name));
        } else if let Some(name) = parse_name_line(line, "name = '", '\'') {
            return Ok(DetectedPackage::Package(name));
        }
    }

    Err(format!(
        "Could not find package name in {}",
        cargo_toml_path.display()
    )
    .into())
}

/// Helper function to parse a name line with given prefix and suffix.
fn parse_name_line(line: &str, prefix: &str, suffix: char) -> Option<String> {
    if line.starts_with(prefix) && line.ends_with(suffix) {
        let start = prefix.len();
        let end = line.len().checked_sub(1)?;
        line.get(start..end).map(ToString::to_string)
    } else {
        None
    }
}

/// Executes the subcommand with cargo arguments (-p or --workspace).
fn execute_with_cargo_args(
    detected_package: &DetectedPackage,
    subcommand: &[String],
) -> Result<std::process::ExitStatus, std::io::Error> {
    let Some(first_arg) = subcommand.first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "No subcommand provided",
        ));
    };

    let mut cmd = Command::new("cargo");
    cmd.arg(first_arg);

    match detected_package {
        DetectedPackage::Package(package_name) => {
            cmd.arg("-p").arg(package_name);
        }
        DetectedPackage::Workspace => {
            cmd.arg("--workspace");
        }
    }

    if let Some(remaining_args) = subcommand.get(1..) {
        cmd.args(remaining_args);
    }

    cmd.status()
}

/// Executes the subcommand with an environment variable set to the package name.
fn execute_with_env_var(
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

    if let Some(remaining_args) = subcommand.get(1..) {
        cmd.args(remaining_args);
    }

    match detected_package {
        DetectedPackage::Package(package_name) => {
            cmd.env(env_var, package_name);
        }
        DetectedPackage::Workspace => {
            // For workspace, we don't set the environment variable
            // This allows the target command to handle the "no package specified" case
        }
    }

    cmd.status()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn extract_package_name_double_quotes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        fs::write(
            &cargo_toml,
            r#"
[package]
name = "test-package"
version = "0.1.0"
"#,
        )
        .unwrap();

        let result = extract_package_name(&cargo_toml).unwrap();
        assert_eq!(result, DetectedPackage::Package("test-package".to_string()));
    }

    #[test]
    fn extract_package_name_single_quotes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        fs::write(
            &cargo_toml,
            r#"
[package]
name = 'test-package-single'
version = "0.1.0"
"#,
        )
        .unwrap();

        let result = extract_package_name(&cargo_toml).unwrap();
        assert_eq!(
            result,
            DetectedPackage::Package("test-package-single".to_string())
        );
    }

    #[test]
    fn extract_package_name_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        fs::write(
            &cargo_toml,
            r#"
[package]
version = "0.1.0"
"#,
        )
        .unwrap();

        extract_package_name(&cargo_toml)
            .expect_err("Expected an error when package name is missing");
    }

    #[test]
    fn detected_package_equality() {
        assert_eq!(
            DetectedPackage::Package("test".to_string()),
            DetectedPackage::Package("test".to_string())
        );
        assert_eq!(DetectedPackage::Workspace, DetectedPackage::Workspace);
        assert_ne!(
            DetectedPackage::Package("test".to_string()),
            DetectedPackage::Workspace
        );
    }

    #[test]
    fn parse_name_line_double_quotes() {
        let result = parse_name_line(r#"name = "test-package""#, "name = \"", '"');
        assert_eq!(result, Some("test-package".to_string()));
    }

    #[test]
    fn parse_name_line_single_quotes() {
        let result = parse_name_line("name = 'test-package'", "name = '", '\'');
        assert_eq!(result, Some("test-package".to_string()));
    }

    #[test]
    fn parse_name_line_no_match() {
        let result = parse_name_line("version = \"0.1.0\"", "name = \"", '"');
        assert_eq!(result, None);
    }

    #[test]
    fn parse_name_line_incomplete() {
        let result = parse_name_line("name = \"test", "name = \"", '"');
        assert_eq!(result, None);
    }

    #[test]
    fn detect_package_nonexistent_file() {
        let result = detect_package(Path::new("nonexistent/file.rs")).unwrap();
        assert_eq!(result, DetectedPackage::Workspace);
    }
}
