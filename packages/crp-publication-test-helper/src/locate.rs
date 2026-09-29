//! Builds the production-linked provider without assuming a hashed Cargo artifact name.

#![allow(
    clippy::unwrap_used,
    reason = "This locator is used only by integration-test fixtures"
)]

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value;

/// Locates the provider executable used by the real Cargo publication fixture.
#[must_use]
// This adapter discovers the running binary and launches Cargo. Publication boundary tests
// exercise those real-process operations; the artifact selection below is tested in-process.
#[cfg_attr(test, mutants::skip)]
pub fn executable() -> &'static PathBuf {
    static PROVIDER: OnceLock<PathBuf> = OnceLock::new();
    PROVIDER.get_or_init(|| {
        // An absolute target path preserves the original workspace invocation's build location
        // when Cargo runs this locator from a package's working directory.
        let executable = env::current_exe().unwrap();
        let target = executable
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let output = Command::new("cargo")
            .args([
                "build",
                "--offline",
                "--locked",
                "--package",
                "crp-publication-test-helper",
                "--bin",
                "crp-publication-test-helper",
                "--message-format=json",
            ])
            .arg("--target-dir")
            .arg(target)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        executable_path(&output.stdout)
    })
}

fn executable_path(output: &[u8]) -> PathBuf {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .filter_map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap()
                .get("executable")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
        .next_back()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_executable_among_cargo_diagnostics_and_library_artifacts() {
        assert_eq!(
            executable_path(
                concat!(
                    "{\"executable\":\"earlier\"}\n",
                    "{\"executable\":null}\n",
                    "{\"executable\":\"provider\"}\n",
                    "{\"reason\":\"build-finished\",\"success\":true}\n"
                )
                .as_bytes()
            ),
            PathBuf::from("provider")
        );
    }

    #[test]
    fn invalid_or_missing_executable_output_fails() {
        for output in [
            &b""[..],
            &b"not-json"[..],
            &b"\xff"[..],
            &b"{\"executable\":null}"[..],
        ] {
            testing::assert_panics(|| executable_path(output));
        }
    }
}
