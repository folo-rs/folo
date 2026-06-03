//! Integration tests for cargo-freeze-deps.
//!
//! These tests exercise the public `run()` entry point against temporary on-disk
//! Cargo.toml files, covering the path arg defaults, the `--output` option, and end-to-end
//! error reporting.
//!
//! Tests that depend on the process current directory are marked `#[serial]` per the
//! workspace convention. They also use a CWD-guard RAII helper so the current directory
//! is restored even when an assertion fails mid-test.

use std::fs;
use std::path::{Path, PathBuf};

use cargo_freeze_deps::{RunError, RunInput, run};
use serial_test::serial;

/// RAII helper: restores the process current directory when dropped, even on panic.
struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn enter(target: &Path) -> Self {
        let original = std::env::current_dir().expect("current_dir should be readable");
        std::env::set_current_dir(target).expect("set_current_dir to test target should succeed");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        // Best-effort restoration; if this fails we are likely already unwinding from
        // another panic and there is nothing useful to do.
        _ = std::env::set_current_dir(&self.original);
    }
}

fn write_cargo_toml(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("Cargo.toml");
    fs::write(&path, content).unwrap();
    path
}

// -- Default in-place rewriting -------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)] // OS interactions exceed Miri emulation capabilities.
fn rewrites_in_place_when_no_output_specified() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = write_cargo_toml(
        temp_dir.path(),
        r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
serde = "1.2.3"
tokio = "^1.48"
maybe_pinned = "<2.0.0"
"#,
    );

    let outcome = run(&RunInput {
        path: path.clone(),
        output: None,
    })
    .unwrap();

    assert_eq!(outcome.frozen_count, 2);
    assert_eq!(outcome.skipped_count, 1);

    let rewritten = fs::read_to_string(&path).unwrap();
    assert!(rewritten.contains(r#"serde = "=1.2.3""#));
    assert!(rewritten.contains(r#"tokio = "=1.48.0""#));
    assert!(rewritten.contains(r#"maybe_pinned = "<2.0.0""#));
}

#[test]
#[cfg_attr(miri, ignore)]
fn writes_to_separate_output_path_and_leaves_input_untouched() {
    let temp_dir = tempfile::tempdir().unwrap();
    let input_content = r#"[dependencies]
serde = "1.2.3"
"#;
    let input_path = write_cargo_toml(temp_dir.path(), input_content);
    let output_path = temp_dir.path().join("Cargo.frozen.toml");

    let outcome = run(&RunInput {
        path: input_path.clone(),
        output: Some(output_path.clone()),
    })
    .unwrap();

    assert_eq!(outcome.frozen_count, 1);

    let input_after = fs::read_to_string(&input_path).unwrap();
    let output_after = fs::read_to_string(&output_path).unwrap();
    assert_eq!(input_after, input_content);
    assert!(output_after.contains(r#"serde = "=1.2.3""#));
}

#[test]
#[serial] // Reads ./Cargo.toml relative to the process current directory.
#[cfg_attr(miri, ignore)]
fn defaults_to_cargo_toml_in_current_directory() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_cargo_toml(
        temp_dir.path(),
        r#"[dependencies]
serde = "1.2.3"
"#,
    );

    let _guard = CurrentDirGuard::enter(temp_dir.path());

    let outcome = run(&RunInput {
        path: PathBuf::from("Cargo.toml"),
        output: None,
    })
    .unwrap();

    assert_eq!(outcome.frozen_count, 1);
    let rewritten = fs::read_to_string(temp_dir.path().join("Cargo.toml")).unwrap();
    assert!(rewritten.contains(r#"serde = "=1.2.3""#));
}

// -- Workspace-level and package-level files --------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn workspace_level_cargo_toml_processed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = write_cargo_toml(
        temp_dir.path(),
        r#"[workspace]
members = ["pkg"]

[workspace.dependencies]
serde = "1.2.3"
tokio = { version = "1.48.0", features = ["full"] }
"#,
    );

    let outcome = run(&RunInput {
        path: path.clone(),
        output: None,
    })
    .unwrap();

    assert_eq!(outcome.frozen_count, 2);
    let rewritten = fs::read_to_string(&path).unwrap();
    assert!(rewritten.contains(r#"serde = "=1.2.3""#));
    assert!(rewritten.contains(r#"version = "=1.48.0""#));
}

#[test]
#[cfg_attr(miri, ignore)]
fn combined_workspace_and_package_cargo_toml_processed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = write_cargo_toml(
        temp_dir.path(),
        r#"[workspace]
members = ["."]

[workspace.dependencies]
shared = "1.0.0"

[package]
name = "root_package"
version = "0.1.0"

[dependencies]
serde = "1.2.3"

[dev-dependencies]
mockall = "0.14.0"

[build-dependencies]
cc = "1.0.0"

[target."cfg(unix)".dependencies]
libc = "0.2.172"
"#,
    );

    let outcome = run(&RunInput {
        path: path.clone(),
        output: None,
    })
    .unwrap();

    assert_eq!(outcome.frozen_count, 5);
    let rewritten = fs::read_to_string(&path).unwrap();
    assert!(rewritten.contains(r#"shared = "=1.0.0""#));
    assert!(rewritten.contains(r#"serde = "=1.2.3""#));
    assert!(rewritten.contains(r#"mockall = "=0.14.0""#));
    assert!(rewritten.contains(r#"cc = "=1.0.0""#));
    assert!(rewritten.contains(r#"libc = "=0.2.172""#));
}

// -- Realistic mixed file ---------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn realistic_file_preserves_comments_and_layout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let input = r#"[package]
name = "my_app"
version = "0.2.0"
edition = "2021"

# Runtime dependencies
[dependencies]
# Standard serialization
serde = { version = "1.2.3", features = ["derive"] }
serde_json = "1.0.0"            # pretty printing matters
tokio = "1.48.0"
# Internal crate, no version freezing needed
internal = { path = "../internal" }
# Pinned to a range; we intentionally leave it alone
custom_range = ">=1.0.0, <2.0.0"

[dev-dependencies]
mockall = "0.14.0"
"#;
    let path = write_cargo_toml(temp_dir.path(), input);

    run(&RunInput {
        path: path.clone(),
        output: None,
    })
    .unwrap();

    let rewritten = fs::read_to_string(&path).unwrap();

    assert!(rewritten.contains("# Runtime dependencies"));
    assert!(rewritten.contains("# Standard serialization"));
    assert!(rewritten.contains("# pretty printing matters"));
    assert!(rewritten.contains("# Internal crate, no version freezing needed"));
    assert!(rewritten.contains("# Pinned to a range; we intentionally leave it alone"));

    assert!(rewritten.contains(r#"version = "=1.2.3""#));
    assert!(rewritten.contains(r#"serde_json = "=1.0.0""#));
    assert!(rewritten.contains(r#"tokio = "=1.48.0""#));
    assert!(rewritten.contains(r#"internal = { path = "../internal" }"#));
    assert!(rewritten.contains(r#"custom_range = ">=1.0.0, <2.0.0""#));
    assert!(rewritten.contains(r#"mockall = "=0.14.0""#));
}

#[test]
#[cfg_attr(miri, ignore)]
fn idempotent_when_run_twice() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = write_cargo_toml(
        temp_dir.path(),
        r#"[dependencies]
serde = "1.2.3"
tokio = "^1.48"
"#,
    );

    run(&RunInput {
        path: path.clone(),
        output: None,
    })
    .unwrap();
    let after_first = fs::read_to_string(&path).unwrap();

    let outcome = run(&RunInput {
        path: path.clone(),
        output: None,
    })
    .unwrap();
    let after_second = fs::read_to_string(&path).unwrap();

    assert_eq!(after_first, after_second);
    assert_eq!(outcome.frozen_count, 0);
    assert_eq!(outcome.skipped_count, 0);
}

// -- Error reporting --------------------------------------------------------------------------

#[test]
#[cfg_attr(miri, ignore)]
fn missing_file_returns_io_error() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing = temp_dir.path().join("does-not-exist.toml");

    match run(&RunInput {
        path: missing,
        output: None,
    })
    .unwrap_err()
    {
        RunError::Io(_) => {}
        other => panic!("expected Io error, got {other:?}"),
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn invalid_toml_returns_parse_error() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = write_cargo_toml(temp_dir.path(), "this is = not [valid toml");

    match run(&RunInput { path, output: None }).unwrap_err() {
        RunError::Parse(_) => {}
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn invalid_version_returns_typed_error_with_dep_name() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = write_cargo_toml(
        temp_dir.path(),
        r#"[dependencies]
serde = "not-a-version"
"#,
    );

    match run(&RunInput { path, output: None }).unwrap_err() {
        RunError::InvalidVersion { dep, version, .. } => {
            assert_eq!(dep, "serde");
            assert_eq!(version, "not-a-version");
        }
        other => panic!("expected InvalidVersion error, got {other:?}"),
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn error_during_freeze_leaves_input_intact() {
    let temp_dir = tempfile::tempdir().unwrap();
    let input = r#"[dependencies]
serde = "garbage-version"
"#;
    let path = write_cargo_toml(temp_dir.path(), input);

    run(&RunInput {
        path: path.clone(),
        output: None,
    })
    .unwrap_err();

    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(after, input, "input file must not be partially rewritten");
}
