use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

/// Returns a stable absolute path to the faker binary.
///
/// Resolves once per process. On-demand builds retain their executable until the
/// target tree is cleaned, independently of subsequent faker builds.
/// A test runner may supply an existing file through `CBH_FAKER` to skip building.
/// The runner must keep that file available for the lifetime of its consumers.
///
/// # Panics
///
/// Panics if preparing or building the executable fails.
#[must_use]
pub fn binary_path() -> &'static str {
    static PATH: LazyLock<String> = LazyLock::new(locate_or_build);
    PATH.as_str()
}

/// Supplies the process environment to the cached locator.
fn locate_or_build() -> String {
    prepare_binary(
        std::env::var_os("CBH_FAKER"),
        &engine_target_dir(std::env::var_os("CARGO_TARGET_DIR")),
    )
}

/// Prepares an executable for test runners with an explicit override and cache.
///
/// An existing override is returned as an absolute path; the caller owns its
/// lifetime. Otherwise, builds share intermediate artifacts under `target_dir`
/// but retain separate executables until that directory is cleaned.
///
/// # Panics
///
/// Panics if preparing or building the executable fails.
#[must_use]
pub fn prepare_binary(env_override: Option<OsString>, target_dir: &Path) -> String {
    if let Some(path) = env_override {
        let path = PathBuf::from(path);
        if path.is_file() {
            // `binary_path` promises an absolute path, but the override may be relative.
            // Resolve it to absolute now — while the working directory is still the one
            // the override was written against — so the cached value keeps naming the
            // same file after a later directory change (the faker honors `--chdir` and the
            // integration harness drives commands from various directories). The build
            // path Cargo reports is already absolute, so this keeps both resolution paths
            // consistent. `std::path::absolute` is preferred over `canonicalize` to avoid
            // the Windows `\\?\` verbatim prefix, which need not appear in spawn argv or logs.
            // Failing loudly beats silently caching a relative path that would break the
            // absolute-path contract after the first `--chdir`.
            return std::path::absolute(&path)
                .unwrap_or_else(|error| {
                    panic!(
                        "CBH_FAKER={} could not be resolved to an absolute path: {error}",
                        path.display()
                    )
                })
                .to_string_lossy()
                .into_owned();
        }
    }

    let target_dir = std::path::absolute(target_dir)
        .unwrap_or_else(|error| panic!("cannot resolve faker target directory: {error}"));
    fs::create_dir_all(&target_dir)
        .unwrap_or_else(|error| panic!("cannot create faker target directory: {error}"));
    // Cargo's final artifacts belong to this preparation, not to the shared build
    // cache. A later build may replace cache entries but cannot unlink this path.
    // Ref: docs/implementation.md, "Executable ownership".
    let output_dir = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(&target_dir)
        .unwrap_or_else(|error| panic!("cannot reserve faker output directory: {error}"));
    let executable = interpret_build(&run_cargo_build(
        output_dir.path(),
        &target_dir.join("build"),
    ));

    // Consumers keep only the path, including across processes via CBH_FAKER.
    // Retain the directory until explicit target-tree cleanup, not process exit.
    _ = output_dir.keep();
    executable
}

