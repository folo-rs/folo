//! Resolving the Cargo `target` directory for benchmark output files.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

/// Resolves the Cargo `target` directory.
///
/// Uses the same strategy as Criterion: the `CARGO_TARGET_DIR` environment
/// variable if set, otherwise the `target_directory` reported by
/// `cargo metadata`. When the `CARGO` environment variable is not set (so the
/// path to the `cargo` binary that launched the process is unknown), the
/// `cargo` binary is resolved from `PATH` instead, because `CARGO` is not
/// guaranteed to be present in every runtime environment. Returns `None` if the
/// target directory cannot be determined, in which case callers should fall
/// back to a relative `target` path.
#[cfg_attr(test, mutants::skip)]
// Reads process env and shells out to `cargo metadata`; the JSON parsing is unit-tested separately.
#[cfg_attr(coverage_nightly, coverage(off))]
#[must_use]
pub fn cargo_target_directory() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("CARGO_TARGET_DIR") {
        return Some(PathBuf::from(dir));
    }

    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_metadata_target_directory(&output.stdout)
}

/// Extracts the `target_directory` field from `cargo metadata` JSON output.
fn parse_metadata_target_directory(metadata_json: &[u8]) -> Option<PathBuf> {
    #[derive(Deserialize)]
    struct Metadata {
        target_directory: PathBuf,
    }

    let metadata: Metadata = serde_json::from_slice(metadata_json).ok()?;
    Some(metadata.target_directory)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn parses_target_directory_from_metadata() {
        let json = br#"{
            "packages": [],
            "workspace_root": "/home/user/project",
            "target_directory": "/home/user/project/target",
            "version": 1
        }"#;

        let result = parse_metadata_target_directory(json);
        assert_eq!(result, Some(PathBuf::from("/home/user/project/target")));
    }

    #[test]
    fn parses_target_directory_ignoring_unknown_fields() {
        let json = br#"{"target_directory": "/custom/target", "extra": [1, 2, 3]}"#;

        let result = parse_metadata_target_directory(json);
        assert_eq!(result, Some(PathBuf::from("/custom/target")));
    }

    #[test]
    fn returns_none_for_invalid_json() {
        let result = parse_metadata_target_directory(b"not json at all");
        assert_eq!(result, None);
    }

    #[test]
    fn returns_none_when_target_directory_missing() {
        let json = br#"{"workspace_root": "/home/user/project"}"#;

        let result = parse_metadata_target_directory(json);
        assert_eq!(result, None);
    }
}
