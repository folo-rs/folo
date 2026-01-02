//! Integration tests for cargo-detect-package tool.
//!
//! These tests generate temporary workspace structures on the fly
//! to verify the tool's behavior in realistic scenarios.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the cargo-detect-package binary.
fn get_binary_path() -> PathBuf {
    // For Windows, include the .exe extension
    let binary_name = if cfg!(windows) {
        "cargo-detect-package.exe"
    } else {
        "cargo-detect-package"
    };

    // Honor explicit target override via CARGO_TARGET_DIR.
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target_dir).join("debug").join(binary_name);
    }

    // Try to find the binary relative to CARGO_MANIFEST_DIR
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let manifest_path = PathBuf::from(&manifest_dir);

        // Look for workspace root by going up from CARGO_MANIFEST_DIR
        // This handles both running from the workspace root and from the package directory.
        // We need to find the target directory relative to the workspace root.
        let workspace_root = manifest_path
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(&manifest_path);

        // When running under cargo-llvm-cov, binaries are placed in target/llvm-cov-target/debug
        // instead of target/debug. Try the llvm-cov path first since it's more specific.
        let llvm_cov_path = workspace_root
            .join("target/llvm-cov-target/debug")
            .join(binary_name);
        if llvm_cov_path.exists() {
            return llvm_cov_path;
        }

        // Fall back to the standard target directory
        let standard_path = workspace_root.join("target/debug").join(binary_name);
        if standard_path.exists() {
            return standard_path;
        }

        // If neither exists, return the standard path (will fail later with a clear error)
        return standard_path;
    }

    // Fallback when CARGO_MANIFEST_DIR is not set
    PathBuf::from("../../target/debug").join(binary_name)
}

