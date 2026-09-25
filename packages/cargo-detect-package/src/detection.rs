// Package detection logic.
//
// This module contains the core logic for detecting which Cargo package a file belongs to.

use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::PackageNameMissingError;
use crate::manifest::read_manifest;
use crate::pal::Filesystem;

/// Represents the result of package detection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DetectedPackage {
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
pub(crate) struct WorkspaceContext {
    /// The canonicalized absolute path to the target file or directory.
    pub(crate) absolute_target_path: PathBuf,
    /// The canonicalized path to the workspace root.
    pub(crate) workspace_root: PathBuf,
}

/// Detects which Cargo package the given file belongs to.
///
/// Takes a `WorkspaceContext` which contains the already-validated and canonicalized paths,
/// avoiding redundant filesystem lookups.
// Mutations to loop conditions can cause infinite loops, timing out tests.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn detect_package(
    context: &WorkspaceContext,
    fs: &impl Filesystem,
) -> Result<DetectedPackage, AppError> {
    let absolute_path = &context.absolute_target_path;
    let workspace_root = &context.workspace_root;

    // Start from the file's directory and walk up to find the nearest Cargo.toml.
    let mut current_dir = if fs.is_file(absolute_path) {
        absolute_path.parent().unwrap()
    } else {
        absolute_path
    };

    while current_dir.starts_with(workspace_root) {
        if fs.cargo_toml_exists(current_dir) && current_dir != workspace_root {
            // Found a package-level Cargo.toml, extract the package name.
            return extract_package_name(current_dir, fs);
        }

        current_dir = match current_dir.parent() {
            Some(parent) => parent,
            None => break,
        };
    }

    // No package found, use workspace.
    Ok(DetectedPackage::Workspace)
}

/// Extracts the package name from a Cargo.toml file in the given directory.
pub(crate) fn extract_package_name(
    dir: &Path,
    fs: &impl Filesystem,
) -> Result<DetectedPackage, AppError> {
    let manifest = read_manifest(dir, fs)?;

    if let Some(package_table) = manifest.get("package")
        && let Some(name) = package_table.get("name")
        && let Some(name_str) = name.as_str()
    {
        return Ok(DetectedPackage::Package(name_str.to_string()));
    }

    Err(PackageNameMissingError::new(dir.join("Cargo.toml")).into())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::ParseManifestError;
    use crate::pal::MockFilesystem;

    fn manifest_filesystem(manifest: &str) -> MockFilesystem {
        let mut fs = MockFilesystem::new();
        let manifest = manifest.to_owned();
        fs.expect_read_cargo_toml()
            .returning(move |_| Ok(manifest.clone()));
        fs
    }

    #[test]
    fn extract_package_name_double_quotes() {
        let fs = manifest_filesystem(
            r#"
[package]
name = "test-package"
version = "0.1.0"
"#,
        );

        let result = extract_package_name(Path::new("package"), &fs).unwrap();
        assert_eq!(result, DetectedPackage::Package("test-package".to_string()));
    }

    #[test]
    fn extract_package_name_single_quotes() {
        let fs = manifest_filesystem(
            r#"
[package]
name = 'test-package-single'
version = "0.1.0"
"#,
        );

        let result = extract_package_name(Path::new("package"), &fs).unwrap();
        assert_eq!(
            result,
            DetectedPackage::Package("test-package-single".to_string())
        );
    }

    #[test]
    fn extract_package_name_with_comments_and_complex_toml() {
        let fs = manifest_filesystem(
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
        );

        let result = extract_package_name(Path::new("package"), &fs).unwrap();
        assert_eq!(
            result,
            DetectedPackage::Package("complex-package".to_string())
        );
    }

    #[test]
    fn extract_package_name_missing() {
        let fs = manifest_filesystem(
            r#"
[package]
version = "0.1.0"
"#,
        );

        let error = extract_package_name(Path::new("package"), &fs).unwrap_err();
        assert!(error.find_source::<PackageNameMissingError>().is_some());
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
    fn detect_package_with_invalid_toml() {
        let workspace_root = Path::new("workspace");
        let bad_package = workspace_root.join("bad_package");
        let mut fs = manifest_filesystem(
            r#"# Intentionally malformed TOML - missing closing bracket
[package
name = "bad_package"
version = "0.1.0"
"#,
        );
        fs.expect_is_file().return_const(true);
        fs.expect_cargo_toml_exists()
            .returning(move |path| path == bad_package);
        let context = WorkspaceContext {
            absolute_target_path: workspace_root.join("bad_package/src/lib.rs"),
            workspace_root: workspace_root.to_owned(),
        };

        let error = detect_package(&context, &fs).unwrap_err();
        assert!(error.find_source::<ParseManifestError>().is_some());
    }
}
