//! Integration tests for cargo-detect-package tool.
//!
//! These tests use pre-built test workspace structures in the `test_data/` directory
//! to verify the tool's behavior in realistic scenarios.

#![cfg(not(miri))]

use std::path::Path;
use std::process::Command;

/// Path to the test data directory.
const TEST_DATA_DIR: &str = "tests/test_data";

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

/// Helper to get the absolute path to a test data workspace.
fn test_workspace_path(workspace_name: &str) -> std::path::PathBuf {
    let test_data_path = Path::new(TEST_DATA_DIR).join(workspace_name);

    // Try to canonicalize, but fall back to the original path if that fails
    test_data_path.canonicalize().unwrap_or(test_data_path)
}

#[test]
fn package_detection_in_simple_workspace() {
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Test detecting package_a from its lib.rs
    let mut args = vec![
        "--path",
        "package_a/src/lib.rs",
        "--via-env",
        "DETECTED_PKG",
    ];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

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
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Test detecting package from a submodule file
    let mut args = vec!["--path", "package_a/src/utils.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should detect package from utils.rs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn package_detection_package_b() {
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Test detecting package_b
    let mut args = vec!["--path", "package_b/src/lib.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should detect package_b: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workspace_root_file_fallback() {
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Test that root-level files fall back to workspace scope
    let mut args = vec!["--path", "root_file.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should handle root files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn readme_fallback() {
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Test that README.md falls back to workspace scope
    let mut args = vec!["--path", "README.md", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should handle README.md: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn nonexistent_file_fallback() {
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Test that non-existent files fall back to workspace scope
    // Use an absolute path instead of relative to avoid workspace validation issues
    let nonexistent_file = workspace.join("package_a/src/nonexistent.rs");
    let mut args = vec![
        "--path",
        nonexistent_file.to_str().unwrap(),
        "--via-env",
        "PKG",
    ];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should handle non-existent files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cross_workspace_rejection() {
    let workspace1 = test_workspace_path("simple_workspace");
    let workspace2 = test_workspace_path("separate_workspace");
    let test_cmd = get_test_command();

    // Try to target a file in separate_workspace while running from simple_workspace
    let target_file = workspace2.join("isolated_package/src/lib.rs");
    let mut args = vec!["--path", target_file.to_str().unwrap(), "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace1, &args).expect("Failed to run tool");

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
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Try to target a file outside any workspace (like /etc/passwd or C:\Windows\System32\notepad.exe)
    let outside_file = if cfg!(windows) {
        "C:\\Windows\\System32\\notepad.exe"
    } else {
        "/etc/passwd"
    };

    let mut args = vec!["--path", outside_file, "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

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
    let workspace = test_workspace_path("simple_workspace");
    let test_cmd = get_test_command();

    // Try to escape the workspace using relative paths
    let mut args = vec!["--path", "../../../outside_file.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

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
    let workspace = test_workspace_path("edge_cases_workspace");
    let test_cmd = get_test_command();

    // Test that valid package still works even with malformed package in same workspace
    let mut args = vec!["--path", "valid_package/src/lib.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(&workspace, &args).expect("Failed to run tool");

    assert!(
        output.status.success(),
        "Tool should handle valid package despite malformed neighbor: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cargo_integration_mode() {
    let workspace = test_workspace_path("simple_workspace");

    // Test the default cargo integration mode (no --via-env)
    // We'll use 'cargo check --message-format=json' to avoid actually building
    let output = run_tool(
        &workspace,
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
    let workspace = test_workspace_path("simple_workspace");

    // Test workspace scope with cargo integration
    let output = run_tool(
        &workspace,
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
