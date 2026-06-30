//! Test-support crate providing a fake benchmark engine for the `cargo-bench-history`
//! integration tests.
//!
//! The fake engine itself is this crate's **binary** (`src/main.rs`); it writes
//! benchmark-engine output files into the target tree and exits with a caller-chosen
//! code, standing in for a real engine. This **library** exists only to let the
//! consuming tests locate that binary: Cargo exposes `CARGO_BIN_EXE_*` only to the
//! integration tests of the package that owns the binary, so `cargo-bench-history`'s
//! tests cannot reference it that way. Instead they take a normal `dev-dependency` on
//! this crate and call [`binary_path`], which builds the binary on demand and reports
//! its path.
//!
//! The whole crate is `publish = false`, so neither the binary nor this locator ever
//! ships, and `cargo install cargo-bench-history` places only the real tool on a
//! user's PATH (issue #289).

use std::path::Path;
use std::sync::LazyLock;

/// Returns the absolute path to the freshly built `mock_bench_engine` binary.
///
/// The binary is built on demand (a fast no-op once it is up to date) and rebuilt
/// automatically after edits, so the path is always present and current; it is
/// resolved once per process. A test runner that has already built the binary may
/// point the `MOCK_BENCH_ENGINE` environment variable at it to skip the per-process
/// build — the value is trusted only when it names an existing file.
#[must_use]
pub fn binary_path() -> &'static str {
    static PATH: LazyLock<String> = LazyLock::new(resolve);
    PATH.as_str()
}

/// Builds this crate's binary and returns the absolute path to it, as reported by
/// Cargo's JSON build messages.
fn resolve() -> String {
    if let Some(path) = std::env::var_os("MOCK_BENCH_ENGINE") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return path.to_string_lossy().into_owned();
        }
    }

    // `--manifest-path` (absolute, derived from this crate's own manifest directory)
    // makes the build independent of the current working directory, which some tests
    // change before first touching the engine.
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "--message-format=json-render-diagnostics",
            "--manifest-path",
        ])
        .arg(&manifest_path)
        .output()
        .expect("spawning `cargo build` for mock_bench_engine should succeed");
    assert!(
        output.status.success(),
        "building mock_bench_engine failed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout =
        String::from_utf8(output.stdout).expect("cargo build JSON output should be valid UTF-8");
    for line in stdout.lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let is_artifact =
            message.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact");
        let is_mock = message
            .get("target")
            .and_then(|target| target.get("name"))
            .and_then(serde_json::Value::as_str)
            == Some("mock_bench_engine");
        if is_artifact
            && is_mock
            && let Some(executable) = message
                .get("executable")
                .and_then(serde_json::Value::as_str)
        {
            return executable.to_owned();
        }
    }
    panic!("cargo build did not report an executable path for mock_bench_engine");
}