/// Spawns `cargo build` for this crate and captures its outcome.
fn run_cargo_build(output_dir: &Path, build_dir: &Path) -> BuildOutput {
    // `--manifest-path` (absolute, derived from this crate's own manifest directory)
    // makes the build independent of the current working directory, which some tests
    // change before first touching the engine. `--locked` matches the
    // `just _faker-path` recipe and refuses to silently rewrite the lockfile.
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "build",
            "--locked",
            "--bin",
            "cargo-bench-history-faker",
            "--message-format=json-render-diagnostics",
            "--manifest-path",
        ])
        .arg(&manifest_path)
        .arg("--target-dir")
        .arg(output_dir)
        .env("CARGO_BUILD_BUILD_DIR", build_dir)
        .output()
        .unwrap_or_else(|error| panic!("cannot spawn cargo build for faker: {error}"));
    BuildOutput {
        success: output.status.success(),
        code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

/// Anchors the faker cache and retained executables within the active target tree.
///
/// It nests under the active target directory — the ambient `CARGO_TARGET_DIR` when one is
/// set (as coverage runs do), otherwise the workspace `target/` — so it stays git-ignored,
/// is cached in CI alongside everything else, and is removed by `cargo clean`. The
/// `just _faker-path` recipe uses the same cache and separate output directories. The
/// ambient value is taken as a parameter (rather than read here) so the mapping stays
/// unit-testable without mutating process-wide environment.
///
/// A *relative* ambient `CARGO_TARGET_DIR` is absolutized against the workspace root rather
/// than the process working directory. The result is handed to `cargo build` as
/// `--target-dir`, and some callers change the working directory before first touching the
/// engine, so a cwd-relative value would otherwise send the build to an unexpected tree —
/// the same reason the harvester absolutizes `CARGO_TARGET_DIR` (see
/// `cargo-bench-history`'s `commands::collect::target_root_from`).
fn engine_target_dir(ambient_target_dir: Option<OsString>) -> PathBuf {
    // The workspace root, two levels above this crate's own manifest
    // (`<root>/packages/cargo-bench-history-faker`).
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let base = ambient_target_dir.map_or_else(
        || workspace_root.join("target"),
        |configured| {
            let configured = PathBuf::from(configured);
            if configured.is_absolute() {
                configured
            } else {
                workspace_root.join(configured)
            }
        },
    );
    base.join("faker")
}

/// Cargo's outcome as plain values, for in-process diagnostic and JSON tests.
struct BuildOutput {
    success: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Reads the built binary's path from Cargo's JSON build output, panicking with the
/// captured diagnostics (exit code, stdout, and stderr) when the build failed or
/// reported no executable.
fn interpret_build(output: &BuildOutput) -> String {
    assert!(
        output.success,
        "building cargo-bench-history-faker failed (exit code: {}):\nstdout:\n{}\nstderr:\n{}",
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
        let is_faker = message
            .get("target")
            .and_then(|target| target.get("name"))
            .and_then(serde_json::Value::as_str)
            == Some("cargo-bench-history-faker");
        if is_artifact
            && is_faker
            && let Some(executable) = message
                .get("executable")
                .and_then(serde_json::Value::as_str)
        {
            return executable.to_owned();
        }
    }
    panic!("cargo build did not report an executable path for cargo-bench-history-faker");
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
            "cargo-bench-history-faker",
            "/tmp/cargo-bench-history-faker",
        )));
        assert_eq!(resolved, "/tmp/cargo-bench-history-faker");
    }

    #[test]
    fn ignores_non_json_lines_other_packages_and_the_lib_artifact() {
        // The lib artifact has no executable; an unrelated package and a stray
        // non-JSON line must both be skipped before the faker bin is found.
        let lib_artifact = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": "cargo-bench-history-faker" },
            "executable": serde_json::Value::Null,
        })
        .to_string();
        let stdout = format!(
            "{}\n{lib_artifact}\nthis is not json\n{}\n",
            artifact_line("serde_json", "/tmp/serde_json"),
            artifact_line(
                "cargo-bench-history-faker",
                "/tmp/cargo-bench-history-faker"
            ),
        );

        assert_eq!(
            interpret_build(&ok_build(stdout)),
            "/tmp/cargo-bench-history-faker"
        );
    }

    #[test]
    #[should_panic(expected = "did not report an executable path")]
    fn panics_when_no_executable_is_reported() {
        let lib_only = serde_json::json!({
            "reason": "compiler-artifact",
            "target": { "name": "cargo-bench-history-faker" },
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
    fn engine_target_dir_uses_an_absolute_ambient_target_dir_as_is() {
        // An absolute `CARGO_TARGET_DIR` (the normal case, e.g. a coverage run) is honored
        // verbatim as the base, so the isolated tree stays inside that tree.
        let ambient = Path::new(env!("CARGO_MANIFEST_DIR")).join("custom-target");
        assert!(ambient.is_absolute());
        let dir = engine_target_dir(Some(ambient.clone().into_os_string()));
        assert_eq!(dir, ambient.join("faker"));
    }

    #[test]
    fn engine_target_dir_absolutizes_a_relative_ambient_target_dir_against_the_workspace() {
        // A relative `CARGO_TARGET_DIR` is resolved against the workspace root, not the
        // process working directory, so a later `--chdir` cannot redirect the build.
        let dir = engine_target_dir(Some(OsString::from("rel-target")));
        let expected = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("rel-target")
            .join("faker");
        assert_eq!(dir, expected);
    }

    #[test]
    fn engine_target_dir_defaults_under_the_workspace_target() {
        // With no override it anchors at the workspace `target/`, addressed absolutely from
        // this crate's manifest so it does not depend on the current working directory.
        let dir = engine_target_dir(None);
        let expected = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("faker");
        assert_eq!(dir, expected);
        assert!(
            dir.is_absolute(),
            "the default target dir should be absolute: {dir:?}"
        );
    }
}
