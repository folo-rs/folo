//! Captured Git membership and standalone preview completion boundaries.

use std::fs;

use cargo_release_plan::{RunInput, run};
use tempfile::tempdir;

use crate::harness::seeded_package;

#[test]
#[cfg_attr(
    miri,
    ignore = "reserves an owned prospective directory through preparation"
)]
fn preparation_preserves_an_occupied_prospective_directory() {
    let fixture = seeded_package();
    fixture.write("prepared/.prospective/keep", "another owner");
    run(&RunInput::Prepare {
        output: fixture.path().join("prepared"),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fixture.read("prepared/.prospective/keep"), "another owner");
    assert!(!fixture.path().join("prepared/prepared.json").exists());
    assert!(!fixture.path().join("Cargo.lock").exists());
}

#[test]
#[cfg_attr(miri, ignore = "uses owned preview artifact files")]
fn preview_output_cannot_destroy_an_input_document() {
    let directory = tempdir().unwrap();
    let plan = directory.path().join("plan.json");
    let before = r#"{"schema_version":4,"increments":[]}"#;
    fs::write(&plan, before).unwrap();
    // Collision checks precede reads, so neither a repository nor prepared evidence is needed.
    run(&RunInput::Preview {
        plan: plan.clone(),
        prepared: directory.path().join("absent-prepared.json"),
        output: directory.path().to_owned(),
        manifest_path: directory.path().join("absent-Cargo.toml"),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fs::read_to_string(plan).unwrap(), before);
}

#[test]
#[cfg_attr(miri, ignore = "uses real offline Cargo with an empty local source")]
fn offline_resolution_failure_never_becomes_prepared_evidence() {
    let fixture = seeded_package();
    fixture.write(
        ".cargo/config.toml",
        "[source.crates-io]\nreplace-with = \"local\"\n[source.local]\ndirectory = \"vendor\"\n",
    );
    fixture.write("vendor/.keep", "");
    fixture.git(&["add", ".cargo/config.toml", "vendor/.keep"]);
    let manifest = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                    [dependencies]\nfixture_only_missing_dependency = \"1.0.0\"\n";
    fixture.write("packages/demo/Cargo.toml", manifest);
    run(&RunInput::Prepare {
        output: fixture.path().join("prepared"),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!fixture.path().join("prepared/prepared.json").exists());
    assert!(!fixture.path().join("prepared/.prospective").exists());
    assert!(!fixture.path().join("Cargo.lock").exists());
    assert_eq!(fixture.read("packages/demo/Cargo.toml"), manifest);
}
