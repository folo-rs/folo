//! Exercises legacy argument and JSON boundaries without a release query or a build.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{assert_success, command};

#[test]
fn empty_plan_is_an_explicit_array_but_malformed_input_is_failure() {
    let directory = TempDir::new().unwrap();
    let input = directory.path().join("input.json");
    fs::write(
        &input,
        serde_json::to_vec(&json!({
            "targets": [{"triple": "x86_64-unknown-linux-gnu", "os": "ubuntu-24.04"}],
            "binaries": []
        }))
        .unwrap(),
    )
    .unwrap();
    let execute = || {
        command(directory.path(), env!("CARGO_BIN_EXE_release-binaries"))
            .args(["plan", "--input"])
            .arg(&input)
            .args(["--repository", "fixture/does-not-exist"])
            .output()
            .unwrap()
    };
    let result = execute();
    assert_success(&result);
    let plan: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(plan, json!([]));
    fs::write(&input, "malformed JSON").unwrap();
    let result = execute();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!result.stderr.is_empty());
}
