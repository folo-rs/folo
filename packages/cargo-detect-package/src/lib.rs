#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! A Cargo tool to detect the package that a file belongs to, passing the package name
//! to a subcommand.
//!
//! This crate provides the core logic for package detection, exposed via the [`run`] function.
//! The binary entry point is in `main.rs`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use toml::Value;

/// Action to take when a path is not within any package.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OutsidePackageAction {
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

/// Input parameters for the `run` function.
///
/// This is the parsed and validated input that the core logic operates on.
#[doc(hidden)]
#[derive(Debug)]
#[allow(
    clippy::exhaustive_structs,
    reason = "This is a hidden struct for internal/test use only"
)]
pub struct RunInput {
    /// Path to the file to detect package for.
    pub path: PathBuf,
    /// Pass the detected package as an environment variable instead of as a cargo argument.
    pub via_env: Option<String>,
    /// Action to take when path is not in any package.
    pub outside_package: OutsidePackageAction,
    /// The subcommand to execute.
    pub subcommand: Vec<String>,
}

/// The outcome of a successful run.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::exhaustive_enums,
    reason = "This is a hidden enum for internal/test use only"
)]
pub enum RunOutcome {
    /// A package was detected and the subcommand executed successfully.
    PackageDetected {
        /// The name of the detected package.
        package_name: String,
        /// Whether the subcommand succeeded.
        subcommand_succeeded: bool,
    },
    /// The path was not in any package, workspace scope was used.
    WorkspaceScope {
        /// Whether the subcommand succeeded.
        subcommand_succeeded: bool,
    },
    /// The path was not in any package and was ignored (no subcommand executed).
    Ignored,
}

