//! Integration tests for cargo-detect-package tool.
//!
//! These tests generate temporary workspace structures on the fly
//! to verify the tool's behavior in realistic scenarios.
//!
//! Tests call the `run()` function directly instead of spawning a subprocess,
//! which allows proper code coverage measurement.

use std::fs;
use std::path::PathBuf;

use cargo_detect_package::{OutsidePackageAction, RunInput, RunOutcome, run};
use serial_test::serial;

/// Get a cross-platform command that exists on both Windows and Unix.
fn get_test_command() -> Vec<String> {
    if cfg!(windows) {
        vec![
            "powershell".to_string(),
            "-c".to_string(),
            "echo".to_string(),
            "test".to_string(),
        ]
    } else {
        vec!["echo".to_string(), "test".to_string()]
    }
}

/// Creates a simple test workspace with two packages.
fn create_simple_workspace() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_root = temp_dir.path();

    // Create workspace Cargo.toml
    fs::write(
        workspace_root.join("Cargo.toml"),
        r#"[workspace]
members = ["package_a", "package_b"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // Create package_a
    let package_a = workspace_root.join("package_a");
    fs::create_dir_all(package_a.join("src")).unwrap();
    fs::write(
        package_a.join("Cargo.toml"),
        r#"[package]
name = "package_a"
version.workspace = true
edition.workspace = true

[dependencies]
"#,
    )
    .unwrap();

    fs::write(package_a.join("src/lib.rs"), "// package_a lib\n").unwrap();
    fs::write(package_a.join("src/main.rs"), "// package_a main\n").unwrap();
    fs::write(package_a.join("src/utils.rs"), "// package_a utils\n").unwrap();

    // Create package_b
    let package_b = workspace_root.join("package_b");
    fs::create_dir_all(package_b.join("src")).unwrap();
    fs::write(
        package_b.join("Cargo.toml"),
        r#"[package]
name = "package_b"
version.workspace = true
edition.workspace = true

[dependencies]
package_a = { path = "../package_a" }
"#,
    )
    .unwrap();

    fs::write(package_b.join("src/lib.rs"), "// package_b lib\n").unwrap();

    // Create root files
    fs::write(workspace_root.join("README.md"), "# Test Workspace\n").unwrap();
    fs::write(workspace_root.join("root_file.rs"), "// root file\n").unwrap();

    temp_dir
}

/// Creates a workspace and an existing sibling file outside it under one temporary root.
fn create_workspace_with_sibling_file() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_root = temp_dir.path().join("workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    fs::write(
        workspace_root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();

    let outside_file = temp_dir.path().join("outside").join("file.rs");
    fs::create_dir_all(outside_file.parent().unwrap()).unwrap();
    fs::write(&outside_file, "// outside the workspace\n").unwrap();

    (temp_dir, workspace_root, outside_file)
}

/// Creates a separate workspace for cross-workspace testing.
fn create_separate_workspace() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_root = temp_dir.path();

    // Create workspace Cargo.toml
    fs::write(
        workspace_root.join("Cargo.toml"),
        r#"[workspace]
members = ["isolated_package"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // Create isolated_package
    let isolated_package = workspace_root.join("isolated_package");
    fs::create_dir_all(isolated_package.join("src")).unwrap();
    fs::write(
        isolated_package.join("Cargo.toml"),
        r#"[package]
name = "isolated_package"
version.workspace = true
edition.workspace = true

[dependencies]
"#,
    )
    .unwrap();

    fs::write(isolated_package.join("src/lib.rs"), "// isolated package\n").unwrap();

    temp_dir
}

/// Creates a workspace with edge cases (malformed packages).
fn create_edge_cases_workspace() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_root = temp_dir.path();

    // Create workspace Cargo.toml
    fs::write(
        workspace_root.join("Cargo.toml"),
        r#"[workspace]
members = ["valid_package", "malformed_package"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // Create valid_package
    let valid_package = workspace_root.join("valid_package");
    fs::create_dir_all(valid_package.join("src")).unwrap();
    fs::write(
        valid_package.join("Cargo.toml"),
        r#"[package]
name = "valid_package"
version.workspace = true
edition.workspace = true

[dependencies]
"#,
    )
    .unwrap();

    fs::write(valid_package.join("src/lib.rs"), "// valid package\n").unwrap();

    // Create malformed_package (intentionally malformed TOML)
    let malformed_package = workspace_root.join("malformed_package");
    fs::create_dir_all(&malformed_package).unwrap();
    fs::write(
        malformed_package.join("Cargo.toml"),
        r#"# This is intentionally malformed TOML for testing
[package
name = "malformed_package"
version = 0.1.0  # Missing quotes
edition = "2021"
"#,
    )
    .unwrap();

    temp_dir
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn package_detection_in_simple_workspace() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("package_a/src/lib.rs"),
        via_env: Some("DETECTED_PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::PackageDetected { package_name, .. }) => {
            assert_eq!(package_name, "package_a");
        }
        other => panic!("Expected PackageDetected, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn package_detection_package_a_utils() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("package_a/src/utils.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::PackageDetected { package_name, .. }) => {
            assert_eq!(package_name, "package_a");
        }
        other => panic!("Expected PackageDetected, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn package_detection_package_b() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("package_b/src/lib.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::PackageDetected { package_name, .. }) => {
            assert_eq!(package_name, "package_b");
        }
        other => panic!("Expected PackageDetected, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn workspace_root_file_fallback() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("root_file.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::WorkspaceScope { .. }) => {}
        other => panic!("Expected WorkspaceScope, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn readme_fallback() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("README.md"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::WorkspaceScope { .. }) => {}
        other => panic!("Expected WorkspaceScope, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn nonexistent_file_error() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("package_a/src/nonexistent.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn cross_workspace_rejection() {
    let workspace1 = create_simple_workspace();
    let workspace1_root = workspace1.path();
    let workspace2 = create_separate_workspace();
    let workspace2_root = workspace2.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace1_root).unwrap();

    let target_file = workspace2_root.join("isolated_package/src/lib.rs");
    let input = RunInput {
        path: target_file,
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn outside_workspace_rejection() {
    let (_temp_dir, workspace_root, outside_file) = create_workspace_with_sibling_file();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&workspace_root).unwrap();

    let input = RunInput {
        path: outside_file,
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn relative_path_escape_rejection() {
    let (_temp_dir, workspace_root, _outside_file) = create_workspace_with_sibling_file();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(&workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("../outside/file.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn malformed_package_handling() {
    let workspace = create_edge_cases_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    // Valid package should still work even with malformed neighbor
    let input = RunInput {
        path: PathBuf::from("valid_package/src/lib.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::PackageDetected { package_name, .. }) => {
            assert_eq!(package_name, "valid_package");
        }
        other => panic!("Expected PackageDetected, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn cargo_integration_mode() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    // Test the default cargo integration mode (no --via-env)
    let input = RunInput {
        path: PathBuf::from("package_a/src/lib.rs"),
        via_env: None,
        outside_package: OutsidePackageAction::Workspace,
        subcommand: vec![
            "check".to_string(),
            "--message-format=json".to_string(),
            "--quiet".to_string(),
        ],
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    // The tool should succeed and cargo should run (success depends on cargo availability)
    match result {
        Ok(RunOutcome::PackageDetected { package_name, .. }) => {
            assert_eq!(package_name, "package_a");
        }
        other => panic!("Expected PackageDetected, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn workspace_scope_cargo_integration() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    // Test workspace scope with cargo integration
    let input = RunInput {
        path: PathBuf::from("README.md"),
        via_env: None,
        outside_package: OutsidePackageAction::Workspace,
        subcommand: vec![
            "check".to_string(),
            "--message-format=json".to_string(),
            "--quiet".to_string(),
        ],
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    // Should use --workspace flag for workspace-scope files
    match result {
        Ok(RunOutcome::WorkspaceScope { .. }) => {}
        other => panic!("Expected WorkspaceScope, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn missing_subcommand_error() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    // Provide path but no subcommand to execute
    let input = RunInput {
        path: PathBuf::from("package_a/src/lib.rs"),
        via_env: None,
        outside_package: OutsidePackageAction::Workspace,
        subcommand: vec![],
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn outside_package_action_ignore() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("README.md"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Ignore,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::Ignored) => {}
        other => panic!("Expected Ignored, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn outside_package_action_error() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("README.md"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Error,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn directory_path_instead_of_file() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("package_a/src"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    match result {
        Ok(RunOutcome::PackageDetected { package_name, .. }) => {
            assert_eq!(package_name, "package_a");
        }
        other => panic!("Expected PackageDetected, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn execution_outside_any_cargo_workspace() {
    // Create a temp directory that is NOT a Cargo workspace
    let temp_dir = tempfile::tempdir().unwrap();
    let non_workspace_root = temp_dir.path();

    // Create a simple file in the non-workspace
    fs::write(non_workspace_root.join("some_file.rs"), "// test\n").unwrap();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(non_workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("some_file.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn subcommand_with_double_dash_separator() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    // Test clippy-style command with "--" separator
    let input = RunInput {
        path: PathBuf::from("package_a/src/lib.rs"),
        via_env: None,
        outside_package: OutsidePackageAction::Workspace,
        subcommand: vec![
            "clippy".to_string(),
            "--all-features".to_string(),
            "--".to_string(),
            "-A".to_string(),
            "warnings".to_string(),
        ],
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    // The tool should detect the package correctly; clippy may or may not succeed
    match result {
        Ok(RunOutcome::PackageDetected { package_name, .. }) => {
            assert_eq!(package_name, "package_a");
        }
        other => panic!("Expected PackageDetected, got {other:?}"),
    }
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn invalid_toml_package_detection_error() {
    let workspace = create_edge_cases_workspace();
    let workspace_root = workspace.path();

    // Create a file inside the malformed_package to trigger parsing that Cargo.toml
    fs::create_dir_all(workspace_root.join("malformed_package/src")).unwrap();
    fs::write(
        workspace_root.join("malformed_package/src/lib.rs"),
        "// test\n",
    )
    .unwrap();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("malformed_package/src/lib.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: get_test_command(),
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    // Manifest validation fails before any child command is started.
    result.unwrap_err();
}

#[test]
#[serial] // Changes working directory.
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn nonexistent_executable_error() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(workspace_root).unwrap();

    let input = RunInput {
        path: PathBuf::from("package_a/src/lib.rs"),
        via_env: Some("PKG".to_string()),
        outside_package: OutsidePackageAction::Workspace,
        subcommand: vec!["nonexistent-binary-xyz-123456".to_string()],
    };

    let result = run(&input);

    std::env::set_current_dir(original_dir).unwrap();

    result.unwrap_err();
}
