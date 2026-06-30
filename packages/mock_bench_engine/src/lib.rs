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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
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
    static PATH: LazyLock<String> = LazyLock::new(|| {
        let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        resolve(std::env::var_os("MOCK_BENCH_ENGINE"), || {
            run_cargo_build(&manifest_path)
        })
    });
    PATH.as_str()
}

/// The outcome of spawning `cargo build`, reduced to the fields the resolver inspects.
///
/// Modeling it as a plain struct rather than [`std::process::Output`] keeps
/// [`interpret_build`] unit-testable: an `ExitStatus` cannot be constructed portably,
/// but these fields can be filled in directly to exercise every branch without
/// spawning a real process.
struct BuildOutput {
    /// Whether the build process exited successfully.
    success: bool,
    /// The process exit code, if it terminated normally with one.
    code: Option<i32>,
    /// The captured standard output (Cargo's JSON build messages).
    stdout: Vec<u8>,
    /// The captured standard error (Cargo's rendered diagnostics).
    stderr: Vec<u8>,
}

/// Resolves the mock-engine binary path: trust an existing file named by
/// `env_override`, otherwise build the crate via `build` and read the path Cargo
/// reports for the binary artifact.
fn resolve(env_override: Option<OsString>, build: impl FnOnce() -> BuildOutput) -> String {
    if let Some(path) = env_override {
        let path = PathBuf::from(path);
        if path.is_file() {
            return path.to_string_lossy().into_owned();
        }
    }

    interpret_build(&build())
}

/// Spawns `cargo build` for this crate and captures its outcome.
fn run_cargo_build(manifest_path: &Path) -> BuildOutput {
    // `--manifest-path` (absolute, derived from this crate's own manifest directory)
    // makes the build independent of the current working directory, which some tests
    // change before first touching the engine. `--locked` matches the
    // `just _mock-engine-path` recipe and refuses to silently rewrite the lockfile.
    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "build",
            "--locked",
            "--message-format=json-render-diagnostics",
            "--manifest-path",
        ])
        .arg(manifest_path)
        .output()
        .expect("spawning `cargo build` for mock_bench_engine should succeed");
    BuildOutput {
        success: output.status.success(),
        code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

/// Reads the built binary's path from Cargo's JSON build output, panicking with the
/// captured diagnostics (exit code, stdout, and stderr) when the build failed or
/// reported no executable.
fn interpret_build(output: &BuildOutput) -> String {
    assert!(
        output.success,
        "building mock_bench_engine failed (exit code: {}):\nstdout:\n{}\nstderr:\n{}",
        output
            .code
            .map_or_else(|| "unknown".to_owned(), |code| code.to_string()),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout =
        std::str::from_utf8(&output.stdout).expect("cargo build JSON output should be valid UTF-8");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A single `compiler-artifact` JSON line for the named target with the given
    /// executable path, matching the shape Cargo emits.
    fn artifact_line(target_name: &str, executable: &str) -> String {
        serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": target_name },
            "executable": executable,
        })
        .to_string()
    }

    /// A successful build whose stdout is `stdout`.
    fn ok_build(stdout: String) -> BuildOutput {
        BuildOutput {
            success: true,
            code: Some(0),
            stdout: stdout.into_bytes(),
            stderr: Vec::new(),
        }
    }

    /// A failed build carrying a known exit code and stderr, for the failure-path
    /// assertions.
    fn failed_build() -> BuildOutput {
        BuildOutput {
            success: false,
            code: Some(101),
            stdout: b"some compiler chatter".to_vec(),
            stderr: b"error: linker exploded".to_vec(),
        }
    }

    #[test]
    fn picks_the_executable_of_the_bin_artifact() {
        let resolved = interpret_build(&ok_build(artifact_line(
            "mock_bench_engine",
            "/tmp/mock_bench_engine",
        )));
        assert_eq!(resolved, "/tmp/mock_bench_engine");
    }

    #[test]
    fn ignores_non_json_lines_other_packages_and_the_lib_artifact() {
        // The lib artifact has no executable; an unrelated package and a stray
        // non-JSON line must both be skipped before the mock bin is found.
        let lib_artifact = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": "mock_bench_engine" },
            "executable": serde_json::Value::Null,
        })
        .to_string();
        let stdout = format!(
            "{}\n{lib_artifact}\nthis is not json\n{}\n",
            artifact_line("serde_json", "/tmp/serde_json"),
            artifact_line("mock_bench_engine", "/tmp/mock_bench_engine"),
        );

        assert_eq!(interpret_build(&ok_build(stdout)), "/tmp/mock_bench_engine");
    }

    #[test]
    #[should_panic(expected = "did not report an executable path")]
    fn panics_when_no_executable_is_reported() {
        let lib_only = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": "mock_bench_engine" },
            "executable": serde_json::Value::Null,
        })
        .to_string();
        drop(interpret_build(&ok_build(lib_only)));
    }

    #[test]
    #[should_panic(expected = "exit code: 101")]
    fn build_failure_reports_the_exit_code() {
        drop(interpret_build(&failed_build()));
    }

    #[test]
    #[should_panic(expected = "error: linker exploded")]
    fn build_failure_reports_stderr() {
        drop(interpret_build(&failed_build()));
    }

    #[test]
    #[should_panic(expected = "exit code: unknown")]
    fn build_failure_without_an_exit_code_reports_unknown() {
        let signalled = BuildOutput {
            success: false,
            code: None,
            stdout: Vec::new(),
            stderr: b"killed by signal".to_vec(),
        };
        drop(interpret_build(&signalled));
    }

    #[test]
    #[should_panic(expected = "valid UTF-8")]
    fn panics_on_non_utf8_output() {
        let garbled = BuildOutput {
            success: true,
            code: Some(0),
            stdout: vec![0xff, 0xfe, 0xfd],
            stderr: Vec::new(),
        };
        drop(interpret_build(&garbled));
    }

    #[test]
    fn no_env_override_falls_through_to_building() {
        // `None` skips the file check entirely, so this exercises `resolve`'s build
        // path without touching the filesystem (and stays Miri-safe).
        let resolved = resolve(None, || {
            ok_build(artifact_line("mock_bench_engine", "/tmp/mock_bench_engine"))
        });
        assert_eq!(resolved, "/tmp/mock_bench_engine");
    }

    // The remaining cases consult the filesystem (`Path::is_file`), which Miri's
    // isolation does not support, so they are native-only.

    #[cfg(not(miri))]
    #[test]
    fn env_override_naming_an_existing_file_is_used_verbatim() {
        // This crate's own manifest is a file that is guaranteed to exist.
        let existing = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let resolved = resolve(Some(existing.clone().into_os_string()), || {
            panic!("must not build when the env override names an existing file")
        });
        assert_eq!(resolved, existing.to_string_lossy());
    }

    #[cfg(not(miri))]
    #[test]
    fn env_override_naming_a_missing_file_falls_through_to_building() {
        let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("does-not-exist.invalid");
        let resolved = resolve(Some(missing.into_os_string()), || {
            ok_build(artifact_line("mock_bench_engine", "/tmp/mock_bench_engine"))
        });
        assert_eq!(resolved, "/tmp/mock_bench_engine");
    }
}
