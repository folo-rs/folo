//! Captured Git membership and standalone preview completion boundaries.

use std::fs;

use cargo_release_plan::{RunInput, run};
use crp_versioning::plan::SCHEMA_VERSION;
use serde_json::json;
use tempfile::tempdir;

use crate::harness::{prepare, seeded_package};

#[test]
#[cfg_attr(miri, ignore = "executes preparation against a real source directory")]
fn preparation_rejects_source_output_before_mutation() {
    let fixture = seeded_package();
    let output = fixture.path().join("packages/demo/src/evidence");
    let result = run(&RunInput::Prepare {
        merge_target: None,
        output: output.clone(),
        release_history: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    });
    assert!(!output.exists(), "{result:?}");
    result.unwrap_err();
    assert!(!fixture.path().join("Cargo.lock").exists());
}

#[test]
#[cfg_attr(miri, ignore = "executes preview against a real source directory")]
fn preview_rejects_source_output_before_mutation() {
    let fixture = seeded_package();
    let prepared = prepare(&fixture);
    let proposal = fixture.path().join("proposal.json");
    fs::write(
        &proposal,
        serde_json::to_vec(&json!({
            "schema_version": SCHEMA_VERSION,
            "increments": []
        }))
        .unwrap(),
    )
    .unwrap();
    let output = fixture.path().join("packages/demo/src/evidence");
    let result = run(&RunInput::Preview {
        plan: proposal,
        prepared,
        output: output.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    });
    assert!(!output.exists(), "{result:?}");
    result.unwrap_err();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "checks that output marker cleanup cannot remove admitted source"
)]
fn source_output_markers_are_preserved_before_preparation_and_preview() {
    let fixture = seeded_package();
    fixture.write(
        "packages/demo/src/evidence/prepared.json",
        "source preparation",
    );
    fixture.write("packages/demo/src/evidence/plan.json", "source plan");
    let output = fixture.path().join("packages/demo/src/evidence");
    run(&RunInput::Prepare {
        output: output.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    let prepared = prepare(&fixture);
    run(&RunInput::Preview {
        plan: fixture.path().join("missing-proposal.json"),
        prepared,
        output,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(
        fixture.read("packages/demo/src/evidence/prepared.json"),
        "source preparation"
    );
    assert_eq!(
        fixture.read("packages/demo/src/evidence/plan.json"),
        "source plan"
    );
    assert!(
        !fixture
            .path()
            .join("packages/demo/src/evidence/.prospective")
            .exists()
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "reserves an owned prospective directory through preparation"
)]
fn preparation_preserves_an_occupied_prospective_directory() {
    let fixture = seeded_package();
    fixture.write("prepared/.prospective/keep", "another owner");
    run(&RunInput::Prepare {
        merge_target: None,
        output: fixture.path().join("prepared"),
        release_history: Some("HEAD".to_owned()),
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
    let before = r#"{"schema_version":6,"increments":[]}"#;
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
#[cfg_attr(
    miri,
    ignore = "preserves a real preview marker when source admission fails"
)]
fn unavailable_source_cannot_authorize_removing_a_previous_output() {
    let fixture = seeded_package();
    let evidence = tempdir().unwrap();
    for contents in [Some("["), None] {
        if let Some(contents) = contents {
            fixture.write("Cargo.toml", contents);
        } else {
            fs::remove_file(fixture.manifest()).unwrap();
        }
        let marker = evidence.path().join("plan.json");
        fs::write(&marker, "previous completion").unwrap();
        run(&RunInput::Preview {
            plan: fixture.path().join("proposal.json"),
            prepared: fixture.path().join("prepared.json"),
            output: evidence.path().to_owned(),
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap_err();
        assert_eq!(fs::read_to_string(marker).unwrap(), "previous completion");
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "checks captured dependency and resource locations before preparation"
)]
fn preparation_protects_ignored_dependency_sources_and_package_resources() {
    let fixture = seeded_package();
    fixture.write_workspace("exclude = ['target/dependency']\n");
    fixture.write(".gitignore", "/target/\n");
    fixture.write(
        "packages/demo/Cargo.toml",
        "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n\
         readme='../../target/docs/manual.md'\n\
         [dependencies]\nhelper={path='../../target/dependency'}\n",
    );
    fixture.write(
        "target/dependency/Cargo.toml",
        "[package]\nname='helper'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    );
    fixture.write("target/dependency/src/lib.rs", "pub fn helper() {}\n");
    fixture.write("target/docs/manual.md", "resource");
    fixture.commit("ignored local inputs");
    for path in ["target/dependency/src/evidence", "target/docs"] {
        let result = run(&RunInput::Prepare {
            merge_target: None,
            output: fixture.path().join(path),
            release_history: Some("HEAD".to_owned()),
            manifest_path: fixture.manifest(),
            verbose: false,
        });
        result.unwrap_err();
        assert!(!fixture.path().join(path).join(".prospective").exists());
        assert!(!fixture.path().join("Cargo.lock").exists());
    }
    assert_eq!(fixture.read("target/docs/manual.md"), "resource");
    assert_eq!(
        fixture.read("target/dependency/src/lib.rs"),
        "pub fn helper() {}\n"
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "prepares, previews and applies retained evidence in excluded directories"
)]
fn excluded_outputs_retain_applicable_evidence() {
    for directory in ["target/release-evidence", "packages/demo/evidence"] {
        let fixture = seeded_package();
        fixture.write(
            "packages/demo/Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n\
             exclude=['evidence/**']\n[package.metadata.release-plan]\nprivate-api=true\n",
        );
        fixture.commit("excluded output policy");
        let output = fixture.path().join(directory);
        let prepared = output.join("prepared");
        run(&RunInput::Prepare {
            output: prepared.clone(),
            release_history: Some("HEAD".to_owned()),
            merge_target: None,
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap();
        let proposal = fixture.path().join("proposal.json");
        fs::write(
            &proposal,
            json!({
                "schema_version": SCHEMA_VERSION,
                "increments": [{"name": "demo", "bump": "patch"}]
            })
            .to_string(),
        )
        .unwrap();
        let preview = output.join("preview");
        run(&RunInput::Preview {
            plan: proposal,
            prepared: prepared.join("prepared.json"),
            output: preview.clone(),
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap();
        run(&RunInput::CheckCompatibility {
            manifest_path: fixture.manifest(),
            prepared: None,
            plan: Some(preview.join("plan.json")),
            release_history: None,
            merge_target: None,
            output: output.join("compatibility"),
            deny_findings: true,
            verbose: false,
        })
        .unwrap();
        run(&RunInput::Apply {
            plan: preview.join("plan.json"),
            dry_run: false,
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap();
        assert!(fixture.read("packages/demo/Cargo.toml").contains("0.1.1"));
        assert!(preview.join("workspace/Cargo.toml").is_file());
    }
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
        merge_target: None,
        output: fixture.path().join("prepared"),
        release_history: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!fixture.path().join("prepared/prepared.json").exists());
    assert!(!fixture.path().join("prepared/.prospective").exists());
    assert!(!fixture.path().join("Cargo.lock").exists());
    assert_eq!(fixture.read("packages/demo/Cargo.toml"), manifest);
}
