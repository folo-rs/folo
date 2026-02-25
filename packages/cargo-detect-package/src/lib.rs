#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A Cargo tool to detect the package that a file belongs to, passing the package name
//! to a subcommand.
//!
//! This crate provides the core logic for package detection, exposed via the [`run`] function.
//! The binary entry point is in `main.rs`.

mod detection;
mod execution;
mod pal;
mod types;
mod workspace;

use detection::{DetectedPackage, detect_package};
use execution::{execute_with_cargo_args, execute_with_env_var};
use pal::{Filesystem, FilesystemFacade};
pub use types::*;
use workspace::validate_workspace_context;

/// Core logic of the tool, extracted for testability.
///
/// This function contains all the business logic without any process-global dependencies
/// like `std::env::args()`, making it suitable for direct testing.
#[doc(hidden)]
pub fn run(input: &RunInput) -> Result<RunOutcome, RunError> {
    run_with_filesystem(input, &FilesystemFacade::target())
}

/// Internal implementation of `run` that accepts a filesystem abstraction.
///
/// This allows mocking filesystem operations in tests.
fn run_with_filesystem(input: &RunInput, fs: &impl Filesystem) -> Result<RunOutcome, RunError> {
    // Validate that we are running from within the same workspace as the target path.
    // This also canonicalizes paths and finds the workspace root, which we reuse later.
    let workspace_context = validate_workspace_context(&input.path, fs)
        .map_err(|e| RunError::WorkspaceValidation(e.to_string()))?;

    let detected_package = detect_package(&workspace_context, fs)
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

    assert_early_exit_cases_handled(&detected_package, &input.outside_package);

    let exit_status = match &input.via_env {
        Some(env_var) => execute_with_env_var(
            &workspace_context.workspace_root,
            env_var,
            &detected_package,
            &input.subcommand,
        ),
        None => execute_with_cargo_args(
            &workspace_context.workspace_root,
            &detected_package,
            &input.subcommand,
        ),
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

/// Defense-in-depth check: verifies that the match block in `run()` has handled all early-exit
/// cases. If this assertion fails, it indicates a logic bug in the match block that should have
/// returned early for the Ignore and Error cases.
///
/// This assertion can never fail with the current code structure because the match block returns
/// early for both (Workspace, Ignore) and (Workspace, Error) cases.
// Defense-in-depth check that can never be reached due to earlier match block logic.
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
fn assert_early_exit_cases_handled(
    detected_package: &DetectedPackage,
    outside_package: &OutsidePackageAction,
) {
    let is_early_exit_case = matches!(
        (detected_package, outside_package),
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

// Mock-based tests that do not require real filesystem access.
// These tests duplicate key scenarios from the integration tests but use a mock filesystem,
// enabling fine-grained control over filesystem behavior (e.g., files disappearing mid-operation).
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod mock_tests {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::detection::WorkspaceContext;
    use crate::pal::{FilesystemFacade, MockFilesystem};

    /// Helper to create a mock filesystem for a simple workspace with one package.
    ///
    /// Structure:
    /// ```text
    /// /workspace/
    ///   Cargo.toml (workspace)
    ///   package_a/
    ///     Cargo.toml (package: "package_a")
    ///     src/
    ///       lib.rs
    ///   README.md
    /// ```
    fn create_simple_workspace_mock() -> MockFilesystem {
        let mut mock = MockFilesystem::new();

        // current_dir returns the workspace root.
        mock.expect_current_dir()
            .returning(|| Ok(PathBuf::from("/workspace")));

        // Set up cargo_toml_exists expectations.
        mock.expect_cargo_toml_exists().returning(|dir| {
            let path_str = dir.to_string_lossy();
            path_str == "/workspace" || path_str == "/workspace/package_a"
        });

        // Set up read_cargo_toml expectations.
        mock.expect_read_cargo_toml().returning(|dir| {
            let path_str = dir.to_string_lossy();
            if path_str == "/workspace" {
                Ok(r#"[workspace]
members = ["package_a"]
"#
                .to_string())
            } else if path_str == "/workspace/package_a" {
                Ok(r#"[package]
name = "package_a"
version = "0.1.0"
"#
                .to_string())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
            }
        });

        // Set up is_file expectations.
        mock.expect_is_file().returning(|path| {
            let path_str = path.to_string_lossy();
            path_str.ends_with(".rs") || path_str.ends_with(".md")
        });

        // Set up exists expectations.
        mock.expect_exists().returning(|path| {
            let path_str = path.to_string_lossy();
            path_str == "/workspace"
                || path_str == "/workspace/Cargo.toml"
                || path_str == "/workspace/package_a"
                || path_str == "/workspace/package_a/Cargo.toml"
                || path_str == "/workspace/package_a/src"
                || path_str == "/workspace/package_a/src/lib.rs"
                || path_str == "/workspace/README.md"
        });

        // Set up canonicalize expectations - returns path as-is for virtual filesystem.
        mock.expect_canonicalize()
            .returning(|path| Ok(path.to_path_buf()));

        mock
    }

    #[test]
    fn mock_package_detection_in_simple_workspace() {
        let mock = create_simple_workspace_mock();
        let fs = FilesystemFacade::from_mock(mock);

        let context =
            validate_workspace_context(Path::new("/workspace/package_a/src/lib.rs"), &fs).unwrap();

        let result = detect_package(&context, &fs).unwrap();

        assert_eq!(result, DetectedPackage::Package("package_a".to_string()));
    }

    #[test]
    fn mock_workspace_root_file_fallback() {
        let mock = create_simple_workspace_mock();
        let fs = FilesystemFacade::from_mock(mock);

        let context = validate_workspace_context(Path::new("/workspace/README.md"), &fs).unwrap();

        let result = detect_package(&context, &fs).unwrap();

        assert_eq!(result, DetectedPackage::Workspace);
    }

    #[test]
    fn mock_nonexistent_file_error() {
        let mut mock = MockFilesystem::new();

        mock.expect_current_dir()
            .returning(|| Ok(PathBuf::from("/workspace")));

        mock.expect_cargo_toml_exists()
            .returning(|dir| dir.to_string_lossy() == "/workspace");

        mock.expect_read_cargo_toml().returning(|dir| {
            if dir.to_string_lossy() == "/workspace" {
                Ok("[workspace]\nmembers = []\n".to_string())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
            }
        });

        mock.expect_exists().returning(|path| {
            let path_str = path.to_string_lossy();
            path_str == "/workspace" || path_str == "/workspace/Cargo.toml"
        });

        mock.expect_canonicalize().returning(|path| {
            let path_str = path.to_string_lossy();
            if path_str.contains("nonexistent") {
                Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
            } else {
                Ok(path.to_path_buf())
            }
        });

        let fs = FilesystemFacade::from_mock(mock);

        let result =
            validate_workspace_context(Path::new("/workspace/package_a/src/nonexistent.rs"), &fs);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn mock_file_disappears_during_package_detection() {
        // This test demonstrates the power of mocking - we can simulate a file disappearing
        // between the exists check and the read operation.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let file_exists = Arc::new(AtomicBool::new(true));
        let file_exists_clone = Arc::clone(&file_exists);

        let mut mock = MockFilesystem::new();

        mock.expect_current_dir()
            .returning(|| Ok(PathBuf::from("/workspace")));

        mock.expect_cargo_toml_exists().returning(move |dir| {
            let path_str = dir.to_string_lossy();
            if path_str == "/workspace/package_a" {
                // First call returns true, then we "delete" the file.
                let exists = file_exists_clone.load(Ordering::SeqCst);
                file_exists_clone.store(false, Ordering::SeqCst);
                exists
            } else {
                path_str == "/workspace"
            }
        });

        mock.expect_read_cargo_toml().returning(|dir| {
            let path_str = dir.to_string_lossy();
            if path_str == "/workspace" {
                Ok("[workspace]\nmembers = [\"package_a\"]\n".to_string())
            } else {
                // File was "deleted" - return error.
                Err(io::Error::new(io::ErrorKind::NotFound, "file was deleted"))
            }
        });

        mock.expect_is_file()
            .returning(|path| path.to_string_lossy().ends_with(".rs"));

        mock.expect_exists().returning(|_| true);

        mock.expect_canonicalize()
            .returning(|path| Ok(path.to_path_buf()));

        let fs = FilesystemFacade::from_mock(mock);

        // Create context directly to skip validation (which would also call the mock).
        let context = WorkspaceContext {
            absolute_target_path: PathBuf::from("/workspace/package_a/src/lib.rs"),
            workspace_root: PathBuf::from("/workspace"),
        };

        // The file exists when cargo_toml_exists is called, but disappears when we try to read it.
        let result = detect_package(&context, &fs);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("deleted"));
    }

    #[test]
    fn mock_invalid_toml_in_package() {
        let mut mock = MockFilesystem::new();

        mock.expect_current_dir()
            .returning(|| Ok(PathBuf::from("/workspace")));

        mock.expect_cargo_toml_exists().returning(|dir| {
            let path_str = dir.to_string_lossy();
            path_str == "/workspace" || path_str == "/workspace/bad_package"
        });

        mock.expect_read_cargo_toml().returning(|dir| {
            let path_str = dir.to_string_lossy();
            if path_str == "/workspace" {
                Ok("[workspace]\nmembers = [\"bad_package\"]\n".to_string())
            } else if path_str == "/workspace/bad_package" {
                // Return malformed TOML.
                Ok("[package\nname = \"bad\"\n".to_string())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
            }
        });

        mock.expect_is_file()
            .returning(|path| path.to_string_lossy().ends_with(".rs"));

        mock.expect_exists().returning(|_| true);

        mock.expect_canonicalize()
            .returning(|path| Ok(path.to_path_buf()));

        let fs = FilesystemFacade::from_mock(mock);

        let context = WorkspaceContext {
            absolute_target_path: PathBuf::from("/workspace/bad_package/src/lib.rs"),
            workspace_root: PathBuf::from("/workspace"),
        };

        let result = detect_package(&context, &fs);

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("TOML") || error_msg.contains("parse"),
            "Expected TOML parse error, got: {error_msg}"
        );
    }

    #[test]
    fn mock_package_without_name_field() {
        let mut mock = MockFilesystem::new();

        mock.expect_current_dir()
            .returning(|| Ok(PathBuf::from("/workspace")));

        mock.expect_cargo_toml_exists().returning(|dir| {
            let path_str = dir.to_string_lossy();
            path_str == "/workspace" || path_str == "/workspace/nameless"
        });

        mock.expect_read_cargo_toml().returning(|dir| {
            let path_str = dir.to_string_lossy();
            if path_str == "/workspace" {
                Ok("[workspace]\nmembers = [\"nameless\"]\n".to_string())
            } else if path_str == "/workspace/nameless" {
                // Valid TOML but missing package name.
                Ok("[package]\nversion = \"0.1.0\"\n".to_string())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
            }
        });

        mock.expect_is_file()
            .returning(|path| path.to_string_lossy().ends_with(".rs"));

        mock.expect_exists().returning(|_| true);

        mock.expect_canonicalize()
            .returning(|path| Ok(path.to_path_buf()));

        let fs = FilesystemFacade::from_mock(mock);

        let context = WorkspaceContext {
            absolute_target_path: PathBuf::from("/workspace/nameless/src/lib.rs"),
            workspace_root: PathBuf::from("/workspace"),
        };

        let result = detect_package(&context, &fs);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Could not find package name")
        );
    }

    #[test]
    fn mock_nested_package_detection() {
        // Test that we find the nearest package, not the workspace root.
        let mut mock = MockFilesystem::new();

        mock.expect_current_dir()
            .returning(|| Ok(PathBuf::from("/workspace")));

        mock.expect_cargo_toml_exists().returning(|dir| {
            let path_str = dir.to_string_lossy();
            path_str == "/workspace"
                || path_str == "/workspace/crates/inner"
                || path_str == "/workspace/crates"
        });

        mock.expect_read_cargo_toml().returning(|dir| {
            let path_str = dir.to_string_lossy();
            if path_str == "/workspace" {
                Ok("[workspace]\nmembers = [\"crates/*\"]\n".to_string())
            } else if path_str == "/workspace/crates/inner" {
                Ok("[package]\nname = \"inner-package\"\nversion = \"0.1.0\"\n".to_string())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
            }
        });

        mock.expect_is_file()
            .returning(|path| path.to_string_lossy().ends_with(".rs"));

        mock.expect_exists().returning(|_| true);

        mock.expect_canonicalize()
            .returning(|path| Ok(path.to_path_buf()));

        let fs = FilesystemFacade::from_mock(mock);

        let context = WorkspaceContext {
            absolute_target_path: PathBuf::from("/workspace/crates/inner/src/lib.rs"),
            workspace_root: PathBuf::from("/workspace"),
        };

        let result = detect_package(&context, &fs).unwrap();

        assert_eq!(
            result,
            DetectedPackage::Package("inner-package".to_string())
        );
    }

    #[test]
    fn mock_directory_target_instead_of_file() {
        let mock = create_simple_workspace_mock();
        let fs = FilesystemFacade::from_mock(mock);

        // Target is a directory, not a file.
        let context = WorkspaceContext {
            absolute_target_path: PathBuf::from("/workspace/package_a/src"),
            workspace_root: PathBuf::from("/workspace"),
        };

        let result = detect_package(&context, &fs).unwrap();

        assert_eq!(result, DetectedPackage::Package("package_a".to_string()));
    }

    #[test]
    fn mock_current_dir_outside_workspace() {
        let mut mock = MockFilesystem::new();

        mock.expect_current_dir()
            .returning(|| Ok(PathBuf::from("/some/random/path")));

        mock.expect_cargo_toml_exists().returning(|_| false);

        let fs = FilesystemFacade::from_mock(mock);

        let result = validate_workspace_context(Path::new("file.rs"), &fs);

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not within a Cargo workspace")
        );
    }

    #[test]
    fn mock_current_dir_fails() {
        let mut mock = MockFilesystem::new();

        mock.expect_current_dir().returning(|| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "access denied",
            ))
        });

        let fs = FilesystemFacade::from_mock(mock);

        let result = validate_workspace_context(Path::new("file.rs"), &fs);

        result.unwrap_err();
    }

    #[test]
    fn mock_detect_package_reaches_filesystem_root() {
        // This tests the case where try_get_parent() returns None because we have walked
        // up to the filesystem root. We simulate this by having a workspace at the root
        // and a target file directly in the root - when we try to get parent of root, we get None.
        let mut mock = MockFilesystem::new();

        mock.expect_is_file()
            .returning(|path| path.to_string_lossy() == "/file.rs");

        // No Cargo.toml exists anywhere except at root (which is the workspace root).
        mock.expect_cargo_toml_exists()
            .returning(|dir| dir.to_string_lossy() == "/");

        mock.expect_read_cargo_toml().returning(|dir| {
            if dir.to_string_lossy() == "/" {
                Ok("[workspace]\nmembers = []\n".to_string())
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
            }
        });

        let fs = FilesystemFacade::from_mock(mock);

        // Create a context where the file is at root level and workspace is root.
        // The parent of "/" is None, so try_get_parent will return None.
        let context = WorkspaceContext {
            absolute_target_path: PathBuf::from("/file.rs"),
            workspace_root: PathBuf::from("/"),
        };

        // Since the file is at root and there is no package-level Cargo.toml (only workspace),
        // and we will hit None from try_get_parent when trying to go above root,
        // detect_package should return Workspace.
        let result = detect_package(&context, &fs).unwrap();

        assert_eq!(result, DetectedPackage::Workspace);
    }
}
