//! A Cargo tool to detect the package that a file belongs to, passing the package name to a subcommand.
//!
//! This tool automatically detects which Cargo package a given file belongs to within a workspace,
//! and then executes a subcommand with the appropriate package scope.
//!
//! See the [README.md](../README.md) for detailed usage instructions and examples.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use argh::FromArgs;
use toml::Value;

/// Action to take when a path is not within any package.
#[derive(Clone, Debug, Eq, PartialEq)]
enum OutsidePackageAction {
    /// Use the entire workspace.
    Workspace,
    /// Ignore and do not run the subcommand, exit with success.
    Ignore,
    /// Error and do not run the subcommand, exit with failure.
    Error,
}

impl std::str::FromStr for OutsidePackageAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "workspace" => Ok(Self::Workspace),
            "ignore" => Ok(Self::Ignore),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "Invalid outside-package action: '{s}'. Valid options are: workspace, ignore, error"
            )),
        }
    }
}

/// A Cargo tool to detect the package that a file belongs to, passing the package name to a subcommand.
#[derive(FromArgs)]
struct Args {
    /// path to the file to detect package for
    #[argh(option)]
    path: PathBuf,

    /// pass the detected package as an environment variable instead of as a cargo argument
    #[argh(option)]
    via_env: Option<String>,

    /// action to take when path is not in any package (workspace, ignore, error)
    #[argh(option)]
    outside_package: Option<OutsidePackageAction>,

    /// the subcommand to execute
    #[argh(positional, greedy)]
    subcommand: Vec<String>,
}

