// Expanded-plan inspection supplies typed facts to publication and evidence callers.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crp_diag::Verbose;
use crp_workspace::metadata::{WorkTree, load_tracked_work_tree};
use ohno::AppError;
use serde::Serialize;

use crate::groups::Groups;
use crate::plan::{PlanFile, PlanStage, resolve_plan};
use crate::resolved::{ResolvedState, apply_resolved, read_json};

/// Publication eligibility comes from tracked Cargo members, not package naming.
#[derive(Serialize)]
struct PlanInspection {
    publication_targets: Vec<String>,
    evidence_manifest_path: Option<PathBuf>,
}

// Only the real filesystem/Git adapters are excluded; inspection decisions and serialization
// run in the unit-tested core. See docs/implementation.md, "Test boundaries".
#[cfg_attr(test, mutants::skip)]
pub fn run_inspect_plan(
    path: &Path,
    require_resolved: bool,
    manifest: &Path,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    let plan: PlanFile = read_json(path)?;
    inspect_plan(
        plan,
        require_resolved,
        verbose,
        |plan| {
            // Reuse application's captured-state validation without installing any files.
            // The registry probe must see the same target set that application will accept.
            apply_resolved(plan, manifest, true, verbose).map(|_| ())
        },
        |state| state.verify_candidate(&state.evidence_manifest_path),
        || load_tracked_work_tree(manifest).map(|(work_tree, _)| work_tree),
    )
}

