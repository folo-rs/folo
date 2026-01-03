// Workspace validation logic.
//
// This module contains logic for validating that paths are within a Cargo workspace
// and finding the workspace root.

use std::path::{Path, PathBuf};

use toml::Value;

use crate::detection::WorkspaceContext;
use crate::pal::Filesystem;

/// Validates that the current working directory and target path are within the same Cargo
/// workspace. This ensures the tool is only used when both locations are in the same workspace
/// context.
///
/// Returns a `WorkspaceContext` containing the canonicalized target path and workspace root,
/// which can be reused by subsequent operations to avoid redundant filesystem lookups.
pub(crate) fn validate_workspace_context(
    target_path: &Path,
    fs: &impl Filesystem,
) -> Result<WorkspaceContext, Box<dyn std::error::Error>> {
    let current_dir = fs.current_dir()?;

    // Find workspace root from the current directory.
    let current_workspace_root =
        find_workspace_root(&current_dir, fs).map_err(|original_error| {
            format!("Current directory is not within a Cargo workspace: {original_error}")
        })?;

    // Resolve the target path - try to make it absolute.
    let resolved_target_path = if target_path.is_absolute() {
        target_path.to_path_buf()
    } else {
        // For relative paths, try relative to current directory first.
        let relative_to_current = current_dir.join(target_path);
        if fs.exists(&relative_to_current) {
            relative_to_current
        } else {
            // If that does not exist, try relative to workspace root.
            // This handles cases where the tool is run from a different directory.
            current_workspace_root.join(target_path)
        }
    };

    // Canonicalize the resolved target path - it must exist.
    let absolute_target_path = fs.canonicalize(&resolved_target_path).map_err(|error| {
        format!(
            "Target path '{}' does not exist or cannot be accessed: {error}",
            target_path.display()
        )
    })?;

    // Find workspace root for the target path.
    let target_workspace_root =
        find_workspace_root(&absolute_target_path, fs).map_err(|original_error| {
            format!("Target path is not within a Cargo workspace: {original_error}")
        })?;

    // Verify both paths are in the same workspace.
    // Normalize paths to handle Windows path representation differences.
    let current_workspace_normalized = normalize_path(&current_workspace_root, fs);
    let target_workspace_normalized = normalize_path(&target_workspace_root, fs);

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
    let absolute_target_path_normalized = normalize_path(&absolute_target_path, fs);

    Ok(WorkspaceContext {
        absolute_target_path: absolute_target_path_normalized,
        workspace_root: target_workspace_normalized,
    })
}

/// Finds the workspace root by looking for the workspace-level Cargo.toml.
// Mutations to this function cause infinite loops or hangs in integration tests.
#[cfg_attr(test, mutants::skip)]
fn find_workspace_root(
    start_path: &Path,
    fs: &impl Filesystem,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut current_dir = start_path;

    loop {
        if fs.cargo_toml_exists(current_dir) {
            // Check if this is a workspace root.
            let contents = fs.read_cargo_toml(current_dir)?;
            let value: Value = toml::from_str(&contents)?;
            if value.get("workspace").is_some() {
                // Return canonicalized path for consistent comparison.
                return Ok(fs
                    .canonicalize(current_dir)
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

/// Normalizes a path by using OS canonicalization and stripping Windows UNC prefixes.
/// This helps with path comparisons on Windows where paths may have different representations.
fn normalize_path(path: &Path, fs: &impl Filesystem) -> PathBuf {
    // Canonicalize the path (paths are expected to exist).
    let canonical = fs.canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

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
    use std::path::Path;

    use serial_test::serial;

    use super::*;
    use crate::pal::FilesystemFacade;

    #[test]
    fn validate_workspace_context_nonexistent_file() {
        // Nonexistent files are now rejected by validate_workspace_context, not detect_package.
        let fs = FilesystemFacade::target();
        let result = validate_workspace_context(Path::new("nonexistent/file.rs"), &fs);
        assert!(
            result.is_err(),
            "Should return error for non-existent files"
        );
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    /// Creates a minimal temporary Cargo workspace for tests.
    fn create_minimal_workspace_for_validation() -> tempfile::TempDir {
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
    #[serial] // This test changes the global working directory, so must run serially.
    fn validate_workspace_context_from_workspace() {
        // This test ensures validation works when both current dir and target are in the same
        // workspace. We use a temporary workspace to avoid running against the actual repo.
        let workspace = create_minimal_workspace_for_validation();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(workspace.path()).unwrap();

        let target_file = Path::new("test_pkg/src/lib.rs");

        let fs = FilesystemFacade::target();
        let result = validate_workspace_context(target_file, &fs);

        std::env::set_current_dir(original_dir).unwrap();

        result.expect("Should validate successfully when both paths are in the same workspace");
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
        let fs = FilesystemFacade::target();
        let result = validate_workspace_context(target_path, &fs);
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
        let fs = FilesystemFacade::target();
        let result = validate_workspace_context(&other_workspace_file, &fs);
        result.unwrap_err();

        // Restore original directory.
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    #[serial] // This test uses a relative path that depends on the current working directory for resolution.
    fn validate_workspace_context_relative_path_outside() {
        let fs = FilesystemFacade::target();
        let result =
            validate_workspace_context(Path::new("../../../outside_workspace/file.rs"), &fs);
        assert!(result.is_err(), "Expected error but validation succeeded!");
        // The error could be about the file not existing or being outside workspace.
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("does not exist")
                || error_msg.contains("is not within the current workspace"),
            "Expected appropriate error message, got: {error_msg}"
        );
    }
}