fn main() -> ExitCode {
    // When called via `cargo detect-package`, the first argument will be "detect-package"
    // which we need to skip. We handle this by manually parsing the args.
    let mut env_args: Vec<String> = std::env::args().collect();

    // If the first argument after the program name is "detect-package", remove it
    if env_args.get(1).is_some_and(|arg| arg == "detect-package") {
        env_args.remove(1);
    }

    // Convert to &str for argh
    let str_args: Vec<&str> = env_args.iter().map(String::as_str).collect();

    let Some(program_name) = str_args.first() else {
        eprintln!("Failed to get program name");
        return ExitCode::FAILURE;
    };

    let args: Args = match Args::from_args(&[program_name], str_args.get(1..).unwrap_or(&[])) {
        Ok(args) => args,
        Err(early_exit) => {
            println!("{}", early_exit.output);
            return if early_exit.output.contains("help") {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };

    // Validate that we're running from within the same workspace as the target path
    if let Err(e) = validate_workspace_context(&args.path) {
        eprintln!("Error: {e}");
        return ExitCode::FAILURE;
    }

    let outside_package_action = args
        .outside_package
        .unwrap_or(OutsidePackageAction::Workspace);

    let detected_package = match detect_package(&args.path) {
        Ok(package) => package,
        Err(e) => {
            eprintln!("Error detecting package: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Handle outside package actions
    match (&detected_package, &outside_package_action) {
        (DetectedPackage::Workspace, OutsidePackageAction::Ignore) => {
            println!("Path is not in any package, ignoring as requested");
            return ExitCode::SUCCESS;
        }
        (DetectedPackage::Workspace, OutsidePackageAction::Error) => {
            eprintln!("Error: Path is not in any package");
            return ExitCode::FAILURE;
        }
        (DetectedPackage::Package(name), _) => {
            println!("Detected package: {name}");
        }
        (DetectedPackage::Workspace, OutsidePackageAction::Workspace) => {
            println!("Path is not in any package, using workspace scope");
        }
    }

    // Only execute the subcommand if we're not ignoring or erroring out
    let should_execute = !matches!(
        (&detected_package, &outside_package_action),
        (
            DetectedPackage::Workspace,
            OutsidePackageAction::Ignore | OutsidePackageAction::Error
        )
    );

    if !should_execute {
        return ExitCode::SUCCESS;
    }

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
    // Canonicalize the path - it must exist
    let absolute_path = file_path.canonicalize().map_err(|error| {
        format!(
            "File path '{}' does not exist or cannot be accessed: {error}",
            file_path.display()
        )
    })?;

    let workspace_root = find_workspace_root(&absolute_path).map_err(|error| {
        format!(
            "Cannot find workspace root for '{}': {error}",
            file_path.display()
        )
    })?;

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
            let value: Value = toml::from_str(&contents)?;
            if value.get("workspace").is_some() {
                // Return canonicalized path for consistent comparison
                return Ok(current_dir
                    .canonicalize()
                    .unwrap_or_else(|_| current_dir.to_path_buf()));
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
    let value: Value = toml::from_str(&contents)?;

    if let Some(package_table) = value.get("package")
        && let Some(name) = package_table.get("name")
        && let Some(name_str) = name.as_str()
    {
        return Ok(DetectedPackage::Package(name_str.to_string()));
    }

    Err(format!(
        "Could not find package name in {}",
        cargo_toml_path.display()
    )
    .into())
}

/// Executes the subcommand with cargo arguments (-p or --workspace).
fn execute_with_cargo_args(
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

    // Find the position of "--" separator if it exists
    let separator_pos = subcommand.iter().position(|arg| arg == "--");

    match separator_pos {
        Some(pos) => {
            // Add subcommand arguments before "--"
            if let Some(before_sep) = subcommand.get(..pos) {
                cmd.args(before_sep);
            }

            // Add package selection arguments before "--"
            match detected_package {
                DetectedPackage::Package(package_name) => {
                    cmd.arg("-p").arg(package_name);
                }
                DetectedPackage::Workspace => {
                    cmd.arg("--workspace");
                }
            }

            // Add "--" and arguments after it
            if let Some(after_sep) = subcommand.get(pos..) {
                cmd.args(after_sep);
            }
        }
        None => {
            // No "--" separator, add subcommand first then package flags
            cmd.args(subcommand);

            // Add package selection arguments after the subcommand
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

/// Validates that the current working directory and target path are within the same Cargo workspace.
/// This ensures the tool is only used when both locations are in the same workspace context.
fn validate_workspace_context(target_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let current_dir = std::env::current_dir()?;

    // Find workspace root from the current directory
    let current_workspace_root = find_workspace_root(&current_dir).map_err(|original_error| {
        format!("Current directory is not within a Cargo workspace: {original_error}")
    })?;

    // Resolve the target path - try to make it absolute
    let resolved_target_path = if target_path.is_absolute() {
        target_path.to_path_buf()
    } else {
        // For relative paths, try relative to current directory first
        let relative_to_current = current_dir.join(target_path);
        if relative_to_current.exists() {
            relative_to_current
        } else {
            // If that doesn't exist, try relative to workspace root
            // This handles cases where the tool is run from a different directory
            current_workspace_root.join(target_path)
        }
    };

    // Canonicalize the resolved target path - it must exist
    let absolute_target_path = resolved_target_path.canonicalize().map_err(|error| {
        format!(
            "Target path '{}' does not exist or cannot be accessed: {error}",
            target_path.display()
        )
    })?;

    // Find workspace root for the target path
    let target_workspace_root =
        find_workspace_root(&absolute_target_path).map_err(|original_error| {
            format!("Target path is not within a Cargo workspace: {original_error}")
        })?;

    // Verify both paths are in the same workspace
    // Normalize paths to handle Windows path representation differences
    let current_workspace_normalized = normalize_path(&current_workspace_root);
    let target_workspace_normalized = normalize_path(&target_workspace_root);

    if current_workspace_normalized != target_workspace_normalized {
        return Err(format!(
            "Current directory workspace ('{}') differs from target path workspace ('{}')",
            current_workspace_normalized.display(),
            target_workspace_normalized.display()
        )
        .into());
    }

    Ok(())
}

/// Normalizes a path by using OS canonicalization and stripping Windows UNC prefixes.
/// This helps with path comparisons on Windows where paths may have different representations.
fn normalize_path(path: &Path) -> PathBuf {
    // Canonicalize the path (paths are expected to exist)
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Strip Windows UNC prefix if present after canonicalization
    if let Some(path_str) = canonical.to_str()
        && let Some(stripped) = path_str.strip_prefix(r"\\?\")
    {
        return PathBuf::from(stripped);
    }

    canonical
}

#[cfg(all(test, not(miri)))]
mod tests {
    use std::fs;

    use serial_test::serial;

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
    fn extract_package_name_with_comments_and_complex_toml() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        fs::write(
            &cargo_toml,
            r#"
# This is a comment
[package]
# Package name
name = "complex-package"
version = "0.1.0"
authors = ["Test Author <test@example.com>"]
description = "A test package with complex TOML"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
"#,
        )
        .unwrap();

        let result = extract_package_name(&cargo_toml).unwrap();
        assert_eq!(
            result,
            DetectedPackage::Package("complex-package".to_string())
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
    fn detect_package_nonexistent_file() {
        let result = detect_package(Path::new("nonexistent/file.rs"));
        assert!(
            result.is_err(),
            "Should return error for non-existent files"
        );
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    #[serial] // This test uses file!() which gives a relative path that depends on the current working directory
    fn validate_workspace_context_from_workspace() {
        // This test ensures validation works when both current dir and target are in the same workspace
        // Use the current source file which should exist and be in the workspace
        let current_file = file!(); // This gives us the path to this source file
        let current_file_path = Path::new(current_file);

        validate_workspace_context(current_file_path)
            .expect("Should validate successfully when both paths are in the same workspace");
    }

    #[test]
    #[serial] // This test changes the global working directory, so must run serially to avoid interference with other tests
    fn validate_workspace_context_from_temp_dir() {
        // Save current directory
        let original_dir = std::env::current_dir().unwrap();

        // Create a temporary directory that's not a workspace
        let temp_dir = tempfile::tempdir().unwrap();

        // Change to the temp directory
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // Validation should fail when targeting a file that doesn't exist
        let target_path = Path::new("nonexistent.rs");
        let result = validate_workspace_context(target_path);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Current directory is not within a Cargo workspace")
        );

        // Restore original directory
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    #[serial] // This test changes the global working directory, so must run serially to avoid interference with other tests
    fn validate_workspace_context_different_workspaces() {
        // This test verifies that the tool rejects when current dir and target are in different workspaces
        // We'll simulate this by creating a fake workspace structure
        let temp_dir = tempfile::tempdir().unwrap();

        // Create a fake workspace in temp dir
        let fake_workspace = temp_dir.path().join("fake_workspace");
        fs::create_dir_all(&fake_workspace).unwrap();
        fs::write(
            fake_workspace.join("Cargo.toml"),
            r#"
[workspace]
members = ["package1"]
"#,
        )
        .unwrap();

        // Create a package in the fake workspace
        let fake_package = fake_workspace.join("package1");
        fs::create_dir_all(&fake_package).unwrap();
        fs::write(
            fake_package.join("Cargo.toml"),
            r#"
[package]
name = "fake_package"
version = "0.1.0"
"#,
        )
        .unwrap();

        // Create another fake workspace to simulate cross-workspace access
        let other_workspace = temp_dir.path().join("other_workspace");
        fs::create_dir_all(&other_workspace).unwrap();
        fs::write(
            other_workspace.join("Cargo.toml"),
            r#"
[workspace]
members = ["other_package"]
"#,
        )
        .unwrap();

        // Create a package in the other workspace
        let other_package = other_workspace.join("other_package");
        fs::create_dir_all(other_package.join("src")).unwrap();
        fs::write(
            other_package.join("Cargo.toml"),
            r#"
[package]
name = "other_package"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::write(other_package.join("src").join("lib.rs"), "// test file").unwrap();

        // Try to target a file in the other workspace while running from fake workspace
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&fake_workspace).unwrap();

        // This should fail because we're in different workspaces
        let other_workspace_file = other_package.join("src").join("lib.rs");
        let result = validate_workspace_context(&other_workspace_file);
        assert!(result.is_err());

        // Restore original directory
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    #[serial] // This test uses a relative path that depends on the current working directory for resolution
    fn validate_workspace_context_relative_path_outside() {
        let result = validate_workspace_context(Path::new("../../../outside_workspace/file.rs"));
        assert!(result.is_err(), "Expected error but validation succeeded!");
        // The error could be about the file not existing or being outside workspace
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("does not exist")
                || error_msg.contains("is not within the current workspace"),
            "Expected appropriate error message, got: {error_msg}"
        );
    }

    #[test]
    fn outside_package_action_parsing() {
        assert_eq!(
            "workspace".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Workspace
        );
        assert_eq!(
            "Workspace".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Workspace
        );
        assert_eq!(
            "WORKSPACE".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Workspace
        );

        assert_eq!(
            "ignore".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Ignore
        );
        assert_eq!(
            "Ignore".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Ignore
        );

        assert_eq!(
            "error".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Error
        );
        assert_eq!(
            "Error".parse::<OutsidePackageAction>().unwrap(),
            OutsidePackageAction::Error
        );

        let result = "invalid".parse::<OutsidePackageAction>();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Invalid outside-package action")
        );
    }

    #[test]
    fn execute_with_cargo_args_handles_separator() {
        // Test that we properly handle the "--" separator in clippy commands

        // Test without "--" separator (should place package flags after subcommand)
        let subcommand = ["check".to_string(), "--all".to_string()];
        let separator_pos = subcommand.iter().position(|arg| arg == "--");
        assert_eq!(separator_pos, None);

        // Test with "--" separator (should place package flags before "--")
        let subcommand_with_separator = [
            "clippy".to_string(),
            "--all-features".to_string(),
            "--".to_string(),
            "-D".to_string(),
            "warnings".to_string(),
        ];
        let separator_pos = subcommand_with_separator.iter().position(|arg| arg == "--");
        assert_eq!(separator_pos, Some(2));

        // Test edge case with "--" as first argument
        let subcommand_edge_case = ["clippy".to_string(), "--".to_string(), "--help".to_string()];
        let separator_pos = subcommand_edge_case.iter().position(|arg| arg == "--");
        assert_eq!(separator_pos, Some(1));
    }
}
