//! Retained final workspaces used for compatibility evidence before application.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_release_plan::{RunInput, RunOutcome, run};
use ohno::AppError;
use serde_json::{Value, json};

use crate::fixture::{Fixture, write_package};
use crate::harness::{check, resolved_plan};

fn evidence_manifest(plan: &Path) -> PathBuf {
    let plan: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    PathBuf::from(
        plan.get("resolved")
            .unwrap()
            .get("evidence_manifest_path")
            .unwrap()
            .as_str()
            .unwrap(),
    )
}

fn verify(plan: &Path, manifest: &Path) -> Result<(), AppError> {
    run(&RunInput::VerifyPreview {
        plan: plan.to_path_buf(),
        manifest_path: manifest.to_path_buf(),
        verbose: false,
    })
    .map(|_| ())
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns Git and offline Cargo against retained workspaces"
)]
fn resolved_workspace_enforces_evidence_and_application_boundaries() {
    let fixture = Fixture::new("");
    write_package(&fixture, "core", "0.1.0", "");
    write_package(
        &fixture,
        "tool",
        "0.1.0",
        "\n[package.metadata.cargo_check_external_types]\nallowed_external_types = [\"core::*\"]\n\
         [dependencies]\ncore = { path = \"../core\", version = \"0.1.0\" }\n",
    );
    write_package(
        &fixture,
        "helper",
        "0.1.0",
        "\npublish = false\n[dependencies]\ncore = { path = \"../core\", version = \"=0.1.0\" }\n",
    );
    fixture.write("packages/tool/src/main.rs", "fn main() {}\n");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("released workspace");
    fixture.write(
        "proposal.json",
        r#"{"schema_version":4,"increments":[{"name":"core","level":"minor"}]}"#,
    );
    let plan = resolved_plan(&fixture, &fixture.path().join("proposal.json"));
    let candidate = evidence_manifest(&plan);
    // Mutations of emitted paths must never control fixture writes or recursive cleanup.
    let root = fixture.path().join("preview/workspace");
    assert_eq!(candidate, root.join("Cargo.toml"));
    assert!(
        fs::read_to_string(root.join("packages/tool/Cargo.toml"))
            .unwrap()
            .contains("0.2.0")
    );
    assert!(
        fs::read_to_string(root.join("packages/helper/Cargo.toml"))
            .unwrap()
            .contains("=0.2.0")
    );
    assert_eq!(root, fixture.path().join("preview/workspace"));
    assert!(
        fs::read_to_string(root.join("packages/core/Cargo.toml"))
            .unwrap()
            .contains("0.2.0")
    );
    assert!(fixture.read("packages/core/Cargo.toml").contains("0.1.0"));
    let expected_lock = fs::read(root.join("Cargo.lock")).unwrap();
    assert_ne!(
        expected_lock,
        fs::read(fixture.path().join("Cargo.lock")).unwrap()
    );

    let output = Command::new("cargo")
        .current_dir(&root)
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&candidate)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    verify(&plan, &candidate).unwrap();
    assert_eq!(fs::read(root.join("Cargo.lock")).unwrap(), expected_lock);
    let inspection = RunInput::InspectPlan {
        plan: plan.clone(),
        require_resolved: true,
        manifest_path: fixture.manifest(),
        verbose: false,
    };
    let RunOutcome::ArtifactQuery { message } = run(&inspection).unwrap() else {
        panic!()
    };
    let result: Value = serde_json::from_str(&message).unwrap();
    assert_eq!(
        result.get("publication_targets").unwrap(),
        &json!(["core", "tool"])
    );
    assert_eq!(
        Path::new(
            result
                .get("evidence_manifest_path")
                .unwrap()
                .as_str()
                .unwrap()
        ),
        candidate
    );
    let original: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    let mut edited = original.clone();
    *edited
        .pointer_mut("/resolved/evidence_manifest_path")
        .unwrap() = json!(fixture.manifest());
    fs::write(&plan, serde_json::to_vec(&edited).unwrap()).unwrap();
    run(&inspection).unwrap_err();
    fs::write(&plan, serde_json::to_vec(&original).unwrap()).unwrap();

    let source = root.join("packages/core/src/lib.rs");
    let original_source = fs::read(&source).unwrap();
    fs::write(&source, "pub fn changed_after_evidence() {}\n").unwrap();
    run(&inspection).unwrap_err();
    fs::write(&source, &original_source).unwrap();

    let live_source = fixture.read("packages/core/src/lib.rs");
    fixture.write(
        "packages/core/src/lib.rs",
        "pub fn changed_live_source() {}\n",
    );
    verify(&plan, &candidate).unwrap_err();
    fixture.write("packages/core/src/lib.rs", &live_source);

    let manifest = fixture.read("packages/core/Cargo.toml");
    let lockfile = fixture.read("Cargo.lock");
    let mut edited = original.clone();
    // The invalid artifact follows legitimate writes, exercising validation-before-installation.
    edited
        .pointer_mut("/resolved/files")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(json!({"path":"packages/core/src/lib.rs","contents":"unplanned source"}));
    fs::write(&plan, serde_json::to_vec(&edited).unwrap()).unwrap();
    run(&RunInput::Apply {
        plan: plan.clone(),
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fixture.read("packages/core/Cargo.toml"), manifest);
    assert_eq!(fixture.read("Cargo.lock"), lockfile);
    assert_eq!(fixture.read("packages/core/src/lib.rs"), live_source);
    fs::write(&plan, serde_json::to_vec(&original).unwrap()).unwrap();

    // Application depends on the original snapshot and captured bytes, not the retained tree.
    fs::remove_dir_all(root).unwrap();
    run(&RunInput::Apply {
        plan,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert!(check(&fixture, "HEAD").0);
}
