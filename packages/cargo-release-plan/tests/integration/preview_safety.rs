//! Captured Git membership and standalone preview completion boundaries.

use std::fs;

use cargo_release_plan::{RunInput, run};
use crp_versioning::plan::SCHEMA_VERSION;
use serde_json::json;
use tempfile::tempdir;

use crate::harness::{prepare, seeded_package};

#[test]
#[cfg_attr(
    miri,
    ignore = "prepares evidence in a caller-selected source directory"
)]
fn preparation_uses_the_caller_selected_output() {
    let fixture = seeded_package();
    let output = fixture.path().join("packages/demo/src/evidence");
    run(&RunInput::Prepare {
        output: output.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert!(output.join("prepared.json").is_file());
    assert!(output.join("report.json").is_file());
    assert!(!output.join(".prospective").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "previews evidence in a caller-selected source directory"
)]
fn preview_uses_the_caller_selected_output() {
    let fixture = seeded_package();
    let prepared = prepare(&fixture);
    let proposal = fixture.path().join("proposal.json");
    fs::write(
        &proposal,
        json!({"schema_version": SCHEMA_VERSION, "increments": []}).to_string(),
    )
    .unwrap();
    let output = fixture.path().join("packages/demo/src/evidence");
    run(&RunInput::Preview {
        plan: proposal,
        prepared,
        output: output.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert!(output.join("plan.json").is_file());
    assert!(output.join("workspace/Cargo.toml").is_file());
    assert!(!output.join(".prospective").exists());
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
#[cfg_attr(
    miri,
    ignore = "preserves a real preview marker when source admission fails"
)]
fn unavailable_source_cannot_authorize_removing_a_previous_output() {
    let fixture = seeded_package();
    let prepared = prepare(&fixture);
    fixture.write(
        "proposal.json",
        &json!({"schema_version": SCHEMA_VERSION, "increments": []}).to_string(),
    );
    let evidence = tempdir().unwrap();
    for contents in [Some("["), None] {
        if let Some(contents) = contents {
            fixture.write("Cargo.toml", contents);
        } else {
            fs::remove_file(fixture.manifest()).unwrap();
        }
        let marker = evidence.path().join("plan.json");
        fs::write(&marker, "previous completion").unwrap();
        let error = run(&RunInput::Preview {
            plan: fixture.path().join("proposal.json"),
            prepared: prepared.clone(),
            output: evidence.path().to_owned(),
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap_err();
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("stale"));
        assert!(diagnostic.contains("Cargo.toml"));
        assert_eq!(fs::read_to_string(marker).unwrap(), "previous completion");
    }
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
