//! Captured build inputs selected outside conventional member source directories.

use std::fs;

use cargo_release_plan::{RunInput, run};
use crp_versioning::preview::Prepared;
use serde_json::Value;
use std::path::Path;

use crate::fixture::write_package;
use crate::harness::seeded_package;

#[test]
#[cfg_attr(
    miri,
    ignore = "captures a discovered dependency's explicitly selected build script"
)]
fn dependency_explicit_build_file_is_captured() {
    let fixture = seeded_package();
    fixture.write_workspace("exclude=['helper']");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "[dependencies]\nhelper={path='../../helper'}",
    );
    fixture.write(".gitignore", "evidence/\n");
    fixture.write(
        "helper/Cargo.toml",
        "[package]\nname='helper'\nversion='0.1.0'\nedition='2024'\nbuild='../evidence/diffs/build.rs'\n[workspace]\n",
    );
    fixture.write("helper/src/lib.rs", "pub fn helper() {}\n");
    fixture.write("evidence/diffs/build.rs", "fn main() {}\n");
    let metadata: Value =
        serde_json::from_str(&fixture.cargo(&["metadata", "--offline", "--format-version", "1"]))
            .unwrap();
    let helper = metadata
        .get("packages")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package.get("name").unwrap() == "helper")
        .unwrap();
    assert!(
        helper
            .get("targets")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|target| {
                target
                    .get("kind")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|kind| kind == "custom-build")
            })
    );
    fixture.commit("select dependency build");
    let output = fixture.path().join("prepared-evidence");
    run(&RunInput::Prepare {
        output: output.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let prepared: Prepared =
        serde_json::from_slice(&fs::read(output.join("prepared.json")).unwrap()).unwrap();
    assert!(
        prepared
            .inputs
            .paths
            .contains(Path::new("evidence/diffs/build.rs"))
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "retains and verifies Cargo's implicit nonmember build-script input"
)]
fn dependency_default_build_script_is_captured_and_verified() {
    let fixture = seeded_package();
    fixture.write_workspace("exclude=['helper']");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "[dependencies]\nhelper={path='../../helper'}",
    );
    fixture.write(".gitignore", "helper/build.rs\n");
    fixture.write(
        "helper/Cargo.toml",
        "[package]\nname='helper'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
    );
    fixture.write("helper/src/lib.rs", "pub fn helper() {}\n");
    fixture.write("helper/build.rs", "fn main() {}\n");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("select conventional dependency build");
    let prepared = fixture.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fixture.write(
        "proposal.json",
        r#"{"schema_version":6,"increments":[{"name":"demo","bump":"minor"}]}"#,
    );
    let preview = fixture.path().join("preview");
    run(&RunInput::Preview {
        plan: fixture.path().join("proposal.json"),
        prepared: prepared.join("prepared.json"),
        output: preview.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert_eq!(
        fs::read_to_string(preview.join("workspace/helper/build.rs")).unwrap(),
        "fn main() {}\n",
    );
    fixture.write("helper/build.rs", "fn main() { println!(\"changed\"); }\n");
    let rejected = fixture.path().join("rejected");
    run(&RunInput::Preview {
        plan: fixture.path().join("proposal.json"),
        prepared: prepared.join("prepared.json"),
        output: rejected.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!rejected.exists());
}
