//! A Cargo tool to detect the package that a file belongs to, passing the package name to a subcommand.
//!
//! This tool automatically detects which Cargo package a given file belongs to within a workspace,
//! and then executes a subcommand with the appropriate package scope. It supports two operating modes:
//! cargo integration mode (default) and environment variable mode.
//!
//! # Usage
//!
//! ```text
//! cargo detect-package --path <PATH> [--via-env <ENV_VAR>] <SUBCOMMAND>...
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
//! cargo detect-package --path packages/events/src/lib.rs build
//! # Executes: cargo build -p events
//!
//! # Test the package containing a specific test file
//! cargo detect-package --path packages/many_cpus/tests/integration.rs test
//! # Executes: cargo test -p many_cpus
//!
//! # Check a file in the workspace root (falls back to workspace)
//! cargo detect-package --path README.md check
//! # Executes: cargo check --workspace
//!
//! # Run clippy with additional arguments
//! cargo detect-package --path packages/events/src/lib.rs clippy -- -D warnings
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
//! cargo detect-package --path packages/events/src/lib.rs --via-env package just build
//! # Executes: just build (with package=events environment variable)
//!
//! # Use with custom script
//! cargo detect-package --path packages/many_cpus/src/lib.rs --via-env PKG_NAME ./build.sh
//! # Executes: ./build.sh (with PKG_NAME=many_cpus environment variable)
//!
//! # Workspace scope with environment variable
//! cargo detect-package --path nonexistent.rs --via-env package just test
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
//! # Workspace Validation
//!
//! The tool requires that both the current directory and target path are within the same Cargo workspace.
//! Cross-workspace operations are rejected with an error.
//! # Fallback Behavior
//!
//! The tool gracefully handles edge cases by falling back to workspace scope:
//! - Non-existent files or directories
//! - Files outside the workspace  
//! - Files in the workspace root that don't belong to any specific package
//! - Invalid or missing package configuration

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use argh::FromArgs;
use toml::Value;

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
            let value: Value = contents.parse()?;
            if value.get("workspace").is_some() {
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
    let value: Value = contents.parse()?;

    if let Some(package_table) = value.get("package") {
        if let Some(name) = package_table.get("name") {
            if let Some(name_str) = name.as_str() {
                return Ok(DetectedPackage::Package(name_str.to_string()));
            }
        }
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

/// Validates that the current working directory and target path are within the same Cargo workspace.
/// This ensures the tool is only used when both locations are in the same workspace context.
fn validate_workspace_context(target_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let current_dir = std::env::current_dir()?;

    // Try to find workspace root from the current directory
    let current_workspace_root = find_workspace_root(&current_dir).map_err(|original_error| {
        format!("Current directory is not within a Cargo workspace: {original_error}")
    })?;

    // Try to resolve the target path - if it fails, we'll still validate against current workspace
    let target_workspace_root = if let Ok(absolute_target_path) = target_path.canonicalize() {
        // Target path exists, find its workspace root
        find_workspace_root(&absolute_target_path).map_err(|original_error| {
            format!("Target path is not within a Cargo workspace: {original_error}")
        })?
    } else {
        // Target path doesn't exist, but it might be within the current workspace
        let potential_target = if target_path.is_absolute() {
            target_path.to_path_buf()
        } else {
            // Try to resolve it relative to current directory
            current_dir.join(target_path)
        };

        // Manually resolve .. components since canonicalize() won't work for non-existent paths
        let normalized_potential = normalize_path(&potential_target);

        // For non-existent files, we can use the normalized path directly if it has no .. components
        // If it has .. components, we need to resolve them manually
        let resolved_target = if normalized_potential.to_string_lossy().contains("..") {
            resolve_path_components(&normalized_potential)
        } else {
            normalized_potential
        };

        // Walk up from the potential target directory to see if we can find the same workspace
        let target_dir = if resolved_target.extension().is_some() {
            // It's a file, use parent directory
            resolved_target.parent().unwrap_or(&current_dir)
        } else {
            // It's a directory path
            &resolved_target
        };

        // Normalize both paths for proper comparison
        let normalized_target_dir = normalize_path(target_dir);
        let normalized_workspace_root = normalize_path(&current_workspace_root);

        // Check if this path, when resolved, would be within the current workspace
        if normalized_target_dir.starts_with(&normalized_workspace_root) {
            current_workspace_root.clone()
        } else {
            return Err(format!(
                "Target path '{}' is not within the current workspace at '{}'",
                target_path.display(),
                current_workspace_root.display()
            )
            .into());
        }
    };

    // Verify both paths are in the same workspace
    // Normalize paths to handle Windows UNC path differences (\\?\)
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

/// Resolves path components including .. and . manually for paths that may not exist.
/// This is needed because `canonicalize()` only works on existing paths.
fn resolve_path_components(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    let mut has_prefix = false;

    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => {
                result.push(name);
            }
            std::path::Component::ParentDir => {
                // Remove the last component if possible
                result.pop();
            }
            std::path::Component::CurDir => {
                // Skip current directory references
            }
            std::path::Component::RootDir => {
                // On Windows with a prefix, RootDir is handled by the prefix
                // On Unix-like systems, this is the root "/"
                if !has_prefix {
                    result = PathBuf::from("/");
                }
            }
            std::path::Component::Prefix(prefix) => {
                // On Windows, preserve the prefix (e.g., C:) by starting fresh
                result = PathBuf::from(prefix.as_os_str());
                has_prefix = true;
            }
        }
    }

    result
}

/// Normalizes a path by removing Windows UNC prefixes and converting to a consistent format.
/// This helps with path comparisons on Windows where `canonicalize()` may return UNC paths.
fn normalize_path(path: &Path) -> PathBuf {
    // On Windows, canonicalize() may return UNC paths (\\?\) which can cause comparison issues
    // Strip the UNC prefix if present
    if let Some(path_str) = path.to_str() {
        if let Some(stripped) = path_str.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path.to_path_buf()
}

#[cfg(all(test, not(miri)))]
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
        let result = detect_package(Path::new("nonexistent/file.rs")).unwrap();
        assert_eq!(result, DetectedPackage::Workspace);
    }

    #[test]
    fn validate_workspace_context_from_workspace() {
        // This test assumes we're running from within the workspace
        // Since this is a package in the workspace, it should succeed
        let current_file = Path::new("src/main.rs");
        validate_workspace_context(current_file).expect("Should be running from within workspace");
    }

    #[test]
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
    fn validate_workspace_context_relative_path_outside() {
        // Test that relative paths going outside the workspace are rejected
        let result = validate_workspace_context(Path::new("../../../outside_workspace/file.rs"));
        assert!(result.is_err(), "Expected error but validation succeeded!");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("is not within the current workspace")
        );
    }
}
