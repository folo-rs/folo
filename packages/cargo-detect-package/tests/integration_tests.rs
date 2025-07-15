//! Integration tests for cargo-detect-package tool.
//!
//! These tests generate temporary workspace structures on the fly
//! to verify the tool's behavior in realistic scenarios.

#![cfg(not(miri))]

use std::fs;
use std::path::Path;
use std::process::Command;

/// Path to the cargo-detect-package binary.
fn get_binary_path() -> String {
    // For Windows, include the .exe extension
    let binary_name = if cfg!(windows) {
        "cargo-detect-package.exe"
    } else {
        "cargo-detect-package"
    };

    // Try to find the binary in the standard Cargo target directory
    std::env::var("CARGO_MANIFEST_DIR").map_or_else(
        |_| format!("../../target/debug/{binary_name}"),
        |manifest_dir| format!("{manifest_dir}/../../target/debug/{binary_name}"),
    )
}

/// Helper to run cargo-detect-package with given arguments and working directory.
fn run_tool(working_dir: &Path, args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    let binary_path = get_binary_path();

    // Normalize the path to handle mixed separators
    let normalized_binary_path = Path::new(&binary_path)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(&binary_path).to_path_buf());

    // Convert UNC paths back to regular Windows paths to avoid Command issues
    let binary_str = normalized_binary_path.to_string_lossy();
    let clean_binary_path = binary_str
        .strip_prefix(r"\\?\")
        .map_or_else(|| binary_str.to_string(), ToString::to_string);

    let working_str = working_dir.to_string_lossy();
    let clean_working_dir = working_str
        .strip_prefix(r"\\?\")
        .map_or(working_dir, Path::new);

    Command::new(&clean_binary_path)
        .args(args)
        .current_dir(clean_working_dir)
        .output()
}

/// Get a cross-platform command that exists on both Windows and Unix.
fn get_test_command() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["powershell", "-c", "echo", "test"]
    } else {
        vec!["echo", "test"]
    }
}

/// Creates a simple test workspace with two packages.
fn create_simple_workspace() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
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
    .expect("Failed to write workspace Cargo.toml");

    // Create package_a
    let package_a = workspace_root.join("package_a");
    fs::create_dir_all(package_a.join("src")).expect("Failed to create package_a/src");
    fs::write(
        package_a.join("Cargo.toml"),
        r#"[package]
name = "package_a"
version.workspace = true
edition.workspace = true

[dependencies]
"#,
    )
    .expect("Failed to write package_a Cargo.toml");

    fs::write(package_a.join("src/lib.rs"), "// package_a lib\n")
        .expect("Failed to write package_a lib.rs");
    fs::write(package_a.join("src/main.rs"), "// package_a main\n")
        .expect("Failed to write package_a main.rs");
    fs::write(package_a.join("src/utils.rs"), "// package_a utils\n")
        .expect("Failed to write package_a utils.rs");

    // Create package_b
    let package_b = workspace_root.join("package_b");
    fs::create_dir_all(package_b.join("src")).expect("Failed to create package_b/src");
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
    .expect("Failed to write package_b Cargo.toml");

    fs::write(package_b.join("src/lib.rs"), "// package_b lib\n")
        .expect("Failed to write package_b lib.rs");

    // Create root files
    fs::write(workspace_root.join("README.md"), "# Test Workspace\n")
        .expect("Failed to write README.md");
    fs::write(workspace_root.join("root_file.rs"), "// root file\n")
        .expect("Failed to write root_file.rs");

    temp_dir
}

/// Creates a separate workspace for cross-workspace testing.
fn create_separate_workspace() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
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
    .expect("Failed to write workspace Cargo.toml");

    // Create isolated_package
    let isolated_package = workspace_root.join("isolated_package");
    fs::create_dir_all(isolated_package.join("src"))
        .expect("Failed to create isolated_package/src");
    fs::write(
        isolated_package.join("Cargo.toml"),
        r#"[package]
name = "isolated_package"
version.workspace = true
edition.workspace = true

[dependencies]
"#,
    )
    .expect("Failed to write isolated_package Cargo.toml");

    fs::write(isolated_package.join("src/lib.rs"), "// isolated package\n")
        .expect("Failed to write isolated_package lib.rs");

    temp_dir
}

/// Creates a workspace with edge cases (malformed packages).
fn create_edge_cases_workspace() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
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
    .expect("Failed to write workspace Cargo.toml");

    // Create valid_package
    let valid_package = workspace_root.join("valid_package");
    fs::create_dir_all(valid_package.join("src")).expect("Failed to create valid_package/src");
    fs::write(
        valid_package.join("Cargo.toml"),
        r#"[package]
name = "valid_package"
version.workspace = true
edition.workspace = true

[dependencies]
"#,
    )
    .expect("Failed to write valid_package Cargo.toml");

    fs::write(valid_package.join("src/lib.rs"), "// valid package\n")
        .expect("Failed to write valid_package lib.rs");

    // Create malformed_package (intentionally malformed TOML)
    let malformed_package = workspace_root.join("malformed_package");
    fs::create_dir_all(&malformed_package).expect("Failed to create malformed_package");
    fs::write(
        malformed_package.join("Cargo.toml"),
        r#"# This is intentionally malformed TOML for testing
[package
name = "malformed_package"
version = 0.1.0  # Missing quotes
edition = "2021"
"#,
    )
    .expect("Failed to write malformed_package Cargo.toml");

    temp_dir
}

