//! Filesystem validation of Cargo-reported artifacts is independent of Cargo's exit status.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Output;
use std::{env, fs};

use serde_json::{Value, json};

use crate::{Fixture, SMOKE_WATCHDOG, assert_success, compile_tool};

#[test]
fn rejects_non_executable_artifacts_and_relative_target_directories() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let fixture = Fixture::new();
        let tools = fixture.root.path().join("cargo-fixture");
        // Supply the Cargo protocol directly so invalid filesystem reports can be tested
        // without racing Cargo's compiler or changing production code in test builds.
        compile_tool(
            &tools,
            "cargo",
            r#"
use std::env;

fn main() {
    let variable = match env::args().nth(1).unwrap().as_str() {
        "metadata" => "RELEASE_FIXTURE_METADATA",
        "build" => "RELEASE_FIXTURE_ARTIFACT",
        _ => panic!("unexpected Cargo operation"),
    };
    println!("{}", env::var(variable).unwrap());
}
"#,
        );
        let path = env::join_paths(
            std::iter::once(tools).chain(env::split_paths(&env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let executable = fixture.root.path().join("reported-artifact");
        fs::write(&executable, "fixture artifact").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let mut metadata = json!({
            "target_directory": fixture.root.path().join("target"),
            "workspace_members": ["fixture-alpha"],
            "packages": [{
                "id": "fixture-alpha", "name": "alpha", "version": "1.0.0",
                "targets": [{"name": "alpha-bin", "kind": ["bin"]}],
            }],
        });
        let artifact = json!({
            "reason": "compiler-artifact", "package_id": "fixture-alpha",
            "target": {"name": "alpha-bin", "kind": ["bin"]}, "executable": executable,
        });
        let execute = |metadata: &Value, output: &str| {
            fixture
                .batch_command(&json!([fixture.binary("alpha")]), output)
                .arg("--no-upload")
                .env("PATH", &path)
                .env("RELEASE_FIXTURE_METADATA", metadata.to_string())
                .env("RELEASE_FIXTURE_ARTIFACT", artifact.to_string())
                .output()
                .unwrap()
        };
        assert_success(&execute(&metadata, "out/valid"));

        #[cfg(unix)]
        {
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
            assert_failed_build(
                &fixture,
                &execute(&metadata, "out/non-executable"),
                "non-executable",
            );
        }
        fs::remove_file(&executable).unwrap();
        fs::create_dir_all(&executable).unwrap();
        assert_failed_build(&fixture, &execute(&metadata, "out/directory"), "directory");

        metadata["target_directory"] = "relative-target".into();
        let result = execute(&metadata, "out/relative-target");
        assert!(!result.status.success());
        assert!(
            !fixture
                .root
                .path()
                .join("out/relative-target/outcomes.json")
                .exists()
        );
    });
}

fn assert_failed_build(fixture: &Fixture, result: &Output, name: &str) {
    assert!(!result.status.success());
    let output = fixture.root.path().join("out").join(name);
    let outcomes: Value =
        serde_json::from_slice(&fs::read(output.join("outcomes.json")).unwrap()).unwrap();
    assert_eq!(outcomes.as_array().unwrap().len(), 1);
    assert_eq!(outcomes[0]["status"], "failed");
    assert_eq!(outcomes[0]["stage"], "build");
    let base = format!("alpha-v1.0.0-{}", fixture.triple);
    assert!(!output.join(&base).join(format!("{base}.zip")).exists());
}
