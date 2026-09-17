use std::cell::Cell;

use super::*;

fn inputs() -> Inputs {
    Inputs {
        root: PathBuf::from("root"),
        manifest: PathBuf::from("Cargo.toml"),
        head: "head".to_owned(),
        base: "base".to_owned(),
        base_revision: "origin/main".to_owned(),
        index: "100644 blob 0\tCargo.toml\0".to_owned(),
        paths: BTreeSet::from([PathBuf::from("Cargo.toml"), PathBuf::from("src/lib.rs")]),
        digest: "initial".to_owned(),
    }
}

#[test]
fn captured_index_is_returned_verbatim() {
    let inputs = inputs();
    assert_eq!(inputs.index(), "100644 blob 0\tCargo.toml\0");
}

#[test]
fn retained_acquisition_pins_the_resolved_base_and_propagates_both_failures() {
    let inputs = inputs();
    for fail_capture in [false, true] {
        let compared = Cell::new(false);
        let error = inputs
            .verify_candidate_with(
                Path::new("retained/Cargo.toml"),
                "final",
                |manifest, base| {
                    assert_eq!(manifest, Path::new("retained/Cargo.toml"));
                    assert_eq!(base, Some("base"));
                    if fail_capture {
                        Err(CandidateFailure::new().into())
                    } else {
                        Ok(inputs.clone())
                    }
                },
                |current, digest| {
                    compared.set(true);
                    assert_eq!(*current, inputs);
                    assert_eq!(digest, "final");
                    Err(CandidateFailure::new().into())
                },
            )
            .unwrap_err();
        assert_eq!(compared.get(), !fail_capture);
        assert!(error.find_source::<CandidateFailure>().is_some());
        assert_eq!(error.find_source::<StaleInputs>().is_some(), fail_capture);
    }
    inputs
        .verify_candidate_with(
            Path::new("candidate"),
            "final",
            |_, _| Ok(inputs.clone()),
            |_, _| Ok(()),
        )
        .unwrap();
}

#[test]
fn changed_membership_with_unchanged_manifest_still_requires_identity_and_captured_bytes() {
    let inputs = inputs();
    for case in [PathCase::Sensitive, PathCase::Insensitive] {
        let probe = |_: &Path| case;
        let identity = PathIdentity::new(Path::new("retained"), &probe);
        for (paths, matches) in [
            (
                ["Cargo.toml", "src/LIB.rs"].as_slice(),
                case == PathCase::Insensitive,
            ),
            (["Cargo.toml"].as_slice(), false),
            (["Cargo.toml", "src/lib.rs", "extra.rs"].as_slice(), false),
        ] {
            let current = Inputs {
                root: PathBuf::from("retained"),
                paths: paths.iter().map(PathBuf::from).collect(),
                digest: "final".to_owned(),
                ..inputs.clone()
            };
            let called = Cell::new(false);
            let result = inputs.compare_candidate_with(&current, "final", &identity, || {
                called.set(true);
                Ok("final".to_owned())
            });
            assert_eq!(result.is_ok(), matches);
            assert_eq!(called.get(), matches);
            if matches {
                let error = inputs
                    .compare_candidate_with(&current, "final", &identity, || Ok("stale".to_owned()))
                    .unwrap_err();
                assert!(error.find_source::<StaleInputs>().is_some());
                let error = inputs
                    .compare_candidate_with(&current, "final", &identity, || {
                        Err(CandidateFailure::new().into())
                    })
                    .unwrap_err();
                assert!(error.find_source::<CandidateFailure>().is_some());
            }
        }
    }
}

#[test]
fn verification_validates_live_inputs_before_candidate_and_reports_success_only_after_both() {
    let mut plan = PlanFile::new(PlanStage::Expanded, Vec::new());
    let error = verify_preview(&plan, |_| panic!(), |_| panic!()).unwrap_err();
    assert!(error.find_source::<ResolutionRequired>().is_some());
    plan.resolved = Some(ResolvedState {
        inputs: inputs(),
        files: Vec::new(),
        final_digest: "final".to_owned(),
        versions: BTreeMap::new(),
        evidence_manifest_path: "retained/Cargo.toml".into(),
    });
    for failure in [Some(0), Some(1), None] {
        let calls = Cell::new(0);
        let visit = |state: &ResolvedState, step| {
            assert_eq!(calls.replace(step + 1), step);
            assert_eq!(Some(state), plan.resolved.as_ref());
            if failure == Some(step) {
                Err(CandidateFailure::new().into())
            } else {
                Ok(())
            }
        };
        let result = verify_preview(&plan, |state| visit(state, 0), |state| visit(state, 1));
        if let Some(step) = failure {
            assert!(
                result
                    .unwrap_err()
                    .find_source::<CandidateFailure>()
                    .is_some()
            );
            assert_eq!(calls.get(), step + 1);
        } else {
            assert_eq!(calls.get(), 2);
            assert_eq!(
                result.unwrap(),
                "Compatibility workspace matches the captured source, versions, and lockfile."
            );
        }
    }
}

/// Identifies an injected acquisition or verification failure without depending on diagnostics.
#[ohno::error]
struct CandidateFailure;
