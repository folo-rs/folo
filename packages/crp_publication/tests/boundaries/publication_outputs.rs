//! Outcome paths must remain usable before any phase can create directories or write remotely.
#![cfg_attr(coverage_nightly, coverage(off))]

use std::path::Path;
use std::sync::Arc;

use crp_diag::Discard;
use crp_publication::PublicationOutput;
use crp_publication::publication::{binaries, github};

#[test]
#[cfg_attr(
    miri,
    ignore = "Resolves destination paths against a temporary directory"
)]
fn overlapping_phase_outputs_fail_before_inputs_or_artifact_directories_are_touched() {
    let directory = tempfile::tempdir().unwrap();
    let diagnostics = PublicationOutput::new("1.0.0", false, Arc::new(Discard));
    let missing = directory.path().join("missing-input.json");
    for nested in [false, true] {
        let output = directory.path().join("reserved-outcome");
        let artifacts = if nested {
            output.join("artifacts")
        } else {
            output.clone()
        };
        let error = binaries::publish::publish(
            &missing,
            &missing,
            Path::new("Cargo.toml"),
            &output,
            &artifacts,
            true,
            &diagnostics,
        )
        .unwrap_err();
        assert!(error.to_string().contains("reserved-outcome"));
        assert!(!error.to_string().contains("missing-input"));
        assert!(!output.exists());
        let error = github::publish(
            &missing,
            Path::new("Cargo.toml"),
            &output,
            &artifacts,
            true,
            &diagnostics,
        )
        .unwrap_err();
        assert!(error.to_string().contains("reserved-outcome"));
        assert!(!error.to_string().contains("missing-input"));
        assert!(!output.exists());
    }
}
