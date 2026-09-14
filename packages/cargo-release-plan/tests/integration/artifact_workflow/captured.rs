//! Captured-input rejection before preparing a resolution workspace.

use cargo_release_plan::{RunInput, run};

use crate::fixture::write_package;
use crate::harness::seeded_package;

#[test]
#[cfg_attr(
    miri,
    ignore = "captures a real Cargo workspace with an absolute local dependency"
)]
fn preparation_rejects_absolute_paths_before_creating_a_resolution_workspace() {
    let fixture = seeded_package();
    write_package(&fixture, "core", "0.1.0", "");
    fixture.commit("local dependency target");
    let manifest = format!(
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
         [dependencies]\ncore = {{ version = \"0.1.0\", path = {:?} }}\n",
        fixture.path().join("packages/core")
    );
    fixture.write("packages/demo/Cargo.toml", &manifest);
    run(&RunInput::Prepare {
        output: fixture.path().join("prepared"),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!fixture.path().join("prepared/.prospective").exists());
    assert_eq!(fixture.read("packages/demo/Cargo.toml"), manifest);
    assert!(!fixture.path().join("Cargo.lock").exists());
}