fn inspect_plan(
    plan: PlanFile,
    require_resolved: bool,
    verbose: Verbose<'_>,
    validate_resolved: impl FnOnce(&PlanFile) -> Result<(), AppError>,
    verify_candidate: impl FnOnce(&ResolvedState) -> Result<(), AppError>,
    load_work_tree: impl FnOnce() -> Result<WorkTree, AppError>,
) -> Result<String, AppError> {
    validate_expanded(&plan)?;
    if require_resolved || plan.resolved.is_some() {
        validate_resolved(&plan)?;
    }
    if let Some(state) = &plan.resolved {
        // Callers may run compatibility tooling immediately after consuming this path.
        verify_candidate(state)?;
    }
    let work_tree = load_work_tree()?;
    let resolved = resolve_plan(
        &plan,
        &Groups::from_workspace(&work_tree),
        &work_tree.target_versions(),
        verbose,
    )?;
    let publication_targets = work_tree
        .version_targets
        .iter()
        .filter(|target| target.publishable && resolved.packages.contains_key(&target.name))
        .map(|target| target.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let inspection = PlanInspection {
        publication_targets,
        evidence_manifest_path: plan.resolved.map(|state| state.evidence_manifest_path),
    };
    Ok(serde_json::to_string(&inspection)
        .expect("plan inspection contains only JSON-compatible artifact fields"))
}

fn validate_expanded(plan: &PlanFile) -> Result<(), AppError> {
    plan.validate_schema()?;
    let mut names = BTreeSet::new();
    if plan.stage() != PlanStage::Expanded
        || plan.increments.iter().any(|increment| {
            increment.version.is_none()
                || increment.level.is_some()
                || !names.insert(&increment.name)
        })
    {
        return Err(ExpandedPlanRequired::new().into());
    }
    Ok(())
}

/// Inspection requires the complete, uniquely named package/version set.
#[ohno::error]
#[display("inspection requires an expanded plan with one explicit version per package")]
struct ExpandedPlanRequired;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use crp_workspace::lockfile::InstallationGraph;
    use crp_workspace::metadata::VersionTarget;
    use semver::Version;
    use serde_json::{Value, json};

    use super::*;
    use crate::plan::PlanIncrement;
    use crate::{UnknownPlanTargetError, UnsupportedPlanSchemaError};

    #[test]
    fn resolved_validation_is_required_by_either_the_flag_or_captured_state() {
        for require_resolved in [false, true] {
            for has_state in [false, true] {
                let plan = plan(&[], has_state);
                let expected = plan.clone();
                let validated = Cell::new(false);
                let result = inspect_plan(
                    plan,
                    require_resolved,
                    Verbose::new(false, &crp_diag::Discard),
                    |plan| {
                        assert_eq!(*plan, expected);
                        validated.set(true);
                        Err(InspectionFailure::new().into())
                    },
                    |_| panic!(),
                    || Ok(work_tree(&[])),
                );
                match (require_resolved, has_state) {
                    (false, false) => {
                        assert!(!validated.get());
                        assert_eq!(
                            serde_json::from_str::<Value>(&result.unwrap()).unwrap(),
                            json!({
                                "publication_targets": [],
                                "evidence_manifest_path": null
                            })
                        );
                    }
                    _ => {
                        assert!(validated.get());
                        assert!(
                            result
                                .unwrap_err()
                                .find_source::<InspectionFailure>()
                                .is_some()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn inspection_verifies_evidence_before_loading_and_serializes_the_publication_intersection() {
        let plan = plan(&["zeta", "helper", "api"], true);
        let expected = plan.clone();
        let order = Cell::new(0);
        let output = inspect_plan(
            plan,
            false,
            Verbose::new(false, &crp_diag::Discard),
            |plan| {
                assert_eq!(order.replace(1), 0);
                assert_eq!(*plan, expected);
                Ok(())
            },
            |state| {
                assert_eq!(order.replace(2), 1);
                assert_eq!(state, expected.resolved.as_ref().unwrap());
                Ok(())
            },
            || {
                assert_eq!(order.replace(3), 2);
                Ok(work_tree(&[
                    ("zeta", true),
                    ("helper", false),
                    ("unselected-api", true),
                    ("unselected-helper", false),
                    ("api", true),
                    ("api", true),
                ]))
            },
        )
        .unwrap();
        assert_eq!(order.get(), 3);
        assert_eq!(
            serde_json::from_str::<Value>(&output).unwrap(),
            json!({
                "publication_targets": ["api", "zeta"],
                "evidence_manifest_path": expected.resolved.unwrap().evidence_manifest_path
            })
        );
    }

    #[test]
    fn stale_candidate_and_workspace_acquisition_errors_propagate() {
        let error = inspect_plan(
            plan(&[], true),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| Ok(()),
            |_| Err(InspectionFailure::new().into()),
            || panic!(),
        )
        .unwrap_err();
        assert!(error.find_source::<InspectionFailure>().is_some());

        let error = inspect_plan(
            plan(&[], false),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || Err(InspectionFailure::new().into()),
        )
        .unwrap_err();
        assert!(error.find_source::<InspectionFailure>().is_some());
    }

    #[test]
    fn invalid_artifacts_are_rejected_before_external_validation_or_acquisition() {
        let error = inspect_plan(
            PlanFile::new(PlanStage::Proposed, Vec::new()),
            true,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || panic!(),
        )
        .unwrap_err();
        assert!(error.find_source::<ExpandedPlanRequired>().is_some());

        let error = inspect_plan(
            PlanFile::with_schema_version(0),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || panic!(),
        )
        .unwrap_err();
        assert!(error.find_source::<UnsupportedPlanSchemaError>().is_some());
    }

    #[test]
    fn selected_names_must_resolve_against_tracked_workspace_targets() {
        let error = inspect_plan(
            plan(&["absent"], false),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || Ok(work_tree(&[("api", true)])),
        )
        .unwrap_err();
        assert!(error.find_source::<UnknownPlanTargetError>().is_some());
    }

    fn plan(names: &[&str], resolved: bool) -> PlanFile {
        let mut plan = PlanFile::new(
            PlanStage::Expanded,
            names
                .iter()
                .map(|name| PlanIncrement {
                    name: (*name).to_owned(),
                    level: None,
                    version: Some("1.0.1".to_owned()),
                })
                .collect(),
        );
        if resolved {
            // Capture contents are opaque to orchestration; its injected validators own them.
            plan.resolved = Some(
                serde_json::from_value(json!({
                    "inputs": {
                        "root": "workspace", "manifest": "Cargo.toml",
                        "head": "head", "base": "base", "base_revision": "main",
                        "index": "", "paths": [], "digest": "inputs"
                    },
                    "files": [], "final_digest": "candidate", "versions": {},
                    "evidence_manifest_path": "candidate \"quoted\"\\Cargo.toml"
                }))
                .unwrap(),
            );
        }
        plan
    }

    fn work_tree(targets: &[(&str, bool)]) -> WorkTree {
        WorkTree {
            workspace_root: PathBuf::from("workspace"),
            packages: Vec::new(),
            version_targets: targets
                .iter()
                .map(|(name, publishable)| VersionTarget {
                    name: (*name).to_owned(),
                    version: Version::new(1, 0, 0),
                    manifest_path: PathBuf::from(name).join("Cargo.toml"),
                    publishable: *publishable,
                })
                .collect(),
            exact_dependencies: Vec::new(),
            member_manifests: Vec::new(),
            members_by_dir: BTreeMap::new(),
            installation: InstallationGraph::default(),
        }
    }

    /// Identifies an injected external failure without relying on diagnostic wording.
    #[ohno::error]
    struct InspectionFailure;

    #[test]
    fn only_complete_explicit_unique_expansions_are_inspectable() {
        let increment = PlanIncrement {
            name: "api".to_owned(),
            level: None,
            version: Some("1.0.1".to_owned()),
        };
        let plan = PlanFile::new(PlanStage::Expanded, vec![increment.clone()]);
        validate_expanded(&plan).unwrap();
        for plan in [
            PlanFile::new(PlanStage::Proposed, vec![increment.clone()]),
            PlanFile::new(
                PlanStage::Expanded,
                vec![increment.clone(), increment.clone()],
            ),
            PlanFile::new(
                PlanStage::Expanded,
                vec![PlanIncrement {
                    level: Some("patch".to_owned()),
                    ..increment.clone()
                }],
            ),
            PlanFile::new(
                PlanStage::Expanded,
                vec![PlanIncrement {
                    version: None,
                    ..increment
                }],
            ),
        ] {
            assert!(
                validate_expanded(&plan)
                    .unwrap_err()
                    .find_source::<ExpandedPlanRequired>()
                    .is_some()
            );
        }
    }
}
