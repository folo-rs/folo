//! Immutable publication preparation using a local Git transport, never a live forge.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use cargo_release_plan::{RunInput, run};
use serde_json::Value;
use tempfile::TempDir;

use crate::fixture::{Fixture, write_package};

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn prepares_frozen_requests_and_reuses_identical_output_without_changing_source() {
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    run(&input).unwrap();
    let original = fs::read(&path).unwrap();
    let manifest: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(
        manifest.pointer("/publication/source").unwrap(),
        &fixture.sha("HEAD")
    );
    assert_eq!(
        manifest.pointer("/publication/workspace_manifest").unwrap(),
        "Cargo.toml"
    );
    assert_eq!(
        manifest.pointer("/publication/packages/0/name").unwrap(),
        "library"
    );
    assert_eq!(
        manifest.pointer("/publication/packages/0/version").unwrap(),
        "1.0.0"
    );
    assert_eq!(
        manifest
            .pointer("/publication/configuration/release-branch")
            .unwrap(),
        "stable"
    );
    assert_eq!(
        manifest.pointer("/publication/packages/0/binary").unwrap(),
        &Value::Null
    );
    assert!(fixture.git(&["status", "--porcelain"]).is_empty());
    run(&input).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn rejects_dirty_or_untracked_publication_inputs_without_writing_an_artifact() {
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    fixture.write("packages/library/src/lib.rs", "pub fn changed() {}\n");
    run(&input).unwrap_err();
    assert!(!path.exists());
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn branch_movement_does_not_change_intent_for_the_same_source() {
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    let source = fixture.sha("HEAD");
    run(&input).unwrap();
    let original = fs::read(&path).unwrap();
    fixture.write("notes.txt", "unrelated release-branch movement\n");
    fixture.commit("advance release line");
    fixture.git(&["branch", "--force", "stable", "HEAD"]);
    fixture.git(&["checkout", "--detach", &source]);
    run(&input).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn rejects_different_existing_intent_without_overwriting_it() {
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    run(&input).unwrap();
    let original = fs::read(&path).unwrap();
    fixture.write(
        "notes.txt",
        "another commit with identical package versions\n",
    );
    fixture.commit("advance release line");
    fixture.git(&["branch", "--force", "stable", "HEAD"]);
    run(&preparation(&fixture, path.clone())).unwrap_err();
    assert_eq!(fs::read(&path).unwrap(), original);
}

fn preparation(fixture: &Fixture, output: PathBuf) -> RunInput {
    RunInput::PreparePublish {
        manifest_path: fixture.manifest(),
        config: None,
        source: fixture.sha("HEAD"),
        output,
        verbose: false,
    }
}

fn publication_source() -> Fixture {
    let fixture = Fixture::new("");
    write_package(&fixture, "library", "1.0.0", "");
    fixture.write(
        ".cargo/release_plan.toml",
        "schema-version = 1\nrepository = 'example/publication-fixture'\n\
         release-branch = 'stable'\ntargets = []\n",
    );
    let output = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(fixture.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    fixture.commit("publication source");
    fixture.git(&["branch", "stable", "HEAD"]);
    // Git's local URL rewrite exercises production fetch arguments without a network service.
    fixture.git(&[
        "config",
        &format!("url.{}.insteadOf", fixture.path().display()),
        "https://github.com/example/publication-fixture.git",
    ]);
    fixture
}