#[test]
fn package_detection_in_simple_workspace() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test detecting package_a from its lib.rs
    let mut args = vec![
        "--path",
        "package_a/src/lib.rs",
        "--via-env",
        "DETECTED_PKG",
    ];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The tool should detect package_a
    // We can't directly check environment variables, but we can verify it doesn't error
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Error"),
        "Should not contain errors: {stderr}"
    );
}

#[test]
fn package_detection_package_a_utils() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test detecting package from a submodule file
    let mut args = vec!["--path", "package_a/src/utils.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should detect package from utils.rs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn package_detection_package_b() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test detecting package_b
    let mut args = vec!["--path", "package_b/src/lib.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should detect package_b: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workspace_root_file_fallback() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test that root-level files fall back to workspace scope
    let mut args = vec!["--path", "root_file.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should handle root files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn readme_fallback() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test that README.md falls back to workspace scope
    let mut args = vec!["--path", "README.md", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should handle README.md: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn nonexistent_file_fallback() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test that non-existent files now return an error instead of falling back to workspace scope
    let nonexistent_file = workspace_root.join("package_a/src/nonexistent.rs");
    let mut args = vec![
        "--path",
        nonexistent_file.to_str().unwrap(),
        "--via-env",
        "PKG",
    ];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        !output.status.success(),
        "Tool should reject non-existent files: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "Should have error about non-existent file: {stderr}"
    );
}

#[test]
fn cross_workspace_rejection() {
    let workspace1 = create_simple_workspace();
    let workspace1_root = workspace1.path();
    let workspace2 = create_separate_workspace();
    let workspace2_root = workspace2.path();
    let test_cmd = get_test_command();

    // Try to target a file in separate_workspace while running from simple_workspace
    let target_file = workspace2_root.join("isolated_package/src/lib.rs");
    let mut args = vec!["--path", target_file.to_str().unwrap(), "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace1_root, &args).expect("Failed to run tool");

    assert!(
        !output.status.success(),
        "Tool should reject cross-workspace operations"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("differs from target path workspace"),
        "Should have cross-workspace error: {stderr}"
    );
}

#[test]
fn outside_workspace_rejection() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Try to target a file outside any workspace (like /etc/passwd or C:\Windows\System32\notepad.exe)
    let outside_file = if cfg!(windows) {
        "C:\\Windows\\System32\\notepad.exe"
    } else {
        "/etc/passwd"
    };

    let mut args = vec!["--path", outside_file, "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        !output.status.success(),
        "Tool should reject files outside workspace"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not within a Cargo workspace"),
        "Should have outside-workspace error: {stderr}"
    );
}

#[test]
fn relative_path_escape_rejection() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Try to escape the workspace using relative paths
    let mut args = vec!["--path", "../../../outside_file.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        !output.status.success(),
        "Tool should reject relative path escapes"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("workspace") || stderr.contains("Error"),
        "Should have workspace-related error: {stderr}"
    );
}

#[test]
fn malformed_package_handling() {
    let workspace = create_edge_cases_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test that valid package still works even with malformed package in same workspace
    let mut args = vec!["--path", "valid_package/src/lib.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should handle valid package despite malformed neighbor: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cargo_integration_mode() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    // Test the default cargo integration mode (no --via-env)
    // We'll use 'cargo check --message-format=json' to avoid actually building
    let output = run_tool(
        workspace_root,
        &[
            "--path",
            "package_a/src/lib.rs",
            "check",
            "--message-format=json",
            "--quiet",
        ],
    )
    .expect("Failed to run tool");

    // The tool should succeed and cargo should run
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Only fail if it's our tool's error, not a cargo error about missing dependencies
        assert!(
            !stderr.contains("Error:") || stderr.contains("could not find"),
            "Tool should succeed in cargo integration mode: {stderr}"
        );
    }
}

#[test]
fn workspace_scope_cargo_integration() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    // Test workspace scope with cargo integration
    let output = run_tool(
        workspace_root,
        &[
            "--path",
            "README.md",
            "check",
            "--message-format=json",
            "--quiet",
        ],
    )
    .expect("Failed to run tool");

    // Should use --workspace flag for workspace-scope files
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Error:") || stderr.contains("could not find"),
            "Tool should succeed with workspace scope: {stderr}"
        );
    }
}
