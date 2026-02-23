// Package detection logic.
//
// This module contains the core logic for detecting which Cargo package a file belongs to.

use std::error;
use std::path::{Path, PathBuf};

use toml::Value;

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
) -> Result<DetectedPackage, Box<dyn error::Error>> {
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
) -> Result<DetectedPackage, Box<dyn error::Error>> {
    let contents = fs.read_cargo_toml(dir)?;
    let value: Value = toml::from_str(&contents)?;

    if let Some(package_table) = value.get("package")
        && let Some(name) = package_table.get("name")
        && let Some(name_str) = name.as_str()
    {
        return Ok(DetectedPackage::Package(name_str.to_string()));
    }

    Err(format!(
        "Could not find package name in {}/Cargo.toml",
        dir.display()
    )
    .into())
}

#[cfg(all(test, not(miri)))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fs;

    use super::*;
    use crate::pal::FilesystemFacade;

    #[test]
    fn extract_package_name_double_quotes() {
        let temp_dir = tempfile::tempdir().unwrap();

        fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"
[package]
name = "test-package"
version = "0.1.0"
"#,
        )
        .unwrap();

        let fs = FilesystemFacade::target();
        let result = extract_package_name(temp_dir.path(), &fs).unwrap();
        assert_eq!(result, DetectedPackage::Package("test-package".to_string()));
    }

    #[test]
    fn extract_package_name_single_quotes() {
        let temp_dir = tempfile::tempdir().unwrap();

        fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"
[package]
name = 'test-package-single'
version = "0.1.0"
"#,
        )
        .unwrap();

        let fs = FilesystemFacade::target();
        let result = extract_package_name(temp_dir.path(), &fs).unwrap();
        assert_eq!(
            result,
            DetectedPackage::Package("test-package-single".to_string())
        );
    }

    #[test]
    fn extract_package_name_with_comments_and_complex_toml() {
        let temp_dir = tempfile::tempdir().unwrap();

        fs::write(
            temp_dir.path().join("Cargo.toml"),
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

        let fs = FilesystemFacade::target();
        let result = extract_package_name(temp_dir.path(), &fs).unwrap();
        assert_eq!(
            result,
            DetectedPackage::Package("complex-package".to_string())
        );
    }

    #[test]
    fn extract_package_name_missing() {
        let temp_dir = tempfile::tempdir().unwrap();

        fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"
[package]
version = "0.1.0"
"#,
        )
        .unwrap();

        let fs = FilesystemFacade::target();
        extract_package_name(temp_dir.path(), &fs)
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

        let fs = FilesystemFacade::target();
        // detect_package should fail because the Cargo.toml is malformed.
        let result = detect_package(&context, &fs);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("TOML") || error_msg.contains("parse"),
            "Expected TOML parse error, got: {error_msg}"
        );
    }
}
