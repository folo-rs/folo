use std::cell::Cell;

use super::*;

fn inputs() -> Inputs {
    Inputs {
        root: PathBuf::from("root"),
        manifest: PathBuf::from("Cargo.toml"),
        head: "head".to_owned(),
        release_history: "base".to_owned(),
        release_history_revision: "origin/main".to_owned(),
        merge_target: None,
        merge_target_revision: None,
        index: "100644 blob 0\tCargo.toml\0".to_owned(),
        paths: BTreeSet::from([PathBuf::from("Cargo.toml"), PathBuf::from("src/lib.rs")]),
        digest: "initial".to_owned(),
    }
}

#[test]
fn history_verification_uses_original_repository_refs_and_preserves_failures() {
    let inputs = Inputs {
        merge_target: Some("parent-final".into()),
        merge_target_revision: Some("parent-branch".into()),
        ..inputs()
    };
    for fail in [false, true] {
        let mut called = false;
        let result = inputs.verify_history_with(|history, git| {
            called = true;
            assert_eq!(git.root(), Path::new("root"));
            assert_eq!(git.prefix(), "");
            assert_eq!(history.release_history, "base");
            assert_eq!(history.release_history_revision, "origin/main");
            assert_eq!(history.merge_target.as_deref(), Some("parent-final"));
            assert_eq!(
                history.merge_target_revision.as_deref(),
                Some("parent-branch")
            );
            if fail {
                Err(CandidateFailure::new().into())
            } else {
                Ok(())
            }
        });
        assert!(called);
        if fail {
            let error = result.unwrap_err();
            assert!(error.find_source::<StaleInputs>().is_some());
            assert!(error.find_source::<CandidateFailure>().is_some());
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn captured_index_is_returned_verbatim() {
    let inputs = inputs();
    assert_eq!(inputs.index(), "100644 blob 0\tCargo.toml\0");
}

#[test]
fn live_verification_distinguishes_original_final_and_stale_inputs() {
    let inputs = Inputs {
        merge_target: Some("parent".into()),
        merge_target_revision: Some("parent-branch".into()),
        ..inputs()
    };
    for (digest, expected) in [
        ("initial", Some(false)),
        ("final", Some(true)),
        ("stale", None),
    ] {
        let result = inputs.verify_with(
            Path::new("root/Cargo.toml"),
            Some("final"),
            |manifest, history, target| {
                assert_eq!(manifest, Path::new("root/Cargo.toml"));
                assert_eq!(history, Some("origin/main"));
                assert_eq!(target, Some("parent-branch"));
                Ok(Inputs {
                    digest: digest.into(),
                    ..inputs.clone()
                })
            },
        );
        if let Some(expected) = expected {
            assert_eq!(result.unwrap(), expected);
        } else {
            assert!(result.unwrap_err().find_source::<StaleInputs>().is_some());
        }
    }
    assert!(
        inputs
            .verify_with(Path::new("root/Cargo.toml"), None, |_, _, _| Err(
                CandidateFailure::new().into()
            ))
            .unwrap_err()
            .find_source::<CandidateFailure>()
            .is_some()
    );
}

#[test]
fn final_digest_uses_exact_artifact_bytes_and_propagates_fingerprint_failure() {
    let inputs = inputs();
    let identity = PathIdentity::new(inputs.root(), &|_| PathCase::Sensitive);
    let files = vec![Artifact {
        path: "Cargo.toml".into(),
        contents: "captured\n".into(),
    }];
    assert_eq!(
        inputs
            .final_digest_with(&files, &identity, |replacements| {
                assert_eq!(
                    *replacements,
                    BTreeMap::from([(PathBuf::from("Cargo.toml"), b"captured\n".to_vec())])
                );
                Ok("fingerprint".into())
            })
            .unwrap(),
        "fingerprint"
    );
    assert!(
        inputs
            .final_digest_with(&files, &identity, |_| Err(CandidateFailure::new().into()))
            .unwrap_err()
            .find_source::<CandidateFailure>()
            .is_some()
    );
}

#[test]
fn artifact_validation_requires_versions_membership_supported_paths_and_final_bytes() {
    let inputs = inputs();
    let identity = PathIdentity::new(inputs.root(), &|_| PathCase::Sensitive);
    let versions = BTreeMap::from([("pkg".into(), "1.0.1".into())]);
    let allowed = BTreeSet::from([PathBuf::from("Cargo.toml"), PathBuf::from("src/lib.rs")]);
    let state = ResolvedState {
        inputs: inputs.clone(),
        files: vec![Artifact {
            path: "Cargo.toml".into(),
            contents: "final".into(),
        }],
        final_digest: "final-digest".into(),
        versions: versions.clone(),
        evidence_manifest_path: "retained/Cargo.toml".into(),
    };
    state
        .validate_artifacts_with(&versions, &allowed, &identity, || Ok("final-digest".into()))
        .unwrap();
    for path in ["src/lib.rs", "other/Cargo.toml"] {
        let mut invalid = state.clone();
        invalid.files.first_mut().unwrap().path = path.into();
        assert!(
            invalid
                .validate_artifacts_with(&versions, &allowed, &identity, || panic!(
                    "invalid artifact"
                ))
                .unwrap_err()
                .find_source::<ResolutionRequired>()
                .is_some()
        );
    }
    assert!(
        state
            .validate_artifacts_with(&BTreeMap::new(), &allowed, &identity, || panic!(
                "wrong versions"
            ))
            .is_err()
    );
    assert!(
        state
            .validate_artifacts_with(&versions, &allowed, &identity, || Ok("changed".into()))
            .unwrap_err()
            .find_source::<ResolutionRequired>()
            .is_some()
    );
    assert!(
        state
            .validate_artifacts_with(&versions, &allowed, &identity, || Err(
                CandidateFailure::new().into()
            ))
            .unwrap_err()
            .find_source::<CandidateFailure>()
            .is_some()
    );
}

#[test]
fn json_emission_preserves_serialized_values_and_write_failures() {
    let path = Path::new("output.json");
    let value = BTreeMap::from([("contents", "quotes \" and newline\n")]);
    let mut called = false;
    write_json_with(path, &value, |actual, bytes| {
        called = true;
        assert_eq!(actual, path);
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(
            serde_json::from_slice::<BTreeMap<String, String>>(bytes).unwrap(),
            value
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect()
        );
        Ok(())
    })
    .unwrap();
    assert!(called);
    assert!(
        write_json_with(path, &value, |_, _| Err(ErrorKind::PermissionDenied.into()))
            .unwrap_err()
            .find_source::<WriteFileError>()
            .is_some()
    );
}

#[test]
fn source_collection_follows_acquired_directories_and_preserves_actual_paths() {
    let root = Path::new("root");
    let mut paths = BTreeSet::new();
    let mut visited = Vec::new();
    collect_sources_with(root, &root.join("src"), &mut paths, &mut |path| {
        visited.push(path.to_path_buf());
        if path == root.join("src") {
            Ok(Some(vec![
                (root.join("src/lib.rs"), false),
                (root.join("src/nested"), true),
                (root.join("src/missing"), true),
            ]))
        } else if path == root.join("src/nested") {
            Ok(Some(vec![(root.join("src/nested/mod.rs"), false)]))
        } else {
            assert_eq!(path, root.join("src/missing"));
            Ok(None)
        }
    })
    .unwrap();
    assert_eq!(
        paths,
        ["src/lib.rs", "src/nested/mod.rs"]
            .map(PathBuf::from)
            .into()
    );
    assert_eq!(
        visited,
        ["src", "src/nested", "src/missing"].map(|path| root.join(path))
    );
    assert!(
        collect_sources_with(root, &root.join("src"), &mut paths, &mut |_| Err(
            CandidateFailure::new().into()
        ))
        .unwrap_err()
        .find_source::<CandidateFailure>()
        .is_some()
    );
}

#[test]
fn retained_acquisition_pins_history_and_target_and_propagates_both_failures() {
    let inputs = Inputs {
        merge_target: Some("parent-final".to_owned()),
        merge_target_revision: Some("parent-branch".to_owned()),
        ..inputs()
    };
    for fail_capture in [false, true] {
        let compared = Cell::new(false);
        let error = inputs
            .verify_candidate_with(
                Path::new("retained/Cargo.toml"),
                "final",
                |manifest, release_history, target| {
                    assert_eq!(manifest, Path::new("retained/Cargo.toml"));
                    assert_eq!(release_history, Some("base"));
                    assert_eq!(target, Some("parent-final"));
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
            |_, _, _| Ok(inputs.clone()),
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