/// Helper to run cargo-detect-package with given arguments and working directory.
fn run_tool(working_dir: &Path, args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    let binary_path = get_binary_path();

    // Normalize the path to handle mixed separators
    let normalized_binary_path = match binary_path.canonicalize() {
        Ok(path) => path,
        Err(_) => binary_path,
    };

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
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
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

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The tool should detect package_a
    // We cannot directly check environment variables, but we can verify it does not error
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Error"),
        "Should not contain errors: {stderr}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn package_detection_package_a_utils() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test detecting package from a submodule file
    let mut args = vec!["--path", "package_a/src/utils.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should detect package from utils.rs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn package_detection_package_b() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test detecting package_b
    let mut args = vec!["--path", "package_b/src/lib.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should detect package_b: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn workspace_root_file_fallback() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test that root-level files fall back to workspace scope
    let mut args = vec!["--path", "root_file.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should handle root files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn readme_fallback() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test that README.md falls back to workspace scope
    let mut args = vec!["--path", "README.md", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should handle README.md: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
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

    let output = run_tool(workspace_root, &args).unwrap();

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
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
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

    let output = run_tool(workspace1_root, &args).unwrap();

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
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
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

    let output = run_tool(workspace_root, &args).unwrap();

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
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn relative_path_escape_rejection() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Try to escape the workspace using relative paths
    let mut args = vec!["--path", "../../../outside_file.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

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
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn malformed_package_handling() {
    let workspace = create_edge_cases_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test that valid package still works even with malformed package in same workspace
    let mut args = vec!["--path", "valid_package/src/lib.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should handle valid package despite malformed neighbor: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
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
    .unwrap();

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
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
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
    .unwrap();

    // Should use --workspace flag for workspace-scope files
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("Error:") || stderr.contains("could not find"),
            "Tool should succeed with workspace scope: {stderr}"
        );
    }
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn cargo_invocation_style_with_detect_package_arg() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Simulate how cargo invokes subcommands: `cargo-detect-package detect-package --path ...`
    // The first argument after the program name is "detect-package" which should be skipped
    let mut args = vec![
        "detect-package", // This is what cargo passes as first arg
        "--path",
        "package_a/src/lib.rs",
        "--via-env",
        "DETECTED_PKG",
    ];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should succeed when invoked cargo-style: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn help_flag_early_exit() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    let output = run_tool(workspace_root, &["--help"]).unwrap();

    assert!(
        output.status.success(),
        "Tool should succeed with --help: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage") || stdout.contains("path"),
        "Help output should contain usage information: {stdout}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn missing_required_args_early_exit() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    // Run without required --path argument
    let output = run_tool(workspace_root, &[]).unwrap();

    assert!(
        !output.status.success(),
        "Tool should fail when required args are missing"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Required") || stdout.contains("path"),
        "Error output should mention required arguments: {stdout}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn missing_subcommand_error() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    // Provide path but no subcommand to execute
    let output = run_tool(workspace_root, &["--path", "package_a/src/lib.rs"]).unwrap();

    assert!(
        !output.status.success(),
        "Tool should fail when no subcommand is provided"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No subcommand") || stderr.contains("subcommand"),
        "Error should mention missing subcommand: {stderr}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn outside_package_action_ignore() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test --outside-package ignore with a root-level file
    let mut args = vec![
        "--path",
        "README.md",
        "--outside-package",
        "ignore",
        "--via-env",
        "PKG",
    ];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        output.status.success(),
        "Tool should succeed with --outside-package ignore: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ignoring"),
        "Output should indicate ignoring: {stdout}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn outside_package_action_error() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Test --outside-package error with a root-level file
    let mut args = vec![
        "--path",
        "README.md",
        "--outside-package",
        "error",
        "--via-env",
        "PKG",
    ];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        !output.status.success(),
        "Tool should fail with --outside-package error for root-level file"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not in any package"),
        "Error should indicate file is not in a package: {stderr}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn directory_path_instead_of_file() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Pass a directory instead of a file
    let mut args = vec!["--path", "package_a/src", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    // Tool should handle directories - it should detect the package
    assert!(
        output.status.success(),
        "Tool should handle directory paths: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn execution_outside_any_cargo_workspace() {
    // Create a temp directory that is NOT a Cargo workspace
    let temp_dir = tempfile::tempdir().unwrap();
    let non_workspace_root = temp_dir.path();

    // Create a simple file in the non-workspace
    fs::write(non_workspace_root.join("some_file.rs"), "// test\n").unwrap();

    let test_cmd = get_test_command();
    let mut args = vec!["--path", "some_file.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(non_workspace_root, &args).unwrap();

    assert!(
        !output.status.success(),
        "Tool should fail when run outside a Cargo workspace"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not within a Cargo workspace"),
        "Error should indicate not in a workspace: {stderr}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn subcommand_with_double_dash_separator() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    // Test clippy-style command with "--" separator
    // The package flags should be inserted before "--"
    let output = run_tool(
        workspace_root,
        &[
            "--path",
            "package_a/src/lib.rs",
            "clippy",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )
    .unwrap();

    // This may fail due to missing dependencies, but it should not fail with our tool's error
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Error detecting package")
            && !stderr.contains("No subcommand")
            && !stderr.contains("not within a Cargo workspace"),
        "Tool should properly handle -- separator: {stderr}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn invalid_toml_package_detection_error() {
    let workspace = create_edge_cases_workspace();
    let workspace_root = workspace.path();
    let test_cmd = get_test_command();

    // Create a file inside the malformed_package to trigger parsing that Cargo.toml
    fs::create_dir_all(workspace_root.join("malformed_package/src")).unwrap();
    fs::write(
        workspace_root.join("malformed_package/src/lib.rs"),
        "// test\n",
    )
    .unwrap();

    let mut args = vec!["--path", "malformed_package/src/lib.rs", "--via-env", "PKG"];
    args.extend_from_slice(&test_cmd);

    let output = run_tool(workspace_root, &args).unwrap();

    assert!(
        !output.status.success(),
        "Tool should fail when package has invalid TOML"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // The TOML parse error may surface during workspace validation or package detection
    assert!(
        stderr.contains("TOML parse error") || stderr.contains("Error detecting package"),
        "Error should indicate TOML parsing failure: {stderr}"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn nonexistent_executable_error() {
    let workspace = create_simple_workspace();
    let workspace_root = workspace.path();

    // Use a non-existent executable as the subcommand
    let output = run_tool(
        workspace_root,
        &[
            "--path",
            "package_a/src/lib.rs",
            "--via-env",
            "PKG",
            "nonexistent-binary-xyz-123456",
        ],
    )
    .unwrap();

    assert!(
        !output.status.success(),
        "Tool should fail when executable does not exist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error executing command"),
        "Error should indicate command execution failure: {stderr}"
    );
}