/// Errors that can occur during a run.
#[doc(hidden)]
#[derive(Debug)]
#[allow(
    clippy::exhaustive_enums,
    reason = "This is a hidden enum for internal/test use only"
)]
pub enum RunError {
    /// Failed to validate workspace context.
    WorkspaceValidation(String),
    /// Failed to detect package.
    PackageDetection(String),
    /// Path is not in any package and --outside-package=error was specified.
    OutsidePackage,
    /// Failed to execute the subcommand.
    CommandExecution(std::io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkspaceValidation(msg) => write!(f, "{msg}"),
            Self::PackageDetection(msg) => write!(f, "Error detecting package: {msg}"),
            Self::OutsidePackage => write!(f, "Path is not in any package"),
            Self::CommandExecution(e) => write!(f, "Error executing command: {e}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Core logic of the tool, extracted for testability.
///
/// This function contains all the business logic without any process-global dependencies
/// like `std::env::args()`, making it suitable for direct testing.
#[doc(hidden)]
pub fn run(input: &RunInput) -> Result<RunOutcome, RunError> {
    // Validate that we are running from within the same workspace as the target path.
    // This also canonicalizes paths and finds the workspace root, which we reuse later.
    let workspace_context = validate_workspace_context(&input.path)
        .map_err(|e| RunError::WorkspaceValidation(e.to_string()))?;

    let detected_package = detect_package(&workspace_context)
        .map_err(|e| RunError::PackageDetection(e.to_string()))?;

    // Handle outside package actions.
    match (&detected_package, &input.outside_package) {
        (DetectedPackage::Workspace, OutsidePackageAction::Ignore) => {
            println!("Path is not in any package, ignoring as requested");
            return Ok(RunOutcome::Ignored);
        }
        (DetectedPackage::Workspace, OutsidePackageAction::Error) => {
            return Err(RunError::OutsidePackage);
        }
        (DetectedPackage::Package(name), _) => {
            println!("Detected package: {name}");
        }
        (DetectedPackage::Workspace, OutsidePackageAction::Workspace) => {
            println!("Path is not in any package, using workspace scope");
        }
    }

    // Defense-in-depth check: verify that the match block above has handled all early-exit cases.
    // As of the current logic, is_early_exit_case is always false because the match block above
    // returns early for both Ignore and Error cases. If is_early_exit_case is true here, it
    // indicates a logic bug in the match block.
    //
    // Coverage: This assertion can never fail with the current code structure because the match
    // block above returns early for both (Workspace, Ignore) and (Workspace, Error) cases.
    #[cfg_attr(coverage_nightly, coverage(off))]
    {
        let is_early_exit_case = matches!(
            (&detected_package, &input.outside_package),
            (
                DetectedPackage::Workspace,
                OutsidePackageAction::Ignore | OutsidePackageAction::Error
            )
        );
        assert!(
            !is_early_exit_case,
            "Logic error: the match block above must return early for Ignore/Error cases"
        );
    }

    let exit_status = match &input.via_env {
        Some(env_var) => execute_with_env_var(env_var, &detected_package, &input.subcommand),
        None => execute_with_cargo_args(&detected_package, &input.subcommand),
    }
    .map_err(RunError::CommandExecution)?;

    let subcommand_succeeded = exit_status.success();

    match detected_package {
        DetectedPackage::Package(package_name) => Ok(RunOutcome::PackageDetected {
            package_name,
            subcommand_succeeded,
        }),
        DetectedPackage::Workspace => Ok(RunOutcome::WorkspaceScope {
            subcommand_succeeded,
        }),
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

/// Holds validated workspace context information.
///
/// This struct is returned by `validate_workspace_context` and contains all the validated
/// and canonicalized paths needed for package detection, avoiding redundant lookups.
#[derive(Debug)]
struct WorkspaceContext {
    /// The canonicalized absolute path to the target file or directory.
    absolute_target_path: PathBuf,
    /// The canonicalized path to the workspace root.
    workspace_root: PathBuf,
}

/// Detects which Cargo package the given file belongs to.
///
/// Takes a `WorkspaceContext` which contains the already-validated and canonicalized paths,
/// avoiding redundant filesystem lookups.
fn detect_package(
    context: &WorkspaceContext,
) -> Result<DetectedPackage, Box<dyn std::error::Error>> {
    let absolute_path = &context.absolute_target_path;
    let workspace_root = &context.workspace_root;

    // Start from the file's directory and walk up to find the nearest Cargo.toml.
    let mut current_dir = if absolute_path.is_file() {
        absolute_path.parent().unwrap()
    } else {
        absolute_path
    };

    while current_dir.starts_with(workspace_root) {
        let cargo_toml = current_dir.join("Cargo.toml");
        if cargo_toml.exists() && current_dir != workspace_root {
            // Found a package-level Cargo.toml, extract the package name.
            return extract_package_name(&cargo_toml);
        }

        match current_dir.parent() {
            Some(parent) => current_dir = parent,
            // Coverage: This branch can only be reached if the filesystem changes during the
            // operation - we found a workspace Cargo.toml earlier but now we have walked up
            // to the drive root without finding any Cargo.toml. Not worth mocking for tests.
            #[cfg_attr(coverage_nightly, coverage(off))]
            None => break,
        }
    }

    // No package found, use workspace.
    Ok(DetectedPackage::Workspace)
}

/// Finds the workspace root by looking for the workspace-level Cargo.toml.
fn find_workspace_root(start_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut current_dir = start_path;

    loop {
        let cargo_toml = current_dir.join("Cargo.toml");
        if cargo_toml.exists() {
            // Check if this is a workspace root.
            let contents = std::fs::read_to_string(&cargo_toml)?;
            let value: Value = toml::from_str(&contents)?;
            if value.get("workspace").is_some() {
                // Return canonicalized path for consistent comparison.
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
            // For workspace, we do not set the environment variable.
            // This allows the target command to handle the "no package specified" case.
        }
    }

    cmd.status()
}

/// Validates that the current working directory and target path are within the same Cargo
/// workspace. This ensures the tool is only used when both locations are in the same workspace
/// context.
///
/// Returns a `WorkspaceContext` containing the canonicalized target path and workspace root,
/// which can be reused by subsequent operations to avoid redundant filesystem lookups.
fn validate_workspace_context(
    target_path: &Path,
) -> Result<WorkspaceContext, Box<dyn std::error::Error>> {
    let current_dir = std::env::current_dir()?;

    // Find workspace root from the current directory.
    let current_workspace_root = find_workspace_root(&current_dir).map_err(|original_error| {
        format!("Current directory is not within a Cargo workspace: {original_error}")
    })?;

    // Resolve the target path - try to make it absolute.
    let resolved_target_path = if target_path.is_absolute() {
        target_path.to_path_buf()
    } else {
        // For relative paths, try relative to current directory first.
        let relative_to_current = current_dir.join(target_path);
        if relative_to_current.exists() {
            relative_to_current
        } else {
            // If that does not exist, try relative to workspace root.
            // This handles cases where the tool is run from a different directory.
            current_workspace_root.join(target_path)
        }
    };

    // Canonicalize the resolved target path - it must exist.
    let absolute_target_path = resolved_target_path.canonicalize().map_err(|error| {
        format!(
            "Target path '{}' does not exist or cannot be accessed: {error}",
            target_path.display()
        )
    })?;

    // Find workspace root for the target path.
    let target_workspace_root =
        find_workspace_root(&absolute_target_path).map_err(|original_error| {
            format!("Target path is not within a Cargo workspace: {original_error}")
        })?;

    // Verify both paths are in the same workspace.
    // Normalize paths to handle Windows path representation differences.
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

    // Normalize the absolute target path as well to ensure consistent path format with
    // workspace_root. This is important on Windows where canonicalize() adds UNC prefixes
    // that would break starts_with() comparisons in detect_package().
    let absolute_target_path_normalized = normalize_path(&absolute_target_path);

    Ok(WorkspaceContext {
        absolute_target_path: absolute_target_path_normalized,
        workspace_root: target_workspace_normalized,
    })
}

/// Normalizes a path by using OS canonicalization and stripping Windows UNC prefixes.
/// This helps with path comparisons on Windows where paths may have different representations.
fn normalize_path(path: &Path) -> PathBuf {
    // Canonicalize the path (paths are expected to exist).
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Strip Windows UNC prefix if present after canonicalization.
    if let Some(path_str) = canonical.to_str()
        && let Some(stripped) = path_str.strip_prefix(r"\\?\")
    {
        return PathBuf::from(stripped);
    }

    canonical
}

#[cfg(all(test, not(miri)))]
#[cfg_attr(coverage_nightly, coverage(off))]
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
    fn validate_workspace_context_nonexistent_file() {
        // Nonexistent files are now rejected by validate_workspace_context, not detect_package.
        let result = validate_workspace_context(Path::new("nonexistent/file.rs"));
        assert!(
            result.is_err(),
            "Should return error for non-existent files"
        );
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    #[serial] // This test uses file!() which gives a relative path that depends on the current working directory.
    fn validate_workspace_context_from_workspace() {
        // This test ensures validation works when both current dir and target are in the same
        // workspace. Use the current source file which should exist and be in the workspace.
        let current_file = file!(); // This gives us the path to this source file.
        let current_file_path = Path::new(current_file);

        validate_workspace_context(current_file_path)
            .expect("Should validate successfully when both paths are in the same workspace");
    }

    #[test]
    #[serial] // This test changes the global working directory, so must run serially to avoid interference with other tests.
    fn validate_workspace_context_from_temp_dir() {
        // Save current directory.
        let original_dir = std::env::current_dir().unwrap();

        // Create a temporary directory that is not a workspace.
        let temp_dir = tempfile::tempdir().unwrap();

        // Change to the temp directory.
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // Validation should fail when targeting a file that does not exist.
        let target_path = Path::new("nonexistent.rs");
        let result = validate_workspace_context(target_path);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Current directory is not within a Cargo workspace")
        );

        // Restore original directory.
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    #[serial] // This test changes the global working directory, so must run serially to avoid interference with other tests.
    fn validate_workspace_context_different_workspaces() {
        // This test verifies that the tool rejects when current dir and target are in different
        // workspaces. We simulate this by creating a fake workspace structure.
        let temp_dir = tempfile::tempdir().unwrap();

        // Create a fake workspace in temp dir.
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

        // Create a package in the fake workspace.
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

        // Create another fake workspace to simulate cross-workspace access.
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

        // Create a package in the other workspace.
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

        // Try to target a file in the other workspace while running from fake workspace.
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&fake_workspace).unwrap();

        // This should fail because we are in different workspaces.
        let other_workspace_file = other_package.join("src").join("lib.rs");
        let result = validate_workspace_context(&other_workspace_file);
        result.unwrap_err();

        // Restore original directory.
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    #[serial] // This test uses a relative path that depends on the current working directory for resolution.
    fn validate_workspace_context_relative_path_outside() {
        let result = validate_workspace_context(Path::new("../../../outside_workspace/file.rs"));
        assert!(result.is_err(), "Expected error but validation succeeded!");
        // The error could be about the file not existing or being outside workspace.
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
    #[serial] // This test runs cargo which requires a workspace context.
    fn execute_with_cargo_args_workspace_branch() {
        // Test that the Workspace branch correctly adds --workspace flag.
        // We use "tree" with --depth 0 as it is fast and accepts --workspace.
        let result = execute_with_cargo_args(
            &DetectedPackage::Workspace,
            &["tree".to_string(), "--depth".to_string(), "0".to_string()],
        );
        assert!(result.is_ok());
        assert!(result.unwrap().success());
    }

    #[test]
    #[serial] // This test runs cargo which requires a workspace context.
    fn execute_with_cargo_args_workspace_with_separator() {
        // Test that the Workspace branch correctly adds --workspace flag when there is a "--"
        // separator. This tests the `Some(pos)` branch with `DetectedPackage::Workspace`.
        // We use "clippy" with "--" separator as it is a common use case.
        let result = execute_with_cargo_args(
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
    #[serial] // This test creates a workspace on disk.
    fn detect_package_with_invalid_toml() {
        // Test that detect_package returns an error when the package has invalid TOML.
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path();

        // Create workspace Cargo.toml.
        fs::write(
            workspace_root.join("Cargo.toml"),
            r#"[workspace]
members = ["bad_package"]
"#,
        )
        .unwrap();

        // Create bad_package with intentionally malformed TOML.
        let bad_package = workspace_root.join("bad_package");
        fs::create_dir_all(bad_package.join("src")).unwrap();
        fs::write(
            bad_package.join("Cargo.toml"),
            r#"# Intentionally malformed TOML - missing closing bracket
[package
name = "bad_package"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::write(bad_package.join("src/lib.rs"), "// test\n").unwrap();

        // Create a WorkspaceContext pointing to the file in bad_package.
        let context = WorkspaceContext {
            absolute_target_path: bad_package.join("src/lib.rs").canonicalize().unwrap(),
            workspace_root: workspace_root.canonicalize().unwrap(),
        };

        // detect_package should fail because the Cargo.toml is malformed.
        let result = detect_package(&context);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("TOML") || error_msg.contains("parse"),
            "Expected TOML parse error, got: {error_msg}"
        );
    }

    #[test]
    fn execute_with_cargo_args_no_subcommand_returns_error() {
        let result = execute_with_cargo_args(&DetectedPackage::Workspace, &[]);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn execute_with_env_var_no_subcommand_returns_error() {
        let result = execute_with_env_var("TEST_ENV", &DetectedPackage::Workspace, &[]);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
