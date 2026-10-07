//! External acquisition for preview.

use std::cell::Cell;
use std::fs;

use crp_diag::{Discard, Verbose};
use crp_versioning::plan::SCHEMA_VERSION;
use crp_versioning::preview::*;
use crp_versioning::resolved::read_json;
use serde_json::{Value, json};
use tempfile::tempdir;

fn prepared_document() -> String {
    json!({
        "schema_version": SCHEMA_VERSION,
        "inputs": {
            "root": "repository", "manifest": "Cargo.toml", "head": "head", "release_history": "base",
            "release_history_revision": "main", "index": "index", "paths": ["Cargo.toml"], "digest": "initial"
        }
    })
    .to_string()
}

#[test]
#[cfg_attr(miri, ignore = "uses owned preview artifact files")]
fn marker_invalidation_requires_source_admission_and_precedes_proposal_reads() {
    let directory = tempdir().unwrap();
    let output = directory.path();
    let marker = output.join("plan.json");
    let prepared = output.join("prepared.json");
    let proposal = output.join("proposal.json");
    let verified = Cell::new(false);
    fs::write(&proposal, "{ invalid plan").unwrap();
    fs::write(&prepared, "{ invalid preparation").unwrap();
    fs::write(&marker, "previous completion").unwrap();
    let error = preview_inputs(&proposal, &prepared, output, |_| {
        verified.set(true);
        Ok(())
    })
    .err()
    .unwrap();
    assert!(error.find_source::<serde_json::Error>().is_some());
    assert!(!verified.get());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "previous completion");

    fs::write(&prepared, prepared_document()).unwrap();
    fs::write(&marker, "previous completion").unwrap();
    let error = preview_inputs(&proposal, &prepared, output, |inputs| {
        assert_eq!(fs::read_to_string(&marker).unwrap(), "previous completion");
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
    assert_eq!(fs::read_to_string(&marker).unwrap(), "previous completion");

    fs::write(&marker, "previous completion").unwrap();
    let error = preview_inputs(&proposal, &prepared, output, |inputs| {
        assert_eq!(fs::read_to_string(&marker).unwrap(), "previous completion");
        assert_eq!(inputs.head, "head");
        Ok(())
    })
    .err()
    .unwrap();
    assert!(error.find_source::<serde_json::Error>().is_some());
    assert!(!marker.exists());

    fs::write(&proposal, r#"{"schema_version":7,"increments":[]}"#).unwrap();
    let (_, plan) = preview_inputs(&proposal, &prepared, output, |_| Ok(())).unwrap();
    assert!(plan.increments.is_empty());
    assert!(!marker.exists());
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
fn occupied_completion_marker_survives_failed_admission() {
    let directory = tempdir().unwrap();
    let marker = directory.path().join("plan.json/keep");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "not a completion file").unwrap();
    let absent = directory.path().join("absent");
    let error = preview_inputs(&absent, &absent, directory.path(), |_| {
        panic!("missing preparation must fail before repository acquisition")
    })
    .err()
    .unwrap();
    assert!(error.find_source::<std::io::Error>().is_some());
    assert_eq!(fs::read_to_string(marker).unwrap(), "not a completion file");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "checks default preview admission with missing native inputs"
)]
fn unavailable_preview_inputs_never_authorize_marker_or_alias_removal() {
    let directory = tempdir().unwrap();
    let marker = directory.path().join("plan.json");
    let absent = directory.path().join("absent");
    fs::write(&marker, "previous completion").unwrap();
    run_preview(
        &absent,
        &absent,
        directory.path(),
        &absent,
        Verbose::new(false, &Discard),
    )
    .unwrap_err();
    assert_eq!(fs::read_to_string(&marker).unwrap(), "previous completion");

    fs::write(&marker, "input document").unwrap();
    run_preview(
        &marker,
        &absent,
        &directory.path().join("missing/.."),
        &absent,
        Verbose::new(false, &Discard),
    )
    .unwrap_err();
    assert_eq!(fs::read_to_string(marker).unwrap(), "input document");
}
