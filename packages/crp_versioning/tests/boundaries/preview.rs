//! External acquisition for preview.

use std::cell::Cell;
use std::fs;

use crp_versioning::plan::SCHEMA_VERSION;
use crp_versioning::preview::*;
use crp_versioning::resolved::read_json;
use serde_json::{Value, json};
use tempfile::tempdir;

fn prepared_document() -> String {
    json!({
        "schema_version": SCHEMA_VERSION,
        "inputs": {
            "root": "repository", "manifest": "Cargo.toml", "head": "head", "base": "base",
            "base_revision": "main", "index": "index", "paths": ["Cargo.toml"], "digest": "initial"
        }
    })
    .to_string()
}

#[test]
#[cfg_attr(miri, ignore = "uses owned preview artifact files")]
fn failed_input_reads_and_verification_invalidate_the_previous_completion_marker() {
    let directory = tempdir().unwrap();
    let output = directory.path();
    let marker = output.join("plan.json");
    let prepared = output.join("prepared.json");
    let proposal = output.join("proposal.json");
    let manifest = output.join("Cargo.toml");
    let verified = Cell::new(false);
    fs::write(&proposal, "{ invalid plan").unwrap();
    fs::write(&prepared, "{ invalid preparation").unwrap();
    fs::write(&marker, "previous completion").unwrap();
    let error = preview_inputs(&proposal, &prepared, output, &manifest, |_| {
        verified.set(true);
        Ok(())
    })
    .err()
    .unwrap();
    assert!(error.find_source::<serde_json::Error>().is_some());
    assert!(!verified.get());
    assert!(!marker.exists());

    fs::write(&prepared, prepared_document()).unwrap();
    fs::write(&marker, "previous completion").unwrap();
    let error = preview_inputs(&proposal, &prepared, output, &manifest, |inputs| {
        assert!(!marker.exists());
        verified.set(true);
        let mut changed = inputs.clone();
        changed.head.push_str("-changed");
        inputs.compare(&changed, None).map(|_| ())
    })
    .err()
    .unwrap();
    // Staleness wins over the malformed proposal, preserving input acquisition order.
    assert!(error.find_source::<serde_json::Error>().is_none());
    assert!(verified.get());
    assert!(!marker.exists());

    fs::write(&marker, "previous completion").unwrap();
    let error = preview_inputs(&proposal, &prepared, output, &manifest, |inputs| {
        assert!(!marker.exists());
        assert_eq!(inputs.head, "head");
        Ok(())
    })
    .err()
    .unwrap();
    assert!(error.find_source::<serde_json::Error>().is_some());
    assert!(!marker.exists());

    fs::write(&proposal, r#"{"schema_version":4,"increments":[]}"#).unwrap();
    let (_, plan) = preview_inputs(&proposal, &prepared, output, &manifest, |_| Ok(())).unwrap();
    assert!(plan.increments.is_empty());
    assert!(!marker.exists());
}

#[test]
#[cfg_attr(miri, ignore = "checks owned filesystem output aliases")]
fn preview_collisions_preserve_inputs_and_never_acquire_repository_state() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("preview");
    fs::create_dir_all(&output).unwrap();
    for relative in [
        "plan.json",
        "report.json",
        "report.json.tmp",
        "diffs/proposal.json",
        "workspace/proposal.json",
        ".prospective/proposal.json",
    ] {
        let input = output.join(relative);
        fs::create_dir_all(input.parent().unwrap()).unwrap();
        fs::write(&input, "input document").unwrap();
        for position in 0..3 {
            let unrelated = directory.path().join("unrelated");
            let mut inputs = [&unrelated, &unrelated, &unrelated];
            *inputs.get_mut(position).unwrap() = &input;
            let _error = preview_inputs(inputs[0], inputs[1], &output, inputs[2], |_| {
                panic!("input collisions must be rejected before repository acquisition")
            })
            .err()
            .unwrap();
            assert_eq!(fs::read_to_string(&input).unwrap(), "input document");
        }
    }
    let input = output.join("plan.json");
    fs::write(&input, "input document").unwrap();
    let alias = directory.path().join("missing/../preview");
    let _error = preview_inputs(&input, &input, &alias, &input, |_| {
        panic!("output aliases must be rejected before repository acquisition")
    })
    .err()
    .unwrap();
    assert_eq!(fs::read_to_string(input).unwrap(), "input document");
}

#[test]
#[cfg_attr(miri, ignore = "reads an owned prepared artifact")]
fn preparation_does_not_accept_alternative_resolution_artifacts() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("prepared.json");
    let mut prepared: Value = serde_json::from_str(&prepared_document()).unwrap();
    prepared.as_object_mut().unwrap().insert(
        "files".to_owned(),
        json!([{"path":"Cargo.lock","contents":"alternative resolution"}]),
    );
    fs::write(&path, prepared.to_string()).unwrap();
    let error = read_json::<Prepared>(&path).err().unwrap();
    assert!(error.find_source::<serde_json::Error>().is_some());
}

#[test]
#[cfg_attr(miri, ignore = "checks an owned preview marker directory")]
fn occupied_completion_marker_precedes_input_acquisition() {
    let directory = tempdir().unwrap();
    let marker = directory.path().join("plan.json/keep");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "not a completion file").unwrap();
    let absent = directory.path().join("absent");
    let error = preview_inputs(&absent, &absent, directory.path(), &absent, |_| {
        panic!("an occupied marker must fail before repository acquisition")
    })
    .err()
    .unwrap();
    assert!(error.find_source::<std::io::Error>().is_some());
    assert_eq!(fs::read_to_string(marker).unwrap(), "not a completion file");
}
