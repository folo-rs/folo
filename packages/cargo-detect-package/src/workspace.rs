// Workspace validation logic.
//
// This module contains logic for validating that paths are within a Cargo workspace
// and finding the workspace root.

use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::detection::WorkspaceContext;
use crate::errors::CanonicalizeTargetPathError;
use crate::manifest::read_manifest;
use crate::pal::Filesystem;
use crate::{
    CurrentDirectoryError, CurrentDirectoryOutsideWorkspaceError, TargetPathOutsideWorkspaceError,
    WorkspaceMismatchError,
};

/// Validates that the current working directory and target path are within the same Cargo
/// workspace. This ensures the tool is only used when both locations are in the same workspace
/// context.
///
/// Returns a `WorkspaceContext` containing the canonicalized target path and workspace root,
/// which can be reused by subsequent operations to avoid redundant filesystem lookups.
pub(crate) fn validate_workspace_context(
    target_path: &Path,
    fs: &impl Filesystem,
) -> Result<WorkspaceContext, AppError> {
    let current_dir = fs.current_dir().map_err(CurrentDirectoryError::caused_by)?;

    // Find workspace root from the current directory. A read or parse failure of a manifest
    // that does exist propagates as itself: it says the manifest is broken, not that there is
    // no workspace. Only `None` means the walk reached the filesystem root without finding one.
    let current_workspace_root = find_workspace_root(&current_dir, fs)?
        .ok_or_else(CurrentDirectoryOutsideWorkspaceError::new)?;

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

    // Canonicalize the resolved target path - it must exist. The error names the resolved
    // path rather than the argument, because a relative argument is probed at two different
    // absolute locations and only the resolved one says where the lookup actually failed.
    // Normalizing immediately keeps the Windows verbatim prefix out of the workspace walk
    // below, out of the manifest diagnostics that walk can raise, and out of the
    // `starts_with()` comparisons in `detect_package()`.
    let canonical_target_path = fs
        .canonicalize(&resolved_target_path)
        .map_err(|error| CanonicalizeTargetPathError::caused_by(&resolved_target_path, error))?;
    let absolute_target_path = normalize_path(&canonical_target_path, fs);

    // Find workspace root for the target path, distinguishing a broken manifest from an
    // absent workspace for the same reason as above.
    let target_workspace_root = find_workspace_root(&absolute_target_path, fs)?
        .ok_or_else(TargetPathOutsideWorkspaceError::new)?;

    // Verify both paths are in the same workspace. Both roots arrive normalized, so they
    // compare in one path representation.
    if current_workspace_root != target_workspace_root {
        return Err(
            WorkspaceMismatchError::new(current_workspace_root, target_workspace_root).into(),
        );
    }

    Ok(WorkspaceContext {
        absolute_target_path,
        workspace_root: target_workspace_root,
    })
}

/// Finds the workspace root by looking for the workspace-level `Cargo.toml`.
///
/// Returns `Ok(None)` when no workspace-level manifest exists at or above `start_path`.
// Mutations to this function cause infinite loops or hangs in integration tests.
#[cfg_attr(test, mutants::skip)]
fn find_workspace_root(
    start_path: &Path,
    fs: &impl Filesystem,
) -> Result<Option<PathBuf>, AppError> {
    let mut current_dir = start_path;

    loop {
        if fs.cargo_toml_exists(current_dir) {
            // Check if this is a workspace root.
            let manifest = read_manifest(current_dir, fs)?;
            if manifest.get("workspace").is_some() {
                // Return a normalized path so comparisons see one representation and
                // diagnostics built from this root never carry a Windows verbatim prefix.
                return Ok(Some(normalize_path(current_dir, fs)));
            }
        }

        match current_dir.parent() {
            Some(parent) => current_dir = parent,
            None => break,
        }
    }

    Ok(None)
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
    #[serial] // This test depends on the current directory being inside a Cargo workspace.
    fn validate_workspace_context_nonexistent_file() {
        // Nonexistent files are now rejected by validate_workspace_context, not detect_package.
        let fs = FilesystemFacade::target();
        let error = validate_workspace_context(Path::new("nonexistent/file.rs"), &fs).unwrap_err();
        assert!(error.find_source::<CanonicalizeTargetPathError>().is_some());
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

        result.unwrap();
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
        let error = validate_workspace_context(target_path, &fs).unwrap_err();
        assert!(
            error
                .find_source::<CurrentDirectoryOutsideWorkspaceError>()
                .is_some()
        );

        // Restore original directory.
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    #[serial] // This test changes the global working directory, so must run serially.
    fn malformed_current_workspace_manifest_is_a_parse_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join("Cargo.toml"), "not valid TOML [").unwrap();
        fs::write(temp_dir.path().join("target.rs"), "// target\n").unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let filesystem = FilesystemFacade::target();
        let result = validate_workspace_context(Path::new("target.rs"), &filesystem);

        std::env::set_current_dir(original_dir).unwrap();

        let error = result.unwrap_err();
        assert!(error.find_source::<crate::ParseManifestError>().is_some());
        assert!(
            error
                .find_source::<CurrentDirectoryOutsideWorkspaceError>()
                .is_none()
        );
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
    #[serial] // This test changes the global working directory, so must run serially.
    fn validate_workspace_context_relative_path_outside() {
        // A relative path with `..` components can escape the workspace the current directory
        // belongs to. The whole tree is built under one temporary directory so the outcome
        // does not depend on what happens to exist above the checkout on this machine.
        let temp_dir = tempfile::tempdir().unwrap();

        let workspace_root = temp_dir.path().join("workspace");
        fs::create_dir_all(&workspace_root).unwrap();
        fs::write(
            workspace_root.join("Cargo.toml"),
            r#"[workspace]
members = []
resolver = "2"
"#,
        )
        .unwrap();

        // A sibling of the workspace root: it exists, so path resolution succeeds, but no
        // manifest at or above it declares a workspace.
        let outside_dir = temp_dir.path().join("outside_workspace");
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("file.rs"), "// outside any workspace\n").unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&workspace_root).unwrap();

        let filesystem = FilesystemFacade::target();
        let result =
            validate_workspace_context(Path::new("../outside_workspace/file.rs"), &filesystem);

        std::env::set_current_dir(original_dir).unwrap();

        let error = result.unwrap_err();
        assert!(
            error
                .find_source::<TargetPathOutsideWorkspaceError>()
                .is_some()
        );
    }
}
